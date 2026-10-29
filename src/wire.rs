//! Byte-level parsers for the proxy: the HTTP `CONNECT` request from msb (RFC 9110 §9.3.6,
//! RFC 9112 §3), and the hostname hidden in the guest's first bytes, from a TLS `ClientHello`'s
//! `server_name` extension (RFC 8446 §4.1.2, RFC 6066 §3) or an HTTP/1.x `Host` header
//! (RFC 9110 §7.2). Pure functions over byte slices, so they are tested on fixtures.

use std::net::SocketAddr;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;

/// Longest request head (through the blank line) the proxy accepts.
pub(crate) const MAX_HEAD: usize = 8 * 1024;

/// Path of the status request only an msbx proxy answers.
pub(crate) const STATUS_PATH: &str = "/msbx-status";
/// Path of the authenticated stop request.
pub(crate) const STOP_PATH: &str = "/msbx-stop";
/// Path of the authenticated request that makes the proxy re-read `policy.toml` now.
pub(crate) const RELOAD_PATH: &str = "/msbx-reload";

/// `Proxy-Authorization: Basic` credentials, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Basic {
    pub(crate) user: String,
    pub(crate) password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    /// `CONNECT ip:port`. msb always sends a socket address, never a name.
    Connect {
        target: SocketAddr,
        auth: Option<Basic>,
    },
    /// `GET /msbx-status`.
    Status,
    /// `POST /msbx-stop` with the proxy token as the Basic password.
    Stop {
        auth: Option<Basic>,
    },
    /// `POST /msbx-reload` with the proxy token as the Basic password.
    Reload {
        auth: Option<Basic>,
    },
    Other,
}

/// Index just past the `\r\n\r\n` that ends a request head, if it is in `bytes`.
pub(crate) fn head_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|start| start + 4)
}

/// Parses a complete request head. Lines may end in `\n` as well as `\r\n` (RFC 9112 §2.2).
pub(crate) fn parse_request(head: &[u8]) -> Result<Request> {
    let text = std::str::from_utf8(head).context("request head is not UTF-8")?;
    let mut lines = text.split('\n').map(|line| line.trim_end_matches('\r'));
    let request_line = lines
        .by_ref()
        .find(|line| !line.is_empty())
        .context("empty request")?;
    let mut parts = request_line.split(' ').filter(|part| !part.is_empty());
    let (method, target, version) = (parts.next(), parts.next(), parts.next());
    let (Some(method), Some(target), Some(version)) = (method, target, version) else {
        bail!("malformed request line {request_line:?}");
    };
    ensure!(
        version.starts_with("HTTP/1."),
        "unsupported version {version:?}"
    );
    let auth = proxy_authorization(lines.clone()).transpose()?;
    Ok(match (method, target) {
        ("CONNECT", authority) => Request::Connect {
            target: authority
                .parse()
                .with_context(|| format!("CONNECT target {authority:?} is not ip:port"))?,
            auth,
        },
        ("GET", STATUS_PATH) => Request::Status,
        ("POST", STOP_PATH) => Request::Stop { auth },
        ("POST", RELOAD_PATH) => Request::Reload { auth },
        _ => Request::Other,
    })
}

fn proxy_authorization<'a>(lines: impl Iterator<Item = &'a str>) -> Option<Result<Basic>> {
    let value = lines
        .take_while(|line| !line.is_empty())
        .find_map(|line| header_value(line, "proxy-authorization"))?;
    let Some(encoded) = value
        .split_once(' ')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("basic"))
        .map(|(_, rest)| rest.trim())
    else {
        return Some(Err(anyhow::anyhow!(
            "only Basic proxy authentication is supported"
        )));
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(bytes) => bytes,
        Err(error) => return Some(Err(error).context("decoding Proxy-Authorization")),
    };
    let Ok(text) = String::from_utf8(decoded) else {
        return Some(Err(anyhow::anyhow!("Proxy-Authorization is not UTF-8")));
    };
    let (user, password) = text.split_once(':').unwrap_or((&text, ""));
    Some(Ok(Basic {
        user: user.to_owned(),
        password: password.to_owned(),
    }))
}

/// The value of header `name` (ASCII case-insensitive) if `line` is that header.
fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
}

/// What a peek at the guest's first bytes found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sniff {
    Found(String),
    /// Not TLS and not HTTP, or they carry no name: decide on the IP.
    None,
    /// The name may still come with more bytes.
    NeedMore,
}

/// The destination name in a TLS `ClientHello` or an HTTP/1.x request.
pub(crate) fn hostname(bytes: &[u8]) -> Sniff {
    if bytes.is_empty() {
        return Sniff::NeedMore;
    }
    if bytes[0] == 0x16 {
        return sni(bytes);
    }
    http_host(bytes)
}

/// The `server_name` host from a TLS `ClientHello`, which may span several handshake records.
pub(crate) fn sni(bytes: &[u8]) -> Sniff {
    // Reassemble the handshake message from consecutive handshake records (RFC 8446 §5.1).
    let mut handshake = Vec::new();
    let mut rest = bytes;
    loop {
        if handshake.len() >= 4 {
            let wanted = 4 + be24(&handshake[1..4]);
            if handshake.len() >= wanted {
                handshake.truncate(wanted);
                break;
            }
        }
        if rest.len() < 5 {
            return Sniff::NeedMore;
        }
        if rest[0] != 0x16 {
            return Sniff::None;
        }
        let length = usize::from(u16::from_be_bytes([rest[3], rest[4]]));
        if length == 0 || length > 1 << 14 {
            return Sniff::None;
        }
        let Some(fragment) = rest.get(5..5 + length) else {
            // A partial final record still tells us whether more could help.
            handshake.extend_from_slice(&rest[5..]);
            return if handshake.first().is_some_and(|kind| *kind != 0x01) {
                Sniff::None
            } else {
                Sniff::NeedMore
            };
        };
        handshake.extend_from_slice(fragment);
        rest = &rest[5 + length..];
    }
    client_hello_sni(&handshake)
}

fn be24(bytes: &[u8]) -> usize {
    usize::from(bytes[0]) << 16 | usize::from(bytes[1]) << 8 | usize::from(bytes[2])
}

/// Walks a complete `ClientHello` (RFC 8446 §4.1.2) to its `server_name` extension (RFC 6066 §3).
fn client_hello_sni(message: &[u8]) -> Sniff {
    if message.first() != Some(&0x01) {
        return Sniff::None;
    }
    let mut cursor = Cursor(&message[4..]);
    let parsed = (|| {
        cursor.skip(2 + 32)?; // legacy_version, random
        let session = cursor.u8()?;
        cursor.skip(session)?;
        let suites = cursor.u16()?;
        cursor.skip(suites)?;
        let compression = cursor.u8()?;
        cursor.skip(compression)?;
        let extensions_length = cursor.u16()?;
        let mut extensions = Cursor(cursor.take(extensions_length)?);
        while !extensions.0.is_empty() {
            let kind = extensions.u16()?;
            let length = extensions.u16()?;
            let data = extensions.take(length)?;
            if kind == 0 {
                return server_name(data);
            }
        }
        None
    })();
    parsed.map_or(Sniff::None, Sniff::Found)
}

/// The `host_name` entry of a `ServerNameList`; other name types are ignored.
fn server_name(data: &[u8]) -> Option<String> {
    let mut outer = Cursor(data);
    let list_length = outer.u16()?;
    let mut list = Cursor(outer.take(list_length)?);
    while !list.0.is_empty() {
        let name_type = list.u8()?;
        let length = list.u16()?;
        let name = list.take(length)?;
        if name_type == 0 {
            return clean_host(std::str::from_utf8(name).ok()?);
        }
    }
    None
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(count)?;
        self.0 = tail;
        Some(head)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        self.take(count).map(|_| ())
    }

    fn u8(&mut self) -> Option<usize> {
        self.take(1).map(|bytes| usize::from(bytes[0]))
    }

    fn u16(&mut self) -> Option<usize> {
        self.take(2)
            .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
    }
}

/// The `Host` header of an HTTP/1.x request, without its port.
pub(crate) fn http_host(bytes: &[u8]) -> Sniff {
    let Some(line_end) = bytes.iter().position(|byte| *byte == b'\n') else {
        return if bytes.len() > MAX_HEAD || !looks_like_request_start(bytes) {
            Sniff::None
        } else {
            Sniff::NeedMore
        };
    };
    let Ok(request_line) = std::str::from_utf8(&bytes[..line_end]) else {
        return Sniff::None;
    };
    if !is_request_line(request_line.trim_end_matches('\r')) {
        return Sniff::None;
    }
    let Ok(text) = std::str::from_utf8(&bytes[line_end + 1..]) else {
        // Headers are ASCII; a UTF-8 cut at the end only matters for the last partial line.
        return Sniff::NeedMore;
    };
    let mut complete = false;
    for line in text.split_inclusive('\n') {
        let Some(line) = line.strip_suffix('\n') else {
            break; // an unterminated last line may still grow
        };
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            complete = true;
            break;
        }
        if let Some(value) = header_value(line, "host") {
            return match clean_host(strip_port(value)) {
                Some(host) => Sniff::Found(host),
                None => Sniff::None,
            };
        }
    }
    if complete || bytes.len() > MAX_HEAD {
        Sniff::None
    } else {
        Sniff::NeedMore
    }
}

/// `METHOD SP target SP HTTP/1.x` (RFC 9112 §3).
fn is_request_line(line: &str) -> bool {
    let mut parts = line.split(' ');
    let method_ok = parts
        .next()
        .is_some_and(|method| !method.is_empty() && method.bytes().all(|b| b.is_ascii_uppercase()));
    let target_ok = parts.next().is_some_and(|target| !target.is_empty());
    let version_ok = parts
        .next()
        .is_some_and(|version| version.starts_with("HTTP/1."));
    method_ok && target_ok && version_ok && parts.next().is_none()
}

/// Whether a partial first line could still become a request line.
fn looks_like_request_start(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .take_while(|byte| **byte != b' ')
        .all(u8::is_ascii_uppercase)
}

/// `example.com:443` → `example.com`; `[::1]:80` → `[::1]`.
fn strip_port(value: &str) -> &str {
    if value.starts_with('[') {
        return value.find(']').map_or(value, |end| &value[..=end]);
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => value,
    }
}

/// Lowercase, no trailing dot, no empty or control-character names.
fn clean_host(name: &str) -> Option<String> {
    let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty() || name.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `curl --resolve example.com:45678:127.0.0.1 https://example.com:45678/` (curl 8.7.1,
    /// `SecureTransport` on macOS), captured with `nc -l`: one 321-byte handshake record.
    const CURL_HELLO: &[u8] = include_bytes!("fixtures/curl-client-hello.bin");

    fn basic(user: &str, password: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    }

    #[test]
    fn head_end_finds_the_blank_line() {
        assert_eq!(head_end(b"GET / HTTP/1.1\r\n\r\nbody"), Some(18));
        assert_eq!(head_end(b"GET / HTTP/1.1\r\nHost: x\r\n"), None);
        assert_eq!(head_end(b""), None);
    }

    #[test]
    fn connect_with_basic_auth_parses_like_msb_sends_it() -> Result<()> {
        // microsandbox-network 0.7.11 proxy/http_connect.rs `build_connect_request`.
        let head = format!(
            "CONNECT 93.184.216.34:443 HTTP/1.1\r\nHost: 93.184.216.34:443\r\n\
             Proxy-Authorization: Basic {}\r\n\r\n",
            basic("codex-api", "tok:en")
        );
        assert_eq!(
            parse_request(head.as_bytes())?,
            Request::Connect {
                target: "93.184.216.34:443".parse()?,
                auth: Some(Basic {
                    user: "codex-api".to_owned(),
                    password: "tok:en".to_owned(),
                }),
            }
        );
        let v6 = "CONNECT [2606:2800:220:1:248:1893:25c8:1946]:443 HTTP/1.1\r\n\r\n";
        assert!(matches!(
            parse_request(v6.as_bytes())?,
            Request::Connect { auth: None, .. }
        ));
        Ok(())
    }

    #[test]
    fn connect_without_or_with_bad_auth() -> Result<()> {
        let none = parse_request(b"CONNECT 1.2.3.4:80 HTTP/1.1\r\nHost: 1.2.3.4:80\r\n\r\n")?;
        assert_eq!(
            none,
            Request::Connect {
                target: "1.2.3.4:80".parse()?,
                auth: None
            }
        );
        let digest = b"CONNECT 1.2.3.4:80 HTTP/1.1\r\nProxy-Authorization: Digest abc\r\n\r\n";
        assert!(parse_request(digest).is_err());
        let garbage = b"CONNECT 1.2.3.4:80 HTTP/1.1\r\nproxy-authorization: basic !!!\r\n\r\n";
        assert!(parse_request(garbage).is_err());
        let no_colon = format!(
            "CONNECT 1.2.3.4:80 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
            base64::engine::general_purpose::STANDARD.encode("justuser")
        );
        assert_eq!(
            parse_request(no_colon.as_bytes())?,
            Request::Connect {
                target: "1.2.3.4:80".parse()?,
                auth: Some(Basic {
                    user: "justuser".to_owned(),
                    password: String::new()
                })
            }
        );
        Ok(())
    }

    #[test]
    fn connect_needs_a_socket_address() {
        assert!(parse_request(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_request(b"CONNECT 1.2.3.4 HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_request(b"CONNECT 1.2.3.4:443 HTTP/2\r\n\r\n").is_err());
        assert!(parse_request(b"CONNECT\r\n\r\n").is_err());
        assert!(parse_request(b"\xff\xfe\r\n\r\n").is_err());
    }

    #[test]
    fn status_stop_and_other_requests() -> Result<()> {
        assert_eq!(
            parse_request(b"\r\nGET /msbx-status HTTP/1.1\nHost: 127.0.0.1\n\n")?,
            Request::Status,
            "a leading empty line and bare LF are accepted"
        );
        let stop = format!(
            "POST /msbx-stop HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
            basic("", "secret")
        );
        assert_eq!(
            parse_request(stop.as_bytes())?,
            Request::Stop {
                auth: Some(Basic {
                    user: String::new(),
                    password: "secret".to_owned()
                })
            }
        );
        let reload = format!(
            "POST /msbx-reload HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
            basic("", "secret")
        );
        assert_eq!(
            parse_request(reload.as_bytes())?,
            Request::Reload {
                auth: Some(Basic {
                    user: String::new(),
                    password: "secret".to_owned()
                })
            }
        );
        assert_eq!(
            parse_request(b"POST /msbx-reload HTTP/1.1\r\n\r\n")?,
            Request::Reload { auth: None }
        );
        assert_eq!(parse_request(b"GET / HTTP/1.1\r\n\r\n")?, Request::Other);
        assert_eq!(
            parse_request(b"GET /msbx-stop HTTP/1.1\r\n\r\n")?,
            Request::Other
        );
        Ok(())
    }

    #[test]
    fn sni_from_a_real_curl_client_hello() {
        assert_eq!(CURL_HELLO.len(), 321);
        assert_eq!(sni(CURL_HELLO), Sniff::Found("example.com".to_owned()));
        assert_eq!(hostname(CURL_HELLO), Sniff::Found("example.com".to_owned()));
    }

    #[test]
    fn sni_needs_the_whole_hello_and_handles_split_records() {
        for cut in [0, 1, 4, 5, 6, 44, 100, 320] {
            assert_eq!(sni(&CURL_HELLO[..cut]), Sniff::NeedMore, "cut at {cut}");
        }
        // The same handshake split over two TLS records at byte 100 of the message.
        let message = &CURL_HELLO[5..];
        let (first, second) = message.split_at(100);
        let mut split = Vec::new();
        for fragment in [first, second] {
            split.extend_from_slice(&[0x16, 0x03, 0x01]);
            split
                .extend_from_slice(&u16::try_from(fragment.len()).map_or([0, 0], u16::to_be_bytes));
            split.extend_from_slice(fragment);
        }
        assert_eq!(sni(&split), Sniff::Found("example.com".to_owned()));
        assert_eq!(sni(&split[..110]), Sniff::NeedMore, "first record only");
        assert_eq!(sni(&split[..120]), Sniff::NeedMore, "second record partial");
        // Trailing bytes after the hello (an early-data record) do not matter.
        let mut extra = CURL_HELLO.to_vec();
        extra.extend_from_slice(&[0x17, 0x03, 0x03, 0x00, 0x01, 0x00]);
        assert_eq!(sni(&extra), Sniff::Found("example.com".to_owned()));
    }

    /// A `ClientHello` without extensions, built by hand from RFC 8446 §4.1.2.
    fn hello_without_sni() -> Vec<u8> {
        let mut body = vec![0x03, 0x03];
        body.extend_from_slice(&[0u8; 32]);
        body.push(0); // session id
        body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]); // one cipher suite
        body.extend_from_slice(&[0x01, 0x00]); // null compression
        body.extend_from_slice(&[0x00, 0x00]); // no extensions
        let mut handshake = vec![0x01, 0x00, 0x00, u8::try_from(body.len()).unwrap_or(0)];
        handshake.extend_from_slice(&body);
        let mut record = vec![
            0x16,
            0x03,
            0x01,
            0x00,
            u8::try_from(handshake.len()).unwrap_or(0),
        ];
        record.extend_from_slice(&handshake);
        record
    }

    #[test]
    fn hellos_without_a_name_and_garbage_give_none() {
        assert_eq!(sni(&hello_without_sni()), Sniff::None);
        assert_eq!(sni(b"\x16\x03\x01\x00\x00"), Sniff::None, "empty record");
        assert_eq!(
            sni(b"\x16\x03\x01\x00\x05\x02\x00\x00\x01\x00"),
            Sniff::None,
            "ServerHello"
        );
        assert_eq!(
            sni(b"\x16\x03\x01\xff\xff"),
            Sniff::None,
            "oversized record"
        );
        assert_eq!(
            sni(b"\x16\x03\x01\x00\x10\x02\x00"),
            Sniff::None,
            "partial non-hello"
        );
        assert_eq!(
            sni(b"\x15\x03\x01\x00\x02\x02\x28"),
            Sniff::None,
            "alert record"
        );
        assert_eq!(hostname(b"SSH-2.0-OpenSSH_9.6\r\n"), Sniff::None);
        assert_eq!(hostname(b"\x00\x01\x02\x03"), Sniff::None);
        assert_eq!(hostname(b""), Sniff::NeedMore);
        // A hello whose SNI entry is not host_name (type 1) is ignored.
        let mut hello = hello_without_sni();
        let ext = [
            0x00, 0x00, 0x00, 0x08, 0x00, 0x06, 0x01, 0x00, 0x03, b'a', b'b', b'c',
        ];
        let body_start = 9;
        hello.truncate(hello.len() - 2);
        hello.extend_from_slice(&[0x00, 0x0c]);
        hello.extend_from_slice(&ext);
        let body_len = u8::try_from(hello.len() - body_start).unwrap_or(0);
        hello[8] = body_len;
        hello[4] = body_len + 4;
        assert_eq!(sni(&hello), Sniff::None);
    }

    #[test]
    fn http_host_with_and_without_port() {
        let found = |text: &str| http_host(text.as_bytes());
        assert_eq!(
            found("GET / HTTP/1.1\r\nHost: Example.COM:8080\r\n\r\n"),
            Sniff::Found("example.com".to_owned())
        );
        assert_eq!(
            found("POST /x HTTP/1.0\r\nhost:   pypi.org.  \r\nAccept: */*\r\n\r\n"),
            Sniff::Found("pypi.org".to_owned())
        );
        assert_eq!(
            found("GET / HTTP/1.1\r\nHost: [2001:db8::1]:80\r\n\r\n"),
            Sniff::Found("[2001:db8::1]".to_owned())
        );
        assert_eq!(
            found("GET / HTTP/1.1\nHost: lf.test\n\n"),
            Sniff::Found("lf.test".to_owned())
        );
        assert_eq!(
            found("GET / HTTP/1.1\r\nAccept: */*\r\nHost: late.test\r\n"),
            Sniff::Found("late.test".to_owned()),
            "found before the head is complete"
        );
    }

    #[test]
    fn http_host_absent_partial_and_oversized() {
        let host = |bytes: &[u8]| http_host(bytes);
        assert_eq!(host(b"GET / HTTP/1.1\r\nAccept: */*\r\n\r\n"), Sniff::None);
        assert_eq!(host(b"GET / HTTP/1.1\r\nHost: \r\n\r\n"), Sniff::None);
        assert_eq!(host(b"GET / HTTP/1.1\r\nAccept: */*\r\n"), Sniff::NeedMore);
        assert_eq!(host(b"GET / HTTP/1"), Sniff::NeedMore);
        assert_eq!(host(b"GE"), Sniff::NeedMore);
        assert_eq!(host(b"get / HTTP/1.1\r\n"), Sniff::None);
        assert_eq!(host(b"GET / HTTP/1.1 extra\r\n"), Sniff::None);
        assert_eq!(host(b"PRI * HTTP/2.0\r\n"), Sniff::None, "HTTP/2 preface");
        let mut oversized = b"GET / HTTP/1.1\r\n".to_vec();
        oversized.extend(std::iter::repeat_n(b'x', MAX_HEAD + 1));
        assert_eq!(host(&oversized), Sniff::None);
        let mut long_first_line = b"GET /".to_vec();
        long_first_line.extend(std::iter::repeat_n(b'a', MAX_HEAD + 1));
        assert_eq!(host(&long_first_line), Sniff::None);
    }
}
