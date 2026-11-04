//! Fork: a Claude agent's background shells, as the footer under its prompt box counts them
//! (`⏵⏵ auto mode on · 2 shells`).
//!
//! The count becomes the pane's `$shells` token, and with
//! `[advanced] claude_background_shells_working = true` an idle Claude with a shell still running
//! counts as working. Upstream counted them as working until #3468, which dropped that rule as a
//! dev server kept its pane working for good. Only an idle reading changes, so a question or
//! permission prompt still reads blocked, and the bundled or downloaded manifest stays as it is.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

use regex::Regex;

use super::{DetectionExplain, DetectionInput, MatchedRule};
use crate::detect::{Agent, AgentState};

/// The pane token holding the count's text, `$shells` in sidebar rows.
pub const SHELLS_TOKEN: &str = "shells";
const RULE_ID: &str = "herdrsson_background_shells_working";
const REGION: &str = "after_last_horizontal_rule";

static COUNT_AS_WORKING: AtomicBool = AtomicBool::new(false);

/// Set from `[advanced] claude_background_shells_working` when the config loads.
pub fn set_count_as_working(enabled: bool) {
    COUNT_AS_WORKING.store(enabled, Ordering::Relaxed);
}

/// How many background shells Claude's footer shows: `1 shell`, `2 shells`, also in
/// `1 shell, 1 monitor`. 0 for other agents, without shells, and without a prompt box, so text
/// in Claude's answers can't count.
pub fn background_shells(agent: Option<Agent>, screen: &str) -> u32 {
    let Some(regex) = shells_regex() else {
        return 0;
    };
    if agent != Some(Agent::Claude) || !screen.lines().any(super::is_horizontal_rule) {
        return 0;
    }
    super::after_last_horizontal_rule(screen)
        .lines()
        .find_map(|line| regex.captures(line)?.get(1)?.as_str().parse().ok())
        .unwrap_or(0)
}

/// The `$shells` token's text for a count, in Claude's words; none without shells.
pub fn token_text(count: u32) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 shell".to_string()),
        count => Some(format!("{count} shells")),
    }
}

pub(super) fn count_as_working(
    agent: Agent,
    input: DetectionInput<'_>,
    explain: DetectionExplain,
) -> DetectionExplain {
    apply(
        COUNT_AS_WORKING.load(Ordering::Relaxed),
        agent,
        input,
        explain,
    )
}

fn apply(
    enabled: bool,
    agent: Agent,
    input: DetectionInput<'_>,
    mut explain: DetectionExplain,
) -> DetectionExplain {
    if !enabled
        || explain.state != AgentState::Idle
        || explain.skip_state_update
        || background_shells(Some(agent), input.screen) == 0
    {
        return explain;
    }
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

fn shells_regex() -> Option<&'static Regex> {
    static REGEX: OnceLock<Option<Regex>> = OnceLock::new();
    REGEX
        .get_or_init(|| {
            // The footer's items are joined by " · "; the count starts one, or the line.
            Regex::new(r"(?:^|·)\s*([1-9]\d*) shells?\b").ok()
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

    fn input(screen: &str) -> DetectionInput<'_> {
        DetectionInput {
            screen,
            osc_title: "",
            osc_progress: "",
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

    #[test]
    fn counts_shells_in_the_footer() {
        assert_eq!(background_shells(Some(Agent::Claude), IDLE_WITH_SHELL), 1);
        let footer = |line: &str| format!("text\n──────\n❯\n──────\n{line}\n");
        for (line, count) in [
            ("  ⏵⏵ auto mode on · 2 shells · ← for agents        /rc", 2),
            ("  ⏸ plan mode on · 1 shell, 1 monitor", 1),
            ("  12 shells · ← for agents", 12),
            ("  ⏵⏵ auto mode on · 1 monitor", 0),
            ("  ⏵⏵ auto mode on · 0 shells", 0),
            ("  ⏵⏵ auto mode on · 3 shellfish", 0),
            ("  ⏵⏵ auto mode on", 0),
        ] {
            assert_eq!(
                background_shells(Some(Agent::Claude), &footer(line)),
                count,
                "{line}"
            );
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
    fn token_text_uses_claudes_words() {
        assert_eq!(token_text(0), None);
        assert_eq!(token_text(1).as_deref(), Some("1 shell"));
        assert_eq!(token_text(3).as_deref(), Some("3 shells"));
    }

    #[test]
    fn idle_with_a_shell_counts_as_working_only_when_enabled() {
        let screen = input(IDLE_WITH_SHELL);
        let off = apply(false, Agent::Claude, screen, idle_explain());
        assert_eq!(off, idle_explain());

        let on = apply(true, Agent::Claude, screen, idle_explain());
        assert_eq!(on.state, AgentState::Working);
        assert!(on.visible_working);
        assert!(!on.visible_idle);
        assert_eq!(
            on.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some(RULE_ID)
        );
    }

    #[test]
    fn only_an_idle_reading_changes() {
        let screen = input(IDLE_WITH_SHELL);
        for state in [
            AgentState::Blocked,
            AgentState::Working,
            AgentState::Unknown,
        ] {
            let explain = DetectionExplain {
                state,
                ..idle_explain()
            };
            assert_eq!(apply(true, Agent::Claude, screen, explain.clone()), explain);
        }
        let skipped = DetectionExplain {
            skip_state_update: true,
            ..idle_explain()
        };
        assert_eq!(apply(true, Agent::Claude, screen, skipped.clone()), skipped);
        let without_shell = input("──────\n❯\n──────\n  ⏵⏵ auto mode on\n");
        assert_eq!(
            apply(true, Agent::Claude, without_shell, idle_explain()),
            idle_explain()
        );
        assert_eq!(
            apply(true, Agent::Codex, screen, idle_explain()),
            idle_explain()
        );
    }
}
