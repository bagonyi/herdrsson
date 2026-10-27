//! Fork: `format = "ago"` on a custom sidebar token shows its value, a Unix time in
//! seconds, as the time since then: `10s ago` (in steps of 10 seconds), `5m ago`, `3h ago`,
//! `2d ago`. Rules match that text. A working agent's row shows `working` in its place
//! (`show_working`). The client repaints each second while a row uses it
//! (`ClientShellState::tick_ago`).

use std::time::{SystemTime, UNIX_EPOCH};

use super::tokens::{ResolvedToken, ResolvedTokenKind};
use crate::config::{AgentsSidebarConfig, SidebarTokenFormat, SpacesSidebarConfig};

/// A custom token's value as its row shows it. A value that isn't a Unix time shows as it is.
pub(super) fn custom_value(value: &str, format: Option<SidebarTokenFormat>) -> String {
    match format {
        Some(SidebarTokenFormat::Ago) => {
            ago(value, unix_now()).unwrap_or_else(|| value.to_string())
        }
        None => value.to_string(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A time in the future, as from another machine's clock, counts as `0s ago`. Under a
/// minute it steps by 10 seconds, so it doesn't count every second.
fn ago(value: &str, now: u64) -> Option<String> {
    let seconds = now.saturating_sub(value.parse().ok()?);
    Some(match seconds {
        0..=59 => format!("{}s ago", seconds / 10 * 10),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    })
}

/// A working agent's row shows `working` in place of its `ago` times: how long ago it started
/// isn't what you look for while it works.
pub(crate) fn show_working(rows: &mut [Vec<ResolvedToken>]) {
    for token in rows.iter_mut().flatten() {
        if token.style.format == Some(SidebarTokenFormat::Ago) {
            token.kind = ResolvedTokenKind::Custom("working".into());
        }
    }
}

/// Whether a new second has begun since `last_second` while a row shows a token as `ago`,
/// so the sidebar needs a repaint. Updates `last_second`.
pub(crate) fn sidebar_ago_changed(
    agents: &AgentsSidebarConfig,
    spaces: &SpacesSidebarConfig,
    last_second: &mut u64,
) -> bool {
    sidebar_uses_ago(agents, spaces) && new_second(last_second, unix_now())
}

fn new_second(last_second: &mut u64, now: u64) -> bool {
    std::mem::replace(last_second, now) != now
}

/// Whether an agent or space row shows a token as `ago`, so the sidebar changes as time passes.
fn sidebar_uses_ago(agents: &AgentsSidebarConfig, spaces: &SpacesSidebarConfig) -> bool {
    let agent_rows = agents
        .rows
        .iter()
        .chain(agents.rows_by_agent.values().flatten());
    agent_rows
        .flatten()
        .any(|token| token.parts().1.format.is_some())
        || spaces
            .rows
            .iter()
            .flatten()
            .any(|token| token.parts().1.format.is_some())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::SidebarConfig;
    use crate::ui::sidebar::tokens::{
        agent_rows, space_rows, AgentTokenContext, ResolvedTokenKind, SpaceTokenContext,
    };

    fn sidebar(toml: &str) -> Result<SidebarConfig, toml::de::Error> {
        toml::from_str(toml)
    }

    fn agent_context(tokens: &HashMap<String, String>) -> AgentTokenContext<'_> {
        AgentTokenContext {
            machine: None,
            workspace: "repo",
            tab: None,
            pane: None,
            agent_label: None,
            terminal_title: None,
            terminal_title_stripped: None,
            canonical_agent: None,
            tokens,
        }
    }

    #[test]
    fn ago_counts_seconds_minutes_hours_then_days() {
        let now = 10_000_000;
        for (age, text) in [
            (0, "0s ago"),
            (9, "0s ago"),
            (10, "10s ago"),
            (59, "50s ago"),
            (60, "1m ago"),
            (3_599, "59m ago"),
            (3_600, "1h ago"),
            (86_399, "23h ago"),
            (86_400, "1d ago"),
            (40 * 86_400, "40d ago"),
        ] {
            assert_eq!(ago(&(now - age).to_string(), now).as_deref(), Some(text));
        }
        assert_eq!(ago(&(now + 30).to_string(), now).as_deref(), Some("0s ago"));
        assert_eq!(ago("0010000000", now).as_deref(), Some("0s ago"));
    }

    #[test]
    fn a_value_that_is_not_a_unix_time_shows_as_it_is() {
        for value in ["soon", "", "-5", "12.5", " 12"] {
            assert_eq!(custom_value(value, Some(SidebarTokenFormat::Ago)), value);
        }
        assert_eq!(custom_value("1700000000", None), "1700000000");
    }

    #[test]
    fn agent_and_space_rows_show_the_time_since_and_rules_match_that_text() {
        let config = sidebar(
            r##"
[agents]
rows = [[{ token = "$last_at", format = "ago", rules = [{ contains = "d ago", dim = true }] }]]
[spaces]
rows = [[{ token = "$last_at", format = "ago" }]]
"##,
        )
        .unwrap();
        let tokens = HashMap::from([(
            "last_at".to_string(),
            (unix_now() - 3 * 86_400 - 5).to_string(),
        )]);

        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom("3d ago".into()));
        assert_eq!(rows[0][0].style.dim, Some(true));

        let rows = space_rows(
            &config.spaces,
            SpaceTokenContext {
                workspace: "repo",
                branch: None,
                state_text: "idle",
                ahead_behind: None,
                tokens: &tokens,
                suppress_git_details: false,
            },
        );
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom("3d ago".into()));
    }

    #[test]
    fn a_working_agent_shows_working_in_place_of_its_ago_times_only() {
        let config = sidebar(
            "[agents]\nrows = [[\"workspace\", \"$note\", { token = \"$last_at\", format = \"ago\" }]]\n",
        )
        .unwrap();
        let tokens = HashMap::from([
            ("note".to_string(), "1700000000".to_string()),
            ("last_at".to_string(), (unix_now() - 120).to_string()),
        ]);
        let mut rows = agent_rows(&config.agents, agent_context(&tokens), "working");
        assert_eq!(rows[0][2].kind, ResolvedTokenKind::Custom("2m ago".into()));

        show_working(&mut rows);
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Workspace("repo".into()));
        assert_eq!(
            rows[0][1].kind,
            ResolvedTokenKind::Custom("1700000000".into())
        );
        assert_eq!(rows[0][2].kind, ResolvedTokenKind::Custom("working".into()));
    }

    #[test]
    fn format_applies_only_to_custom_tokens_and_takes_only_ago() {
        assert!(sidebar("[agents]\nrows = [[{ token = \"tab\", format = \"ago\" }]]\n").is_err());
        assert!(sidebar("[agents]\nrows = [[{ token = \"$t\", format = \"since\" }]]\n").is_err());
    }

    #[test]
    fn format_round_trips_through_serialization() {
        let config =
            sidebar("[agents]\nrows = [[{ token = \"$t\", format = \"ago\" }]]\n").unwrap();
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("format = \"ago\""), "{text}");
        assert_eq!(sidebar(&text).unwrap(), config);
    }

    #[test]
    fn a_repaint_is_due_once_per_second_and_only_while_a_row_shows_ago() {
        let with = sidebar("[agents]\nrows = [[{ token = \"$t\", format = \"ago\" }]]\n").unwrap();
        let without = sidebar("[agents]\nrows = [[\"$t\"]]\n").unwrap();
        let mut last_second = 0;
        assert!(sidebar_ago_changed(
            &with.agents,
            &with.spaces,
            &mut last_second
        ));
        assert!(last_second > 0);
        let mut untouched = 7;
        assert!(!sidebar_ago_changed(
            &without.agents,
            &without.spaces,
            &mut untouched
        ));
        assert_eq!(untouched, 7);

        let mut last_second = 100;
        assert!(!new_second(&mut last_second, 100));
        assert!(new_second(&mut last_second, 101));
        assert_eq!(last_second, 101);
        assert!(!new_second(&mut last_second, 101));
    }

    #[test]
    fn the_sidebar_uses_ago_only_while_a_row_has_it() {
        let uses = |toml: &str| {
            let config = sidebar(toml).unwrap();
            sidebar_uses_ago(&config.agents, &config.spaces)
        };
        assert!(!uses("[agents]\nrows = [[\"$t\"]]\n"));
        assert!(uses(
            "[agents]\nrows = [[{ token = \"$t\", format = \"ago\" }]]\n"
        ));
        assert!(uses(
            "[agents.rows_by_agent]\nclaude = [[{ token = \"$t\", format = \"ago\" }]]\n"
        ));
        assert!(uses(
            "[spaces]\nrows = [[{ token = \"$t\", format = \"ago\" }]]\n"
        ));
    }
}
