//! Fork: whether Remote Control connects a Claude agent to claude.ai, for its pane's
//! `$remote_control` token.
//!
//! Claude Code keeps a record of each running Claude in `sessions/<pid>.json` in its folder
//! (`~/.claude`, or the Herdr server's `CLAUDE_CONFIG_DIR`; a Claude started with a different one
//! keeps its record elsewhere and gets no cloud). While Remote Control is connected, the record's
//! `bridgeSessionId` holds the session's id, and it goes back to null when Remote Control
//! disconnects (Claude Code 2.1.319; the record isn't documented). Claude shows nothing of it on
//! screen and runs no hook for it, so each pane's detection loop reads its Claude agent's record,
//! found by the agent's process id, every CHECK_INTERVAL. A record that's missing or can't be
//! read counts as not connected.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::detect::Agent;

/// The pane token, `$remote_control` in sidebar rows.
pub const REMOTE_CONTROL_TOKEN: &str = "remote_control";
/// The token's text while connected: Material Design's cloud (U+F015F) from the Nerd Font
/// symbols, as Claude's status line can show it. A row's rule can put other text in its place.
const CLOUD: &str = "\u{F015F}";
/// How often a Claude agent's record is read.
const CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// A record is a few hundred bytes; a bigger file isn't one, and isn't read.
const MAX_RECORD_BYTES: u64 = 64 << 10;

#[derive(Deserialize)]
struct SessionRecord {
    pid: Option<u32>,
    #[serde(rename = "bridgeSessionId")]
    bridge_session_id: Option<String>,
}

/// Whether Remote Control is connected to the Claude whose process is `pid`.
pub fn connected(pid: u32) -> bool {
    crate::integration::claude_dir()
        .is_ok_and(|claude_dir| connected_in(&claude_dir.join("sessions"), pid))
}

fn connected_in(sessions: &Path, pid: u32) -> bool {
    let path = sessions.join(format!("{pid}.json"));
    // Only a regular file (a symlink to one is followed), as opening a FIFO would block the
    // pane's detection loop.
    if !std::fs::metadata(&path).is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_RECORD_BYTES)
    {
        return false;
    }
    let Ok(file) = std::fs::File::open(&path) else {
        return false;
    };
    // Capped too, in case the file grew since.
    let mut text = Vec::new();
    if file
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut text)
        .is_err()
        || text.len() as u64 > MAX_RECORD_BYTES
    {
        return false;
    }
    let Ok(record) = serde_json::from_slice::<SessionRecord>(&text) else {
        return false;
    };
    record.pid == Some(pid)
        && record
            .bridge_session_id
            .is_some_and(|id| !id.trim().is_empty())
}

/// The token's text: the cloud while connected, none otherwise.
pub fn token_text(connected: bool) -> Option<String> {
    connected.then(|| CLOUD.to_string())
}

/// A pane's checks: which process was read last, when, and what it said.
#[derive(Debug, Default)]
pub struct Tracker {
    pid: Option<u32>,
    checked_at: Option<Instant>,
    connected: Option<bool>,
}

impl Tracker {
    /// The new answer when it changed, for the pane's `agent` and its process `agent_pid`
    /// (None once it has exited). Only a Claude agent can be connected. `connected` reads the
    /// record: at once for a different process, then at most every CHECK_INTERVAL.
    pub fn observe(
        &mut self,
        agent: Option<Agent>,
        agent_pid: Option<u32>,
        now: Instant,
        connected: impl FnOnce(u32) -> bool,
    ) -> Option<bool> {
        let pid = agent_pid.filter(|_| agent == Some(Agent::Claude));
        let due = self.pid != pid
            || self
                .checked_at
                .is_none_or(|at| now.saturating_duration_since(at) >= CHECK_INTERVAL);
        if !due {
            return None;
        }
        self.pid = pid;
        self.checked_at = Some(now);
        let connected = pid.is_some_and(connected);
        if self.connected == Some(connected) {
            return None;
        }
        self.connected = Some(connected);
        Some(connected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "herdr-remote-control-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_record_with_a_bridge_session_is_connected() {
        let sessions = test_dir("records");
        let write = |pid: u32, json: &str| fs::write(sessions.join(format!("{pid}.json")), json);
        write(
            101,
            r#"{"pid":101,"sessionId":"a","bridgeSessionId":"session_01","status":"idle"}"#,
        )
        .unwrap();
        write(102, r#"{"pid":102,"sessionId":"b","bridgeSessionId":null}"#).unwrap();
        write(103, r#"{"pid":103,"sessionId":"c"}"#).unwrap();
        write(104, r#"{"pid":104,"bridgeSessionId":""}"#).unwrap();
        // Another process's record under this one's name.
        write(105, r#"{"pid":999,"bridgeSessionId":"session_05"}"#).unwrap();
        write(106, r#"{"pid":106,"bridgeSessionId":"session_06""#).unwrap();
        write(107, r#"{"pid":107,"bridgeSessionId":{"id":"session_07"}}"#).unwrap();
        // 108 has no record at all.
        // Not a regular file: a folder (a FIFO is turned away the same way, before it's opened).
        fs::create_dir(sessions.join("109.json")).unwrap();
        // Too big to be a record, though it says connected.
        let padding = " ".repeat(MAX_RECORD_BYTES as usize);
        write(
            110,
            &format!(r#"{{"pid":110,"bridgeSessionId":"session_10"}}{padding}"#),
        )
        .unwrap();

        assert!(connected_in(&sessions, 101));
        for pid in 102..=110 {
            assert!(!connected_in(&sessions, pid), "pid {pid}");
        }
        fs::remove_dir_all(sessions).unwrap();
    }

    #[test]
    fn the_token_is_the_cloud_while_connected() {
        assert_eq!(token_text(true).as_deref(), Some("\u{F015F}"));
        assert_eq!(token_text(false), None);
    }

    #[test]
    fn the_tracker_reads_a_new_process_at_once_and_then_every_interval() {
        let start = Instant::now();
        let claude = Some(Agent::Claude);
        let mut tracker = Tracker::default();
        let mut reads = Vec::new();
        let mut observe = |tracker: &mut Tracker, pid, after_ms, answer| {
            let now = start + Duration::from_millis(after_ms);
            tracker.observe(claude, pid, now, |pid| {
                reads.push(pid);
                answer
            })
        };

        assert_eq!(observe(&mut tracker, Some(7), 0, true), Some(true));
        // Not due yet: nothing is read.
        assert_eq!(observe(&mut tracker, Some(7), 1_000, false), None);
        // Due, and still connected: read, but no change to tell.
        assert_eq!(observe(&mut tracker, Some(7), 2_000, true), None);
        assert_eq!(observe(&mut tracker, Some(7), 4_000, false), Some(false));
        // A different process is read at once.
        assert_eq!(observe(&mut tracker, Some(8), 4_300, true), Some(true));
        // Gone: not connected without a read.
        assert_eq!(observe(&mut tracker, None, 4_600, true), Some(false));
        assert_eq!(reads, [7, 7, 7, 8]);
    }

    #[test]
    fn only_a_claude_agent_is_read() {
        let mut tracker = Tracker::default();
        let now = Instant::now();
        let answer = tracker.observe(Some(Agent::Codex), Some(7), now, |_| {
            panic!("read for another agent")
        });
        assert_eq!(answer, Some(false));
        assert_eq!(
            tracker.observe(None, Some(7), now + CHECK_INTERVAL, |_| panic!(
                "read without an agent"
            )),
            None
        );
    }
}
