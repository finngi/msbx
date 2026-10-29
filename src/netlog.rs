//! The connection log the proxy keeps: one row per (sandbox, host, port, decision, rule) with a
//! count and first/last seen times, like `sbx policy log`. Pure except `load`/`save`.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Rows kept at most; the oldest by `last_seen` go first.
pub(crate) const CAP: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) sandbox: String,
    /// The name the guest asked for, or the IP when none was seen.
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) allowed: bool,
    /// The deciding rule, e.g. `allow pypi.org (global)` or `default`.
    pub(crate) rule: String,
    pub(crate) count: u64,
    /// Unix seconds.
    pub(crate) first_seen: u64,
    pub(crate) last_seen: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Log {
    entries: Vec<Entry>,
    dirty: bool,
}

impl Log {
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Unsaved changes since the last `save`/`load`.
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Counts one connection, merging it into the matching row or adding one.
    pub(crate) fn record(
        &mut self,
        sandbox: &str,
        host: &str,
        port: u16,
        allowed: bool,
        rule: &str,
        now: u64,
    ) {
        self.dirty = true;
        let existing = self.entries.iter_mut().find(|entry| {
            entry.sandbox == sandbox
                && entry.host == host
                && entry.port == port
                && entry.allowed == allowed
                && entry.rule == rule
        });
        if let Some(entry) = existing {
            entry.count += 1;
            entry.last_seen = entry.last_seen.max(now);
            return;
        }
        self.entries.push(Entry {
            sandbox: sandbox.to_owned(),
            host: host.to_owned(),
            port,
            allowed,
            rule: rule.to_owned(),
            count: 1,
            first_seen: now,
            last_seen: now,
        });
        while self.entries.len() > CAP {
            if let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(index, _)| index)
            {
                self.entries.remove(oldest);
            }
        }
    }

    pub(crate) fn to_json(&self) -> Result<String> {
        serde_json::to_string(&self.entries).context("serialising the network log")
    }

    pub(crate) fn from_json(text: &str) -> Result<Self> {
        let entries = serde_json::from_str(text).context("parsing the network log")?;
        Ok(Self {
            entries,
            dirty: false,
        })
    }

    /// The log file, or an empty log when there is none.
    pub(crate) fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                Self::from_json(&text).with_context(|| format!("reading {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Writes the log atomically and clears the dirty flag.
    pub(crate) fn save(&mut self, path: &Path) -> Result<()> {
        crate::paths::write_private(path, self.to_json()?.as_bytes())?;
        self.dirty = false;
        Ok(())
    }
}

/// Unix seconds as local wall-clock time in a chrono `pattern`, e.g. `%H:%M:%S`.
pub(crate) fn local_time(secs: u64, pattern: &str) -> String {
    i64::try_from(secs)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map_or_else(
            || "—".to_owned(),
            |time| {
                time.with_timezone(&chrono::Local)
                    .format(pattern)
                    .to_string()
            },
        )
}

/// Rows for display: optionally one sandbox's, blocked before allowed, newest first.
pub(crate) fn sorted<'a>(entries: &'a [Entry], sandbox: Option<&str>) -> Vec<&'a Entry> {
    let mut rows: Vec<&Entry> = entries
        .iter()
        .filter(|entry| sandbox.is_none_or(|name| entry.sandbox == name))
        .collect();
    rows.sort_by(|a, b| {
        a.allowed
            .cmp(&b.allowed)
            .then(b.last_seen.cmp(&a.last_seen))
            .then(a.host.cmp(&b.host))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_aggregates_identical_connections() {
        let mut log = Log::default();
        assert!(!log.is_dirty());
        log.record("box", "pypi.org", 443, false, "default", 100);
        log.record("box", "pypi.org", 443, false, "default", 90);
        log.record("box", "pypi.org", 443, false, "default", 120);
        log.record("box", "pypi.org", 80, false, "default", 130);
        log.record("other", "pypi.org", 443, false, "default", 130);
        log.record("box", "pypi.org", 443, true, "allow pypi.org (global)", 140);
        assert!(log.is_dirty());
        assert_eq!(log.entries().len(), 4);
        let first = &log.entries()[0];
        assert_eq!(
            (first.count, first.first_seen, first.last_seen),
            (3, 100, 120)
        );
    }

    #[test]
    fn cap_drops_the_oldest_rows() {
        let mut log = Log::default();
        for index in 0..CAP + 3 {
            let now = u64::try_from(index).unwrap_or(0);
            log.record("box", &format!("h{index}.test"), 443, true, "default", now);
        }
        assert_eq!(log.entries().len(), CAP);
        assert!(log.entries().iter().all(|entry| entry.last_seen >= 3));
    }

    #[test]
    fn sorted_puts_blocked_first_then_newest() {
        let mut log = Log::default();
        log.record("box", "old-allowed.test", 443, true, "r", 10);
        log.record("box", "new-blocked.test", 443, false, "default", 50);
        log.record("box", "new-allowed.test", 443, true, "r", 40);
        log.record("box", "old-blocked.test", 443, false, "default", 20);
        log.record("other", "elsewhere.test", 443, false, "default", 99);
        let hosts: Vec<&str> = sorted(log.entries(), Some("box"))
            .iter()
            .map(|entry| entry.host.as_str())
            .collect();
        assert_eq!(
            hosts,
            [
                "new-blocked.test",
                "old-blocked.test",
                "new-allowed.test",
                "old-allowed.test"
            ]
        );
        assert_eq!(sorted(log.entries(), None).len(), 5);
        assert_eq!(sorted(log.entries(), None)[0].host, "elsewhere.test");
    }

    #[test]
    fn json_round_trips() -> Result<()> {
        let mut log = Log::default();
        log.record("box", "pypi.org", 443, false, "default", 100);
        let text = log.to_json()?;
        assert!(text.contains("\"sandbox\":\"box\""), "{text}");
        let back = Log::from_json(&text)?;
        assert_eq!(back.entries(), log.entries());
        assert!(!back.is_dirty());
        assert!(Log::from_json("{").is_err());
        assert_eq!(Log::from_json("[]")?, Log::default());
        Ok(())
    }

    #[test]
    fn load_and_save_round_trip_through_the_file() -> Result<()> {
        let root = std::env::temp_dir().join(format!("msbx-netlog-{}", std::process::id()));
        let path = root.join("network-log.json");
        let outcome = (|| -> Result<()> {
            assert_eq!(Log::load(&path)?, Log::default());
            let mut log = Log::default();
            log.record("box", "pypi.org", 443, true, "allow pypi.org (global)", 100);
            log.save(&path)?;
            assert!(!log.is_dirty());
            assert_eq!(Log::load(&path)?.entries(), log.entries());
            std::fs::write(&path, "not json")?;
            assert!(Log::load(&path).is_err());
            Ok(())
        })();
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        outcome
    }
}
