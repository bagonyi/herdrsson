//! Fork: a Claude agent's background shells, as the footer under its prompt box counts them
//! (`⏵⏵ auto mode on · 2 shells`).
//!
//! The count becomes the pane's `$shells` token, and with
//! `[advanced] claude_background_shells_working = true` an idle Claude with a shell still running
//! counts as working. Upstream counted them as working until #3468, which dropped that rule as a
//! dev server kept its pane working for good. Only an idle reading changes, so a question or
//! permission prompt still reads blocked, and the bundled or downloaded manifest stays as it is.
//!
//! `[[advanced.claude_background_shells_ignore]]` lists commands whose shells don't count, such
//! as dev servers. Each background shell is a child process of Claude's, a shell that runs the
//! command as `eval '<command>'`, so the commands are read from there: only on Linux and macOS,
//! only for an idle reading whose footer shows a shell, and only when a list applies. A shell
//! whose command can't be read counts as work, as before. Claude's monitors run in the same kind
//! of shell, so each monitor the footer shows is taken to be one of the ignored ones.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, LazyLock, OnceLock, RwLock,
    },
};

use regex::Regex;

use super::{DetectionExplain, MatchedRule};
use crate::config::BackgroundShellsIgnoreConfig;
use crate::detect::{Agent, AgentDetection, AgentState};

/// The pane token holding the count's text, `$shells` in sidebar rows.
pub const SHELLS_TOKEN: &str = "shells";
const RULE_ID: &str = "herdrsson_background_shells_working";
const REGION: &str = "after_last_horizontal_rule";

/// Words that can come before a command's name: `nohup npm run dev`, `env PORT=1 npm run dev`.
const COMMAND_PREFIXES: &[&str] = &[
    "!", "{", "if", "then", "else", "elif", "while", "until", "do", "time", "nohup", "exec", "env",
    "command",
];

static COUNT_AS_WORKING: AtomicBool = AtomicBool::new(false);
static IGNORE_LIST: LazyLock<RwLock<Arc<Vec<IgnoreEntry>>>> = LazyLock::new(Default::default);

/// One `[[advanced.claude_background_shells_ignore]]` entry, ready to match.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IgnoreEntry {
    /// Absolute; `None` applies everywhere.
    folder: Option<PathBuf>,
    commands: Vec<IgnoredCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IgnoredCommand {
    text: String,
    /// The program and its first arguments, as `runs` compares them.
    words: Vec<String>,
}

/// A background shell found among Claude's child processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundShell {
    /// The command as Claude ran it; `None` when it can't be read, which counts as work.
    pub command: Option<String>,
    /// The ignore list command it matched; `None` counts as work.
    pub ignored_by: Option<String>,
}

/// What the footer shows: `2 shells`, `1 shell, 1 monitor`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Footer {
    shells: u32,
    monitors: u32,
}

/// Set from `[advanced] claude_background_shells_working` when the config loads.
pub fn set_count_as_working(enabled: bool) {
    COUNT_AS_WORKING.store(enabled, Ordering::Relaxed);
}

/// Set from `[[advanced.claude_background_shells_ignore]]` when the config loads. An entry
/// without commands, or whose folder is neither absolute nor `~/…`, is never used.
pub fn set_ignore_list(entries: &[BackgroundShellsIgnoreConfig]) {
    let entries = Arc::new(
        entries
            .iter()
            .filter_map(IgnoreEntry::from_config)
            .collect::<Vec<_>>(),
    );
    match IGNORE_LIST.write() {
        Ok(mut list) => *list = entries,
        Err(poisoned) => *poisoned.into_inner() = entries,
    }
}

impl IgnoreEntry {
    fn from_config(config: &BackgroundShellsIgnoreConfig) -> Option<Self> {
        // A missing or blank folder applies everywhere.
        let folder = match config.folder.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(folder) => {
                let path = crate::worktree::expand_tilde_path(folder);
                if !path.is_absolute() {
                    return None;
                }
                Some(crate::worktree::canonical_or_original(&path))
            }
        };
        let commands: Vec<_> = config
            .commands
            .iter()
            .filter_map(|text| {
                let words = simple_commands(text)
                    .into_iter()
                    .next()
                    .map(|words| command_words(&words).to_vec())
                    .unwrap_or_default();
                (!words.is_empty()).then(|| IgnoredCommand {
                    text: text.trim().to_string(),
                    words,
                })
            })
            .collect();
        (!commands.is_empty()).then_some(Self { folder, commands })
    }
}

/// How many background shells Claude's footer shows: `1 shell`, `2 shells`, also in
/// `1 shell, 1 monitor`. 0 for other agents, without shells, and without a prompt box, so text
/// in Claude's answers can't count.
pub fn background_shells(agent: Option<Agent>, screen: &str) -> u32 {
    footer(agent, screen).shells
}

fn footer(agent: Option<Agent>, screen: &str) -> Footer {
    if agent != Some(Agent::Claude) || !screen.lines().any(super::is_horizontal_rule) {
        return Footer::default();
    }
    let region = super::after_last_horizontal_rule(screen);
    let count = |regex: Option<&Regex>| {
        regex
            .and_then(|regex| {
                region
                    .lines()
                    .find_map(|line| regex.captures(line)?.get(1)?.as_str().parse().ok())
            })
            .unwrap_or(0)
    };
    Footer {
        shells: count(shells_regex()),
        monitors: count(monitors_regex()),
    }
}

/// The `$shells` token's text for a count: just the number, which a row's `prefix` can give an
/// icon; none without shells.
pub fn token_text(count: u32) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// For the pane's detection loop: an idle reading of a Claude agent becomes working while its
/// footer shows more background shells than the ignore list covers. `agent_pid` is Claude's
/// process, whose children are the shells.
pub fn count_as_working(
    detection: AgentDetection,
    agent: Option<Agent>,
    screen: &str,
    agent_pid: Option<u32>,
) -> AgentDetection {
    detection_with_shells(
        COUNT_AS_WORKING.load(Ordering::Relaxed),
        detection,
        || footer(agent, screen),
        || ignored_shells(agent_pid),
    )
}

/// For `herdr agent explain`: the same change, plus the background shells it found, as JSON for
/// the explain output (none when the footer shows no shell). `agent_pid` is asked only then.
pub fn explain(
    explain: DetectionExplain,
    agent: Agent,
    screen: &str,
    agent_pid: impl FnOnce() -> Option<u32>,
) -> (DetectionExplain, Option<serde_json::Value>) {
    explain_with_shells(
        COUNT_AS_WORKING.load(Ordering::Relaxed),
        explain,
        footer(Some(agent), screen),
        || {
            agent_pid()
                .map(|pid| {
                    let list = ignore_list();
                    let ignore = applicable(&list, || crate::platform::process_cwd(pid));
                    shells_with_commands(crate::platform::child_shell_argvs(pid), &ignore)
                })
                .unwrap_or_default()
        },
    )
}

/// `footer` is read only for an idle reading with the setting on, and `ignored` only when it
/// shows a shell.
fn detection_with_shells(
    enabled: bool,
    detection: AgentDetection,
    footer: impl FnOnce() -> Footer,
    ignored: impl FnOnce() -> u32,
) -> AgentDetection {
    if !changes(enabled, detection.state, detection.skip_state_update) {
        return detection;
    }
    let footer = footer();
    if footer.shells == 0 || !keeps_working(footer, ignored()) {
        return detection;
    }
    AgentDetection {
        state: AgentState::Working,
        visible_idle: false,
        visible_working: true,
        ..detection
    }
}

fn explain_with_shells(
    enabled: bool,
    explain: DetectionExplain,
    footer: Footer,
    shells: impl FnOnce() -> Vec<BackgroundShell>,
) -> (DetectionExplain, Option<serde_json::Value>) {
    if footer.shells == 0 {
        return (explain, None);
    }
    let shells = shells();
    let ignored = ignored_count(&shells);
    let explain = if changes(enabled, explain.state, explain.skip_state_update)
        && keeps_working(footer, ignored)
    {
        working_explain(explain)
    } else {
        explain
    };
    let report = serde_json::json!({
        "count": footer.shells,
        "monitors": footer.monitors,
        "ignored": ignored,
        "shells": shells
            .iter()
            .map(|shell| serde_json::json!({
                "command": shell.command,
                "ignored_by": shell.ignored_by,
            }))
            .collect::<Vec<_>>(),
    });
    (explain, Some(report))
}

/// Only an idle reading changes, and only with the setting on.
fn changes(enabled: bool, state: AgentState, skip_state_update: bool) -> bool {
    enabled && state == AgentState::Idle && !skip_state_update
}

/// Whether the footer's shells outnumber the ignored ones. A monitor runs in the same kind of
/// shell as a background shell, so up to that many of the ignored ones may be monitors: they're
/// taken to be, so a monitor tailing a job's log can't hide the job.
fn keeps_working(footer: Footer, ignored: u32) -> bool {
    footer.shells > ignored.saturating_sub(footer.monitors)
}

fn working_explain(mut explain: DetectionExplain) -> DetectionExplain {
    explain.matched_rule = Some(MatchedRule {
        id: RULE_ID.to_string(),
        priority: explain
            .matched_rule
            .as_ref()
            .map_or(0, |matched| matched.priority),
        region: REGION.to_string(),
        state: AgentState::Working,
    });
    explain.state = AgentState::Working;
    explain.visible_idle = false;
    explain.visible_working = true;
    explain.fallback_reason = None;
    explain
}

/// The ignore list, without holding its lock while processes are read.
fn ignore_list() -> Arc<Vec<IgnoreEntry>> {
    IGNORE_LIST
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// How many of Claude's background shells (and monitors) the ignore list covers. Reads Claude's
/// folder only when an entry names one, and its child shells only when an entry applies.
fn ignored_shells(agent_pid: Option<u32>) -> u32 {
    let Some(pid) = agent_pid else {
        return 0;
    };
    let list = ignore_list();
    if list.is_empty() {
        return 0;
    }
    let ignore = applicable(&list, || crate::platform::process_cwd(pid));
    if ignore.is_empty() {
        return 0;
    }
    ignored_count(&shells_with_commands(
        crate::platform::child_shell_argvs(pid),
        &ignore,
    ))
}

fn ignored_count(shells: &[BackgroundShell]) -> u32 {
    shells
        .iter()
        .filter(|shell| shell.ignored_by.is_some())
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

/// The ignored commands for a Claude agent started in `agent_folder`: entries without a folder,
/// and those whose folder holds it.
fn applicable(
    list: &[IgnoreEntry],
    agent_folder: impl FnOnce() -> Option<PathBuf>,
) -> Vec<&IgnoredCommand> {
    let agent_folder = if list.iter().any(|entry| entry.folder.is_some()) {
        agent_folder()
    } else {
        None
    };
    list.iter()
        .filter(|entry| match &entry.folder {
            None => true,
            Some(folder) => agent_folder
                .as_deref()
                .is_some_and(|agent_folder| agent_folder.starts_with(folder)),
        })
        .flat_map(|entry| &entry.commands)
        .collect()
}

/// Claude's child shells (their argvs) as background shells, each with the ignored command it
/// runs.
fn shells_with_commands(
    children: Vec<Vec<String>>,
    ignore: &[&IgnoredCommand],
) -> Vec<BackgroundShell> {
    children
        .iter()
        .map(|argv| {
            let command = shell_command(argv);
            let ignored_by = command.as_deref().and_then(|command| {
                let commands = simple_commands(command);
                ignore
                    .iter()
                    .find(|ignored| commands.iter().any(|words| runs(words, &ignored.words)))
                    .map(|ignored| ignored.text.clone())
            });
            BackgroundShell {
                command,
                ignored_by,
            }
        })
        .collect()
}

/// The command a background shell runs. Claude starts each one as
/// `zsh -c "source …/shell-snapshots/… && … && eval '<command>' < /dev/null && pwd -P >| …"`
/// (`zsh -c -l …` without a snapshot), quoting the command's own `'` as `'"'"'`. A session
/// environment script can come first with `eval` lines of its own, which end their line, so
/// Claude's is the first `eval` followed by `<`, `&&` or nothing. None for processes that don't
/// look like that.
fn shell_command(argv: &[String]) -> Option<String> {
    let script = argv
        .iter()
        .skip_while(|arg| *arg != "-c")
        .skip(1)
        .find(|arg| !arg.starts_with('-'))?;
    script
        .match_indices("eval ")
        .filter(|(index, _)| {
            script[..*index]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
        })
        .find_map(|(index, eval)| {
            let (command, rest) = shell_word(&script[index + eval.len()..])?;
            let rest = rest.trim_start_matches([' ', '\t']);
            (rest.is_empty() || rest.starts_with('<') || rest.starts_with("&&")).then_some(command)
        })
        .filter(|command| !command.trim().is_empty())
}

/// The first shell word of `text`, with its quotes removed (`'…'`, `"…"` and `\x`), and the
/// text after it. None when a quote isn't closed.
fn shell_word(text: &str) -> Option<(String, &str)> {
    let mut word = String::new();
    let mut chars = text.trim_start().chars();
    loop {
        let rest = chars.as_str();
        let Some(ch) = chars.next() else {
            return Some((word, rest));
        };
        match ch {
            '\'' => loop {
                match chars.next()? {
                    '\'' => break,
                    ch => word.push(ch),
                }
            },
            '"' => loop {
                match chars.next()? {
                    '"' => break,
                    '\\' => push_escaped_in_double_quotes(&mut word, chars.next()?),
                    ch => word.push(ch),
                }
            },
            '\\' => {
                if let Some(next) = chars.next().filter(|next| *next != '\n') {
                    word.push(next);
                }
            }
            ch if ch.is_whitespace() => return Some((word, rest)),
            ch => word.push(ch),
        }
    }
}

/// `\x` inside double quotes: only `"`, `\`, `$`, `` ` `` and a newline are escaped.
fn push_escaped_in_double_quotes(word: &mut String, next: char) {
    if !matches!(next, '"' | '\\' | '$' | '`' | '\n') {
        word.push('\\');
    }
    if next != '\n' {
        word.push(next);
    }
}

/// The simple commands in a shell command line, each as its words with quotes removed: split
/// at `;`, `&`, `|`, `(`, `)`, backticks and newlines outside quotes, leaving out redirections'
/// operators, comments and heredoc bodies. `cd web && PORT=1 npm run dev > dev.log` holds
/// `cd web` and `PORT=1 npm run dev dev.log`.
fn simple_commands(script: &str) -> Vec<Vec<String>> {
    #[derive(Default)]
    struct Lexer {
        word: String,
        in_word: bool,
        words: Vec<String>,
        commands: Vec<Vec<String>>,
    }
    impl Lexer {
        fn end_word(&mut self) {
            if self.in_word {
                self.words.push(std::mem::take(&mut self.word));
                self.in_word = false;
            }
        }
        fn end_command(&mut self) {
            self.end_word();
            if !self.words.is_empty() {
                self.commands.push(std::mem::take(&mut self.words));
            }
        }
    }

    let mut lexer = Lexer::default();
    let mut heredocs = Vec::new();
    let mut chars = script.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                lexer.in_word = true;
                for ch in chars.by_ref() {
                    if ch == '\'' {
                        break;
                    }
                    lexer.word.push(ch);
                }
            }
            '"' => {
                lexer.in_word = true;
                while let Some(ch) = chars.next() {
                    match ch {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                push_escaped_in_double_quotes(&mut lexer.word, next);
                            }
                        }
                        ch => lexer.word.push(ch),
                    }
                }
            }
            '\\' => {
                if let Some(next) = chars.next().filter(|next| *next != '\n') {
                    lexer.in_word = true;
                    lexer.word.push(next);
                }
            }
            '#' if !lexer.in_word => while chars.next_if(|ch| *ch != '\n').is_some() {},
            '<' | '>' => {
                lexer.end_word();
                if ch == '<' && chars.next_if_eq(&'<').is_some() {
                    // `<<<` is a here-string; `<<WORD` and `<<-WORD` start a heredoc.
                    if chars.next_if_eq(&'<').is_none() {
                        chars.next_if_eq(&'-');
                        while chars.next_if(|ch| *ch == ' ' || *ch == '\t').is_some() {}
                        let mut delimiter = String::new();
                        while let Some(ch) =
                            chars.next_if(|ch| !ch.is_whitespace() && !";&|()<>".contains(*ch))
                        {
                            if !matches!(ch, '\'' | '"' | '\\') {
                                delimiter.push(ch);
                            }
                        }
                        if !delimiter.is_empty() {
                            heredocs.push(delimiter);
                        }
                    }
                } else {
                    // `2>&1`, `>&2`: the `&` belongs to the redirection.
                    chars.next_if_eq(&'&');
                }
            }
            '&' if chars.peek() == Some(&'>') => lexer.end_word(),
            '\n' => {
                lexer.end_command();
                for delimiter in heredocs.drain(..) {
                    loop {
                        let mut line = String::new();
                        let mut ended = false;
                        for ch in chars.by_ref() {
                            if ch == '\n' {
                                ended = true;
                                break;
                            }
                            line.push(ch);
                        }
                        if !ended
                            || line.trim_start_matches('\t').trim_end_matches('\r') == delimiter
                        {
                            break;
                        }
                    }
                }
            }
            ';' | '&' | '|' | '(' | ')' | '`' => lexer.end_command(),
            ch if ch.is_whitespace() => lexer.end_word(),
            ch => {
                lexer.in_word = true;
                lexer.word.push(ch);
            }
        }
    }
    lexer.end_command();
    lexer.commands
}

/// A simple command's words from its program on: without `VAR=value` assignments and words
/// such as `nohup` before it.
fn command_words(words: &[String]) -> &[String] {
    let skipped = words
        .iter()
        .take_while(|word| is_assignment(word) || COMMAND_PREFIXES.contains(&word.as_str()))
        .count();
    &words[skipped..]
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.chars()
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && name
                .chars()
                .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    })
}

/// Whether a simple command (its words) runs `wanted`: its words, from its program on, start
/// with them. The program counts without its folder (`/usr/bin/python3` is `python3`) unless
/// `wanted` names one.
fn runs(words: &[String], wanted: &[String]) -> bool {
    let words = command_words(words);
    let (Some(program), Some(wanted_program)) = (words.first(), wanted.first()) else {
        return false;
    };
    let program = if wanted_program.contains('/') {
        program.as_str()
    } else {
        program.rsplit('/').next().unwrap_or(program)
    };
    words.len() >= wanted.len()
        && program == wanted_program
        && words[1..wanted.len()] == wanted[1..]
}

fn shells_regex() -> Option<&'static Regex> {
    static REGEX: OnceLock<Option<Regex>> = OnceLock::new();
    REGEX
        .get_or_init(|| {
            // The footer's items are joined by " · "; the count starts one, or the line.
            Regex::new(r"(?:^|·)\s*([1-9]\d*) shells?\b").ok()
        })
        .as_ref()
}

fn monitors_regex() -> Option<&'static Regex> {
    static REGEX: OnceLock<Option<Regex>> = OnceLock::new();
    REGEX
        .get_or_init(|| {
            // `· 1 monitor`, or after the shells: `1 shell, 2 monitors`.
            Regex::new(r"(?:^|[·,])\s*([1-9]\d*) monitors?\b").ok()
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE_WITH_SHELL: &str = concat!(
        "✻ Baked for 27s · done 13:16 · 1 shell still running\n\n",
        "────────────────────────────────────────────────────────────────\n",
        "❯\n",
        "────────────────────────────────────────────────────────────────\n",
        "  Opus 5.5 xhigh · v2.1.317\n",
        "  ⏵⏵ auto mode on · 1 shell\n",
    );

    const ONE_SHELL: Footer = Footer {
        shells: 1,
        monitors: 0,
    };

    fn idle_detection() -> AgentDetection {
        AgentDetection {
            state: AgentState::Idle,
            skip_state_update: false,
            visible_idle: true,
            visible_blocker: false,
            visible_working: false,
        }
    }

    fn idle_explain() -> DetectionExplain {
        DetectionExplain {
            agent: Some("claude".to_string()),
            state: AgentState::Idle,
            source: None,
            matched_rule: Some(MatchedRule {
                id: "live_prompt_box".to_string(),
                priority: 950,
                region: "prompt_box_body".to_string(),
                state: AgentState::Idle,
            }),
            screen_detection_skipped: false,
            visible_idle: true,
            visible_blocker: false,
            visible_working: false,
            skip_state_update: false,
            skipped_update_reason: None,
            fallback_reason: None,
            evaluated_rules: Vec::new(),
            warning: None,
            manifest_version: None,
            cached_remote_version: None,
            local_override_shadowing_remote: false,
            remote_update_status: None,
            remote_update_error: None,
        }
    }

    /// Claude's wrapper around a background command, already quoted for `eval '…'`.
    fn claude_shell(quoted_command: &str) -> Vec<String> {
        vec![
            "/bin/zsh".to_string(),
            "-c".to_string(),
            format!(
                "source /Users/u/.claude/shell-snapshots/snapshot-zsh-1-x.sh 2>/dev/null || true \
                 && setopt NO_EXTENDED_GLOB NO_BARE_GLOB_QUAL 2>/dev/null || true \
                 && {{ \\builtin unalias -- 'which'; \\builtin unset -f -- 'which'; }} \
                 >/dev/null 2>&1 || true && eval {quoted_command} < /dev/null \
                 && pwd -P >| /tmp/claude-a6b6-cwd"
            ),
        ]
    }

    fn entry(folder: Option<&str>, commands: &[&str]) -> IgnoreEntry {
        IgnoreEntry::from_config(&BackgroundShellsIgnoreConfig {
            folder: folder.map(str::to_string),
            commands: commands.iter().map(|command| command.to_string()).collect(),
        })
        .expect("entry with commands")
    }

    /// Whether the ignore list command `wanted` matches the background command `command`.
    fn matches(command: &str, wanted: &str) -> bool {
        let list = vec![entry(None, &[wanted])];
        let ignore = applicable(&list, || None);
        shells_with_commands(vec![claude_shell(&quote(command))], &ignore)[0]
            .ignored_by
            .is_some()
    }

    /// Quotes `text` the way Claude does for `eval`.
    fn quote(text: &str) -> String {
        format!("'{}'", text.replace('\'', r#"'"'"'"#))
    }

    #[test]
    fn counts_shells_and_monitors_in_the_footer() {
        assert_eq!(background_shells(Some(Agent::Claude), IDLE_WITH_SHELL), 1);
        let footer_of = |line: &str| {
            footer(
                Some(Agent::Claude),
                &format!("text\n──────\n❯\n──────\n{line}\n"),
            )
        };
        for (line, shells, monitors) in [
            (
                "  ⏵⏵ auto mode on · 2 shells · ← for agents        /rc",
                2,
                0,
            ),
            ("  ⏸ plan mode on · 1 shell, 1 monitor", 1, 1),
            ("  12 shells, 3 monitors · ← for agents", 12, 3),
            ("  ⏵⏵ auto mode on · 2 monitors", 0, 2),
            ("  ⏵⏵ auto mode on · 0 shells", 0, 0),
            ("  ⏵⏵ auto mode on · 3 shellfish", 0, 0),
            ("  ⏵⏵ auto mode on", 0, 0),
        ] {
            assert_eq!(footer_of(line), Footer { shells, monitors }, "{line}");
        }
    }

    #[test]
    fn counts_only_claude_under_its_prompt_box() {
        assert_eq!(background_shells(Some(Agent::Codex), IDLE_WITH_SHELL), 0);
        assert_eq!(background_shells(None, IDLE_WITH_SHELL), 0);
        // Above the prompt box, and on a screen without one, it's Claude's text, not the footer.
        assert_eq!(
            background_shells(
                Some(Agent::Claude),
                "· 2 shells\n──────\n❯\n──────\n  ? for shortcuts\n"
            ),
            0
        );
        assert_eq!(
            background_shells(Some(Agent::Claude), "it said · 2 shells\n"),
            0
        );
    }

    #[test]
    fn token_text_is_the_count() {
        assert_eq!(token_text(0), None);
        assert_eq!(token_text(1).as_deref(), Some("1"));
        assert_eq!(token_text(3).as_deref(), Some("3"));
    }

    #[test]
    fn idle_with_a_shell_counts_as_working_only_when_enabled() {
        let off = detection_with_shells(false, idle_detection(), || ONE_SHELL, || 0);
        assert_eq!(off, idle_detection());

        let on = detection_with_shells(true, idle_detection(), || ONE_SHELL, || 0);
        assert_eq!(on.state, AgentState::Working);
        assert!(on.visible_working);
        assert!(!on.visible_idle);
    }

    #[test]
    fn only_an_idle_reading_with_a_shell_changes() {
        for state in [
            AgentState::Blocked,
            AgentState::Working,
            AgentState::Unknown,
        ] {
            let detection = AgentDetection {
                state,
                ..idle_detection()
            };
            assert_eq!(
                detection_with_shells(true, detection, || ONE_SHELL, || 0),
                detection
            );
        }
        let skipped = AgentDetection {
            skip_state_update: true,
            ..idle_detection()
        };
        assert_eq!(
            detection_with_shells(true, skipped, || ONE_SHELL, || 0),
            skipped
        );
        assert_eq!(
            detection_with_shells(true, idle_detection(), Footer::default, || 0),
            idle_detection()
        );
    }

    #[test]
    fn ignored_shells_dont_count() {
        let reading = |footer: Footer, ignored: u32| {
            detection_with_shells(true, idle_detection(), || footer, || ignored).state
        };
        let footer = |shells, monitors| Footer { shells, monitors };
        assert_eq!(reading(footer(1, 0), 1), AgentState::Idle);
        assert_eq!(reading(footer(2, 0), 1), AgentState::Working);
        // An ignored process may be a monitor tailing the job's log: the job still counts.
        assert_eq!(reading(footer(1, 1), 1), AgentState::Working);
        assert_eq!(reading(footer(1, 1), 2), AgentState::Idle);
    }

    #[test]
    fn reads_nothing_it_doesnt_need() {
        let no_footer = || -> Footer { panic!("footer read") };
        let no_processes = || -> u32 { panic!("processes read") };
        detection_with_shells(false, idle_detection(), no_footer, no_processes);
        let blocked = AgentDetection {
            state: AgentState::Blocked,
            ..idle_detection()
        };
        detection_with_shells(true, blocked, no_footer, no_processes);
        detection_with_shells(true, idle_detection(), Footer::default, no_processes);
    }

    #[test]
    fn explain_names_the_check_and_the_shells() {
        let shells = || {
            vec![
                BackgroundShell {
                    command: Some("npm run dev".into()),
                    ignored_by: Some("npm run dev".into()),
                },
                BackgroundShell {
                    command: Some("cargo test".into()),
                    ignored_by: None,
                },
                BackgroundShell {
                    command: None,
                    ignored_by: None,
                },
            ]
        };
        let footer = Footer {
            shells: 2,
            monitors: 1,
        };
        let (explain, report) = explain_with_shells(true, idle_explain(), footer, shells);
        assert_eq!(explain.state, AgentState::Working);
        assert_eq!(
            explain.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some(RULE_ID)
        );
        let report = report.expect("report");
        assert_eq!(report["count"], 2);
        assert_eq!(report["monitors"], 1);
        assert_eq!(report["ignored"], 1);
        assert_eq!(report["shells"][0]["ignored_by"], "npm run dev");
        assert!(report["shells"][1]["ignored_by"].is_null());
        assert!(report["shells"][2]["command"].is_null());

        let (explain, _) =
            explain_with_shells(true, idle_explain(), ONE_SHELL, || shells()[..1].to_vec());
        assert_eq!(explain, idle_explain());
        let (explain, report) =
            explain_with_shells(true, idle_explain(), Footer::default(), || {
                panic!("processes read")
            });
        assert_eq!((explain, report), (idle_explain(), None));
    }

    #[test]
    fn reads_the_command_from_claudes_wrapper() {
        assert_eq!(
            shell_command(&claude_shell("'sleep 300; echo done'")).as_deref(),
            Some("sleep 300; echo done")
        );
        // Claude quotes a `'` in the command as `'"'"'`; `'\''` reads the same.
        assert_eq!(
            shell_command(&claude_shell(
                r#"'sleep 90; echo '"'"'it'"'"'"'"'"'"'"'"'s'"'"' > /dev/null'"#
            ))
            .as_deref(),
            Some(r#"sleep 90; echo 'it'"'"'s' > /dev/null"#)
        );
        assert_eq!(
            shell_command(&claude_shell(r"'echo '\''hi'\'''")).as_deref(),
            Some("echo 'hi'")
        );
        // A command with an `eval` of its own.
        assert_eq!(
            shell_command(&claude_shell(&quote("x=1 && eval 'npm run dev'"))).as_deref(),
            Some("x=1 && eval 'npm run dev'")
        );
        // Without a shell snapshot Claude adds `-l`.
        let mut login = claude_shell("'npm run dev'");
        login.insert(2, "-l".to_string());
        assert_eq!(shell_command(&login).as_deref(), Some("npm run dev"));
        // A session environment script with `eval` lines comes before Claude's `eval`.
        let env_script = vec![
            "/bin/zsh".to_string(),
            "-c".to_string(),
            "source /s.sh 2>/dev/null || true && export A=1\neval \"$(direnv export zsh)\"\n: \
             && eval 'npm run dev' < /dev/null && pwd -P >| /tmp/claude-1-cwd"
                .to_string(),
        ];
        assert_eq!(shell_command(&env_script).as_deref(), Some("npm run dev"));
        // A heredoc command has no `< /dev/null`.
        let heredoc = vec![
            "/bin/zsh".to_string(),
            "-c".to_string(),
            "source /s.sh && eval 'cat <<EOF\nhi\nEOF' && pwd -P >| /tmp/claude-1-cwd".to_string(),
        ];
        assert_eq!(
            shell_command(&heredoc).as_deref(),
            Some("cat <<EOF\nhi\nEOF")
        );
        // Not Claude's background shells: MCP servers, other tools, broken quoting.
        for argv in [
            vec![
                "npm".into(),
                "exec".into(),
                "chrome-devtools-mcp@1.11.0".into(),
            ],
            vec!["caffeinate".into(), "-i".into(), "-t".into(), "300".into()],
            vec!["/bin/sh".into(), "-c".into(), "npm run dev".into()],
            vec![
                "/bin/sh".into(),
                "-c".into(),
                "medieval 'npm run dev'".into(),
            ],
            claude_shell("'npm run dev"),
            claude_shell("''"),
        ] {
            assert_eq!(shell_command(&argv), None, "{argv:?}");
        }
    }

    #[test]
    fn commands_match_where_a_command_starts() {
        for (command, wanted) in [
            ("npm run dev", "npm run dev"),
            ("npm run dev -- --port 3001", "npm run dev"),
            ("cd web && npm run dev > dev.log 2>&1", "npm run dev"),
            ("PORT=3001 npm run dev", "npm run dev"),
            ("env PORT=3001 npm run dev", "npm run dev"),
            ("(cd web; npm run dev)", "npm run dev"),
            ("nohup npm run dev&", "npm run dev"),
            ("npm run dev>dev.log", "npm run dev"),
            ("tail -f a.log | grep --line-buffered error", "tail -f"),
            (
                "/usr/bin/python3 -m http.server 8765",
                "python3 -m http.server",
            ),
            ("./node_modules/.bin/next dev", "next dev"),
            ("vite --port 3000", "vite"),
            ("npx vite --port 3000", "npx vite"),
            ("cat <<EOF > x\nhi\nEOF\nnpm run dev", "npm run dev"),
        ] {
            assert!(
                matches(command, wanted),
                "{command:?} should match {wanted:?}"
            );
        }
        for (command, wanted) in [
            ("npm run devtools", "npm run dev"),
            ("npm run dev:all", "npm run dev"),
            ("npm run build && npm test", "npm run dev"),
            ("echo npm run dev", "npm run dev"),
            ("npx vitest run", "vite"),
            ("npx vite --port 3000", "vite"),
            (
                r#"npx concurrently -k -s first "npm run dev" "npx playwright test""#,
                "npm run dev",
            ),
            (
                "npx start-server-and-test 'npm run dev' http://localhost:3000 'npm run e2e'",
                "npm run dev",
            ),
            ("git commit -m 'fix tail -f in the README' && cargo test", "tail -f"),
            (r#"cargo build && echo "done; next dev step""#, "next dev"),
            (
                "cat > /tmp/e2e.sh <<'EOF'\nnpm run dev &\nnpx playwright test\nEOF\nbash /tmp/e2e.sh",
                "npm run dev",
            ),
            ("cat <<-END\n\tnpm run dev\n\tEND\ncargo test", "npm run dev"),
            ("# npm run dev\ncargo test", "npm run dev"),
            ("/opt/tools/python3 -m http.server", "/usr/bin/python3 -m http.server"),
        ] {
            assert!(!matches(command, wanted), "{command:?} shouldn't match {wanted:?}");
        }
    }

    #[test]
    fn splits_a_command_line_into_simple_commands() {
        assert_eq!(
            simple_commands("cd web && PORT=1 npm run dev > dev.log 2>&1 &\necho 'a; b' | cat"),
            [
                vec!["cd", "web"],
                vec!["PORT=1", "npm", "run", "dev", "dev.log", "2", "1"],
                vec!["echo", "a; b"],
                vec!["cat"],
            ]
        );
        assert_eq!(
            command_words(&simple_commands("nohup env A=1 B=2 time npm start")[0]),
            ["npm", "start"]
        );
        assert!(is_assignment("PORT=3001"));
        assert!(!is_assignment("--port=3001"));
        assert!(!is_assignment("1X=2"));
    }

    #[test]
    fn entries_apply_everywhere_or_in_their_folder() {
        let list = vec![
            entry(None, &["npm run dev"]),
            entry(Some("/work/web"), &["cargo watch"]),
        ];
        let texts = |agent_folder: Option<&str>| -> Vec<String> {
            applicable(&list, || agent_folder.map(PathBuf::from))
                .into_iter()
                .map(|command| command.text.clone())
                .collect()
        };
        assert_eq!(texts(Some("/work/web")), ["npm run dev", "cargo watch"]);
        assert_eq!(texts(Some("/work/web/app")), ["npm run dev", "cargo watch"]);
        assert_eq!(texts(Some("/work/website")), ["npm run dev"]);
        assert_eq!(texts(None), ["npm run dev"]);

        // Claude's folder is read only when an entry names one.
        let global = vec![entry(None, &["npm run dev"])];
        let unread = || -> Option<PathBuf> { panic!("folder read") };
        assert_eq!(applicable(&global, unread).len(), 1);
    }

    #[test]
    fn config_entries_are_cleaned_up() {
        let config = |folder: Option<&str>, commands: &[&str]| BackgroundShellsIgnoreConfig {
            folder: folder.map(str::to_string),
            commands: commands.iter().map(|command| command.to_string()).collect(),
        };
        let parsed = IgnoreEntry::from_config(&config(Some(" "), &[" nohup npm run dev ", "  "]))
            .expect("entry");
        assert_eq!(parsed.folder, None);
        assert_eq!(parsed.commands.len(), 1);
        assert_eq!(parsed.commands[0].text, "nohup npm run dev");
        assert_eq!(parsed.commands[0].words, ["npm", "run", "dev"]);
        // No commands, or a relative folder: the entry is never used.
        assert_eq!(IgnoreEntry::from_config(&config(None, &[])), None);
        assert_eq!(
            IgnoreEntry::from_config(&config(Some("web"), &["vite"])),
            None
        );
    }

    #[test]
    fn finds_ignored_shells_among_claudes_children() {
        let list = vec![entry(None, &["npm run dev", "tail -f"])];
        let ignore = applicable(&list, || None);
        let shells = shells_with_commands(
            vec![
                claude_shell("'cd web && npm run dev > dev.log 2>&1'"),
                claude_shell("'cargo test --workspace'"),
                vec!["/bin/zsh".into(), "-c".into(), "something else".into()],
            ],
            &ignore,
        );
        assert_eq!(
            shells,
            [
                BackgroundShell {
                    command: Some("cd web && npm run dev > dev.log 2>&1".into()),
                    ignored_by: Some("npm run dev".into()),
                },
                BackgroundShell {
                    command: Some("cargo test --workspace".into()),
                    ignored_by: None,
                },
                BackgroundShell {
                    command: None,
                    ignored_by: None,
                },
            ]
        );
        assert_eq!(ignored_count(&shells), 1);
    }
}
