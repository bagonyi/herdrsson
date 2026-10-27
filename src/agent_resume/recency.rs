//! Herdrsson: restored agents start in order of their conversations' last
//! message, latest first, so the agents in use come back before the rest.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::{AgentSessionRefKind, PersistedAgentSession};

/// How far from the end of a conversation to look for its last message.
const MAX_TAIL_BYTES: u64 = 4 << 20;

/// When the agent's conversation last had a message, as a timestamp that
/// sorts in time order, or None when Herdr can't tell (only Claude's
/// conversations are read).
pub fn last_message_stamp(session: &PersistedAgentSession) -> Option<String> {
    let projects = crate::integration::claude_dir().ok()?.join("projects");
    last_message_stamp_in(&projects, session)
}

fn last_message_stamp_in(projects: &Path, session: &PersistedAgentSession) -> Option<String> {
    if session.agent != "claude" || session.session_ref.kind != AgentSessionRefKind::Id {
        return None;
    }
    let path = claude_transcript(projects, &session.session_ref.value)?;
    last_message_stamp_in_file(&path)
}

/// Claude keeps a conversation in `projects/<folder>/<session id>.jsonl`, the
/// folder named after the directory it started in, which needn't be the pane's.
fn claude_transcript(projects: &Path, session_id: &str) -> Option<PathBuf> {
    let file = format!("{session_id}.jsonl");
    let mut components = Path::new(&file).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return None;
    }
    fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(&file))
        .find(|path| path.is_file())
}

/// Reads from the end, as a conversation can be many megabytes. Claude adds
/// entries without a time when it resumes a conversation, which is why the
/// file's modification time won't do.
fn last_message_stamp_in_file(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let mut size = 1 << 16;
    loop {
        let start = end.saturating_sub(size);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::new();
        (&mut file).take(end - start).read_to_end(&mut tail).ok()?;
        let lines: Vec<&[u8]> = tail.split(|byte| *byte == b'\n').collect();
        // The first line may start partway through an entry.
        let whole_lines = if start > 0 { &lines[1..] } else { &lines[..] };
        if let Some(stamp) = whole_lines
            .iter()
            .rev()
            .find_map(|line| message_stamp(line))
        {
            return Some(stamp);
        }
        if start == 0 || size >= MAX_TAIL_BYTES {
            return None;
        }
        size *= 4;
    }
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, rename = "isMeta")]
    is_meta: bool,
    timestamp: Option<String>,
}

/// A message's timestamp. Claude writes JavaScript's `toISOString()`, always
/// UTC (`2026-10-27T11:01:46.123Z`), so the strings sort in time order. Meta
/// messages are Claude's own, not activity.
fn message_stamp(line: &[u8]) -> Option<String> {
    let entry: Entry = serde_json::from_slice(line).ok()?;
    if !matches!(entry.kind.as_str(), "user" | "assistant") || entry.is_meta {
        return None;
    }
    entry.timestamp.filter(|stamp| stamp.ends_with('Z'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_resume::AgentSessionRef;

    fn test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "herdr-recency-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn claude_session(id: &str) -> PersistedAgentSession {
        PersistedAgentSession {
            source: "herdr:claude".into(),
            agent: "claude".into(),
            session_ref: AgentSessionRef::id(id).unwrap(),
        }
    }

    fn message(kind: &str, stamp: &str) -> String {
        format!(r#"{{"type":"{kind}","timestamp":"{stamp}","message":{{"content":"hi"}}}}"#)
    }

    #[test]
    fn finds_the_last_message_in_any_project_folder() {
        let projects = test_dir("projects");
        let folder = projects.join("-Users-someone-elsewhere");
        fs::create_dir_all(projects.join("-Users-someone-first")).unwrap();
        fs::create_dir_all(&folder).unwrap();
        let lines = [
            message("user", "2026-10-26T09:00:00.000Z"),
            message("assistant", "2026-10-26T09:00:05.000Z"),
            r#"{"type":"user","isMeta":true,"timestamp":"2026-10-27T11:00:00.000Z"}"#.into(),
            r#"{"type":"system","timestamp":"2026-10-27T11:00:01.000Z"}"#.into(),
            r#"{"type":"cost-state"}"#.into(),
            "not json".into(),
        ];
        fs::write(folder.join("abc.jsonl"), lines.join("\n") + "\n").unwrap();

        assert_eq!(
            last_message_stamp_in(&projects, &claude_session("abc")).as_deref(),
            Some("2026-10-26T09:00:05.000Z")
        );
        assert_eq!(
            last_message_stamp_in(&projects, &claude_session("missing")),
            None
        );
        fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn reads_back_past_a_long_tail() {
        let dir = test_dir("tail");
        let path = dir.join("long.jsonl");
        let filler = format!(r#"{{"type":"progress","data":"{}"}}"#, "x".repeat(200_000));
        let content = [message("assistant", "2026-10-25T08:00:00.000Z"), filler].join("\n");
        fs::write(&path, content).unwrap();

        assert_eq!(
            last_message_stamp_in_file(&path).as_deref(),
            Some("2026-10-25T08:00:00.000Z")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn other_agents_and_path_like_ids_are_not_read() {
        let projects = test_dir("other");
        let folder = projects.join("folder");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("abc.jsonl"),
            message("user", "2026-10-26T09:00:00.000Z"),
        )
        .unwrap();
        let mut codex = claude_session("abc");
        codex.agent = "codex".into();
        codex.source = "herdr:codex".into();

        assert_eq!(last_message_stamp_in(&projects, &codex), None);
        assert_eq!(
            last_message_stamp_in(&projects, &claude_session("../folder/abc")),
            None
        );
        fs::remove_dir_all(projects).unwrap();
    }
}
