//! pi's `models.json` for the guest: the host's local model servers, reachable from the VM.
//!
//! Host side only; the file is written into the sandbox by `sandbox.rs`. The host's own
//! `models.json` may hold real cloud keys, so only providers on a loopback address are kept,
//! with that address rewritten to microsandbox's host alias. Without such a file, the usual
//! Ollama and LM Studio ports are probed instead.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};

/// The name microsandbox resolves to the host's loopback interface inside a VM.
pub(crate) const HOST_ALIAS: &str = "host.microsandbox.internal";

/// Connect, write and read deadline for each local server probe.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// Default ports of the local model servers worth probing: (pi provider id, port).
const SERVERS: [(&str, u16); 2] = [("ollama", 11434), ("lmstudio", 1234)];

const LOOPBACK_HOSTS: [&str; 4] = ["127.0.0.1", "localhost", "[::1]", "0.0.0.0"];

/// The `models.json` to write into a pi sandbox, or `None` when the host offers no local
/// model server. Blocking (file and socket IO): call it off the async runtime.
pub(crate) fn guest_models_json() -> Result<Option<String>> {
    let agent_dir = std::env::var("PI_CODING_AGENT_DIR").ok();
    let path = host_models_path(agent_dir.as_deref(), std::env::home_dir().as_deref());
    let from_file = match path.filter(|path| path.is_file()) {
        Some(path) => {
            let parsed = std::fs::read_to_string(&path)
                .context("reading the file")
                .and_then(|text| local_providers(&text));
            // A broken host file should not stop pi from starting: fall back to probing.
            parsed.unwrap_or_else(|error| {
                eprintln!("note: ignoring {}: {error:#}", path.display());
                None
            })
        }
        None => None,
    };
    let providers = from_file.or_else(|| probed_providers(probe));
    providers
        .map(|value| serde_json::to_string_pretty(&value))
        .transpose()
        .context("serialising models.json")
}

/// pi's user `models.json`: `$PI_CODING_AGENT_DIR/models.json`, else `~/.pi/agent/models.json`.
pub(crate) fn host_models_path(agent_dir: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    let dir = match agent_dir.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home?.join(".pi").join("agent"),
    };
    Some(dir.join("models.json"))
}

/// Keeps only the providers of a pi `models.json` whose `baseUrl` is on a loopback address,
/// rewritten to the host alias. `None` when nothing is left.
pub(crate) fn local_providers(text: &str) -> Result<Option<Value>> {
    let document: Value = serde_json::from_str(text).context("models.json is not valid JSON")?;
    let Some(providers) = document.get("providers").and_then(Value::as_object) else {
        return Ok(None);
    };
    let kept: Map<String, Value> = providers
        .iter()
        .filter_map(|(id, provider)| {
            let rewritten = rewrite_loopback(provider.get("baseUrl")?.as_str()?)?;
            let mut provider = provider.clone();
            provider["baseUrl"] = Value::String(rewritten);
            Some((id.clone(), provider))
        })
        .collect();
    if kept.is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({ "providers": kept })))
}

/// `http(s)://<loopback>[:port][/path]` with the host swapped for the alias; `None` otherwise.
/// Anything unusual is refused, so a URL like `http://localhost:1@cloud.example` (userinfo) can
/// never carry a cloud provider and its key into the VM.
fn rewrite_loopback(url: &str) -> Option<String> {
    if url
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ' || byte == b'\\')
    {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let (authority, path) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let (host, port) = split_host_port(authority);
    let port_ok = port.is_empty()
        || port.strip_prefix(':').is_some_and(|digits| {
            digits.bytes().all(|byte| byte.is_ascii_digit()) && digits.parse::<u16>().is_ok()
        });
    (port_ok
        && LOOPBACK_HOSTS
            .iter()
            .any(|loopback| loopback.eq_ignore_ascii_case(host)))
    .then(|| format!("{scheme}://{HOST_ALIAS}{port}{path}"))
}

/// Splits `host[:port]`, keeping the colon with the port. Bracketed IPv6 hosts are honoured.
fn split_host_port(authority: &str) -> (&str, &str) {
    if authority.starts_with('[') {
        return authority
            .find(']')
            .map_or((authority, ""), |end| authority.split_at(end + 1));
    }
    authority
        .rsplit_once(':')
        .map_or((authority, ""), |(host, _)| authority.split_at(host.len()))
}

/// Providers for the servers that answered `probe` with at least one model, in the same shape
/// as pi's own examples; `None` when none did.
pub(crate) fn probed_providers(probe: impl Fn(u16) -> Option<Vec<String>>) -> Option<Value> {
    let providers: Map<String, Value> = SERVERS
        .iter()
        .filter_map(|(id, port)| {
            let models = probe(*port).filter(|models| !models.is_empty())?;
            let models: Vec<Value> = models.iter().map(|id| json!({ "id": id })).collect();
            let provider = json!({
                "api": "openai-completions",
                // Local servers ignore the key, but pi needs one to list the models.
                "apiKey": id,
                "baseUrl": format!("http://{HOST_ALIAS}:{port}/v1"),
                "models": models,
            });
            Some(((*id).to_owned(), provider))
        })
        .collect();
    (!providers.is_empty()).then(|| json!({ "providers": providers }))
}

/// `GET /v1/models` on the host's loopback `port`. Any failure means "no server here".
fn probe(port: u16) -> Option<Vec<String>> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, PROBE_TIMEOUT).ok()?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT)).ok()?;
    stream.write_all(models_request(port).as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    parse_models_response(&raw).ok()
}

/// HTTP/1.0 so the server closes the connection after one unchunked response.
fn models_request(port: u16) -> String {
    format!("GET /v1/models HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
}

/// The `data[].id` values of an OpenAI-style `/v1/models` HTTP response.
pub(crate) fn parse_models_response(raw: &[u8]) -> Result<Vec<String>> {
    let text = std::str::from_utf8(raw).context("response is not UTF-8")?;
    let (head, body) = text
        .split_once("\r\n\r\n")
        .context("response has no header block")?;
    let mut lines = head.lines();
    let status = lines.next().context("response is empty")?;
    let code = status
        .split(' ')
        .nth(1)
        .with_context(|| format!("malformed status line {status:?}"))?;
    ensure!(code == "200", "server answered {status}");
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            bail!("chunked responses are not supported");
        }
    }
    let document: Value = serde_json::from_str(body).context("model list is not valid JSON")?;
    let data = document
        .get("data")
        .and_then(Value::as_array)
        .context("model list has no data array")?;
    Ok(data
        .iter()
        .filter_map(|model| model.get("id")?.as_str().map(str::to_owned))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_FILE: &str = r#"{
      "providers": {
        "ollama": {
          "api": "openai-completions",
          "apiKey": "ollama",
          "baseUrl": "http://127.0.0.1:11434/v1",
          "models": [{ "id": "qwen3.5:4b", "input": ["text", "image"], "reasoning": true }]
        },
        "lmstudio": {
          "api": "openai-completions",
          "apiKey": "lmstudio",
          "baseUrl": "http://localhost:1234/v1",
          "models": [{ "id": "fable", "contextWindow": 65536 }]
        },
        "openrouter": {
          "api": "openai-completions",
          "apiKey": "sk-or-real-key",
          "baseUrl": "https://openrouter.ai/api/v1",
          "models": [{ "id": "anthropic/claude" }]
        },
        "anthropic": { "baseUrl": "https://proxy.example.com/v1" }
      }
    }"#;

    #[test]
    fn loopback_providers_are_kept_and_rewritten() -> Result<()> {
        let value = local_providers(HOST_FILE)?.context("local providers expected")?;
        let providers = value["providers"].as_object().context("providers object")?;
        assert_eq!(
            providers.keys().collect::<Vec<_>>(),
            ["lmstudio", "ollama"],
            "serde_json maps are sorted by key"
        );
        assert_eq!(
            providers["ollama"]["baseUrl"],
            "http://host.microsandbox.internal:11434/v1"
        );
        assert_eq!(
            providers["lmstudio"]["baseUrl"],
            "http://host.microsandbox.internal:1234/v1"
        );
        // Everything else of a kept provider is untouched.
        assert_eq!(providers["ollama"]["apiKey"], "ollama");
        assert_eq!(providers["ollama"]["models"][0]["id"], "qwen3.5:4b");
        assert_eq!(providers["ollama"]["models"][0]["reasoning"], true);
        assert_eq!(providers["lmstudio"]["models"][0]["contextWindow"], 65536);
        Ok(())
    }

    #[test]
    fn remote_providers_and_their_keys_are_dropped() -> Result<()> {
        let value = local_providers(HOST_FILE)?.context("local providers expected")?;
        let text = serde_json::to_string(&value)?;
        assert!(!text.contains("openrouter"), "{text}");
        assert!(!text.contains("sk-or-real-key"), "{text}");
        assert!(!text.contains("anthropic"), "{text}");
        Ok(())
    }

    #[test]
    fn malformed_json_is_an_error_with_context() {
        let message = match local_providers("{ not json") {
            Ok(value) => format!("unexpectedly parsed {value:?}"),
            Err(error) => format!("{error:#}"),
        };
        assert!(message.contains("not valid JSON"), "{message}");
    }

    #[test]
    fn no_local_providers_means_none() -> Result<()> {
        assert_eq!(local_providers("{}")?, None);
        assert_eq!(local_providers(r#"{"providers": {}}"#)?, None);
        assert_eq!(
            local_providers(r#"{"providers": {"x": {"baseUrl": "https://x.example/v1"}}}"#)?,
            None
        );
        assert_eq!(
            local_providers(r#"{"providers": {"x": {"api": "openai-completions"}}}"#)?,
            None,
            "a provider without baseUrl is a built-in override, not a local server"
        );
        Ok(())
    }

    #[test]
    fn unusual_urls_are_never_rewritten() {
        for url in [
            "http://localhost:1234@cloud.example/v1",
            "http://127.0.0.1@cloud.example/v1",
            "http://user:pass@localhost/v1",
            "http://localhost:bad/v1",
            "http://localhost:99999/v1",
            "http://localhost\\@cloud.example/v1",
            "http://local host/v1",
            "file://localhost/etc/passwd",
            "http://cloud.example/localhost",
        ] {
            assert_eq!(rewrite_loopback(url), None, "{url}");
        }
        assert_eq!(
            rewrite_loopback("http://localhost:1234?x=1#y"),
            Some(format!("http://{HOST_ALIAS}:1234?x=1#y"))
        );
    }

    #[test]
    fn loopback_rewrite_keeps_scheme_port_and_path() {
        let alias = |rest: &str| Some(format!("http://{HOST_ALIAS}{rest}"));
        assert_eq!(
            rewrite_loopback("http://127.0.0.1:11434/v1"),
            alias(":11434/v1")
        );
        assert_eq!(rewrite_loopback("http://LocalHost:1234"), alias(":1234"));
        assert_eq!(
            rewrite_loopback("http://[::1]:8080/v1/"),
            alias(":8080/v1/")
        );
        assert_eq!(rewrite_loopback("http://0.0.0.0/v1"), alias("/v1"));
        assert_eq!(
            rewrite_loopback("https://localhost:1/v1"),
            Some(format!("https://{HOST_ALIAS}:1/v1"))
        );
        assert_eq!(
            rewrite_loopback("http://127.0.0.1.evil.example:1234/v1"),
            None
        );
        assert_eq!(rewrite_loopback("https://api.openai.com/v1"), None);
        assert_eq!(rewrite_loopback("localhost:1234"), None, "no scheme");
    }

    #[test]
    fn probed_servers_become_providers() -> Result<()> {
        let probe = |port: u16| match port {
            11434 => Some(vec!["qwen3.5:4b".to_owned()]),
            1234 => Some(vec![]),
            _ => None,
        };
        let value = probed_providers(probe).context("ollama answered")?;
        let providers = value["providers"].as_object().context("providers object")?;
        assert_eq!(providers.keys().collect::<Vec<_>>(), ["ollama"]);
        assert_eq!(providers["ollama"]["api"], "openai-completions");
        assert_eq!(providers["ollama"]["apiKey"], "ollama");
        assert_eq!(
            providers["ollama"]["baseUrl"],
            "http://host.microsandbox.internal:11434/v1"
        );
        assert_eq!(
            providers["ollama"]["models"],
            json!([{ "id": "qwen3.5:4b" }])
        );

        assert_eq!(probed_providers(|_| None), None);
        Ok(())
    }

    #[test]
    fn models_request_is_plain_http_1_0() {
        assert_eq!(
            models_request(1234),
            "GET /v1/models HTTP/1.0\r\nHost: 127.0.0.1:1234\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn model_ids_are_read_from_the_response_body() -> Result<()> {
        let raw = b"HTTP/1.1 200 OK\r\nX-Powered-By: Express\r\ncontent-type: application/json; charset=utf-8\r\nContent-Length: 70\r\n\r\n{\"data\":[{\"id\":\"fable\",\"object\":\"model\"},{\"id\":\"lfm3-1.7b-mlx\"},{\"object\":\"model\"}]}";
        assert_eq!(parse_models_response(raw)?, ["fable", "lfm3-1.7b-mlx"]);
        Ok(())
    }

    #[test]
    fn bad_responses_are_rejected_cleanly() {
        let cases: [(&[u8], &str); 6] = [
            (b"", "no header block"),
            (b"HTTP/1.1 404 Not Found\r\n\r\n{}", "answered HTTP/1.1 404"),
            (
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n0\r\n\r\n",
                "chunked",
            ),
            (
                b"HTTP/1.1 200 OK\r\ntransfer-encoding:  Chunked\r\n\r\n{}",
                "chunked",
            ),
            (b"HTTP/1.1 200 OK\r\n\r\n{\"data\": 1}", "no data array"),
            (b"HTTP/1.1 200 OK\r\n\r\n{\"data\": [", "not valid JSON"),
        ];
        for (raw, expected) in cases {
            let message = match parse_models_response(raw) {
                Ok(ids) => format!("unexpectedly parsed {ids:?}"),
                Err(error) => format!("{error:#}"),
            };
            assert!(message.contains(expected), "{raw:?}: {message}");
        }
    }

    #[test]
    fn host_models_path_prefers_the_agent_dir_override() {
        let home = Path::new("/Users/me");
        assert_eq!(
            host_models_path(None, Some(home)),
            Some(PathBuf::from("/Users/me/.pi/agent/models.json"))
        );
        assert_eq!(
            host_models_path(Some("/tmp/pi"), Some(home)),
            Some(PathBuf::from("/tmp/pi/models.json"))
        );
        assert_eq!(
            host_models_path(Some(""), Some(home)),
            Some(PathBuf::from("/Users/me/.pi/agent/models.json")),
            "pi ignores an empty override"
        );
        assert_eq!(host_models_path(None, None), None);
    }
}
