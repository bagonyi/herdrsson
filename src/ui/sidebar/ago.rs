//! Fork: `format = "ago"` on a custom sidebar token shows its value, a Unix time in
//! seconds, as the time since then: `10s ago` (in steps of 10 seconds), `5m ago`, `3h ago`,
//! `2d ago`. Rules match that text. A working agent's row shows `working` in its place
//! (`show_working`). `format = "left"` counts down to such a time instead: `5m left`, and
//! `0s left` in its last 10 seconds and once it has passed. `format = "bar"` shows a value of two such times,
//! `FROM/UNTIL`, as the share of the time between them still left: `▰▰▰▰▱▱`, and `format =
//! "bar_left"` that bar with the time left after it: `▰▰▰▰▱▱ 38m`. The client repaints each
//! second while a row uses any of them (`ClientShellState::tick_ago`).

use std::time::{SystemTime, UNIX_EPOCH};

use super::tokens::{ResolvedToken, ResolvedTokenKind};
use crate::config::{AgentsSidebarConfig, SidebarTokenFormat, SpacesSidebarConfig};

/// A custom token's value as its row shows it. A value its format can't read shows as it is.
pub(super) fn custom_value(value: &str, format: Option<SidebarTokenFormat>) -> String {
    match format {
        Some(SidebarTokenFormat::Ago) => {
            ago(value, unix_now()).unwrap_or_else(|| value.to_string())
        }
        Some(SidebarTokenFormat::Left) => {
            left(value, unix_now()).unwrap_or_else(|| value.to_string())
        }
        Some(SidebarTokenFormat::Bar) => {
            bar(value, unix_now()).unwrap_or_else(|| value.to_string())
        }
        Some(SidebarTokenFormat::BarLeft) => {
            bar_left(value, unix_now()).unwrap_or_else(|| value.to_string())
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
    Some(format!("{} ago", duration(seconds)))
}

/// The time until `value`, in the steps `ago` uses; a time that has passed counts as `0s left`.
fn left(value: &str, now: u64) -> Option<String> {
    let seconds = value.parse::<u64>().ok()?.saturating_sub(now);
    Some(format!("{} left", duration(seconds)))
}

/// A `bar`'s segments.
const BAR_SEGMENTS: u64 = 6;

/// The time left of `FROM/UNTIL` as `BAR_SEGMENTS` segments, `▰` for each one not yet over, so
/// any time left shows at least one: full until `FROM`, empty from `UNTIL` on.
fn bar(value: &str, now: u64) -> Option<String> {
    bar_and_left(value, now).map(|(bar, _)| bar)
}

/// The `bar` of `FROM/UNTIL` with the time left after it, in the steps `left` uses:
/// `▰▰▰▱▱▱ 28m`. Once `UNTIL` has passed only the empty bar shows, for rules to match.
fn bar_left(value: &str, now: u64) -> Option<String> {
    let (bar, left) = bar_and_left(value, now)?;
    Some(if left == 0 {
        bar
    } else {
        format!("{bar} {}", duration(left))
    })
}

/// A `bar` and the seconds it shows as left, never more than the span.
fn bar_and_left(value: &str, now: u64) -> Option<(String, u64)> {
    let (from, until) = value.split_once('/')?;
    let (from, until) = (from.parse::<u64>().ok()?, until.parse::<u64>().ok()?);
    let span = until.checked_sub(from).filter(|span| *span > 0)?;
    let left = until.saturating_sub(now).min(span);
    // In u128, as `left * BAR_SEGMENTS` overflows u64 for a span over a sixth of its range.
    let full = (u128::from(left) * u128::from(BAR_SEGMENTS)).div_ceil(u128::from(span));
    let full = usize::try_from(full)
        .unwrap_or(0)
        .min(BAR_SEGMENTS as usize);
    Some((
        "▰".repeat(full) + &"▱".repeat(BAR_SEGMENTS as usize - full),
        left,
    ))
}

fn duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{}s", seconds / 10 * 10),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
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
/// `left`, `bar` or `bar_left`, so the sidebar needs a repaint. Updates `last_second`.
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

/// Whether an agent or space row shows a token as `ago`, `left`, `bar` or `bar_left`, so the
/// sidebar changes as time passes.
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
    fn left_counts_down_in_the_same_steps_and_stops_at_zero() {
        let now = 10_000_000;
        for (until, text) in [
            (0, "0s left"),
            (9, "0s left"),
            (10, "10s left"),
            (59, "50s left"),
            (60, "1m left"),
            (3_599, "59m left"),
            (3_600, "1h left"),
            (86_400, "1d left"),
        ] {
            assert_eq!(left(&(now + until).to_string(), now).as_deref(), Some(text));
        }
        assert_eq!(
            left(&(now - 30).to_string(), now).as_deref(),
            Some("0s left")
        );
    }

    #[test]
    fn bar_shows_a_segment_for_each_sixth_of_the_time_not_yet_over() {
        let (from, until) = (10_000_000, 10_003_600);
        let value = format!("{from}/{until}");
        for (now, text) in [
            (from - 100, "▰▰▰▰▰▰"),
            (from, "▰▰▰▰▰▰"),
            (from + 599, "▰▰▰▰▰▰"),
            (from + 600, "▰▰▰▰▰▱"),
            (from + 1_800, "▰▰▰▱▱▱"),
            (until - 1, "▰▱▱▱▱▱"),
            (until, "▱▱▱▱▱▱"),
            (until + 100, "▱▱▱▱▱▱"),
        ] {
            assert_eq!(bar(&value, now).as_deref(), Some(text), "at {now}");
        }
    }

    #[test]
    fn bar_left_puts_the_time_left_after_the_bar_and_only_the_empty_bar_once_over() {
        let (from, until) = (10_000_000, 10_003_600);
        let value = format!("{from}/{until}");
        for (now, text) in [
            (from - 100, "▰▰▰▰▰▰ 1h"),
            (from + 1, "▰▰▰▰▰▰ 59m"),
            (from + 1_800, "▰▰▰▱▱▱ 30m"),
            (until - 241, "▰▱▱▱▱▱ 4m"),
            (until - 45, "▰▱▱▱▱▱ 40s"),
            (until - 1, "▰▱▱▱▱▱ 0s"),
            (until, "▱▱▱▱▱▱"),
            (until + 100, "▱▱▱▱▱▱"),
        ] {
            assert_eq!(bar_left(&value, now).as_deref(), Some(text), "at {now}");
        }
        assert_eq!(bar_left("20/10", 15), None);
    }

    #[test]
    fn a_bar_over_a_huge_span_does_not_overflow() {
        let now = 1_793_000_000;
        assert_eq!(
            bar(&format!("0/{}", u64::MAX), now).as_deref(),
            Some("▰▰▰▰▰▰")
        );
        assert_eq!(
            bar(&format!("{}/{}", u64::MAX - 600, u64::MAX), u64::MAX - 1).as_deref(),
            Some("▰▱▱▱▱▱")
        );
    }

    #[test]
    fn a_bar_needs_two_unix_times_the_second_one_later() {
        for value in [
            "10/10", "20/10", "10", "10/", "/10", "a/b", "10-20", "10/20/30",
        ] {
            assert_eq!(bar(value, 15), None, "{value}");
        }
    }

    #[test]
    fn a_value_that_is_not_a_unix_time_shows_as_it_is() {
        for value in ["soon", "", "-5", "12.5", " 12"] {
            assert_eq!(custom_value(value, Some(SidebarTokenFormat::Ago)), value);
            assert_eq!(custom_value(value, Some(SidebarTokenFormat::Left)), value);
            assert_eq!(custom_value(value, Some(SidebarTokenFormat::Bar)), value);
            assert_eq!(
                custom_value(value, Some(SidebarTokenFormat::BarLeft)),
                value
            );
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
    fn agent_rows_show_the_time_left_and_rules_match_that_text() {
        let config = sidebar(
            r##"
[agents]
rows = [[{ token = "$cache_until", format = "left", rules = [{ contains = "s left", fg = "#ff0000" }] }]]
"##,
        )
        .unwrap();
        let tokens = HashMap::from([(
            "cache_until".to_string(),
            (unix_now() + 42 * 60 + 30).to_string(),
        )]);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(
            rows[0][0].kind,
            ResolvedTokenKind::Custom("42m left".into())
        );
        assert_eq!(rows[0][0].style.fg, None);

        let tokens = HashMap::from([("cache_until".to_string(), (unix_now() + 25).to_string())]);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert!(
            matches!(&rows[0][0].kind, ResolvedTokenKind::Custom(text) if text.ends_with("s left"))
        );
        assert!(rows[0][0].style.fg.is_some());
    }

    #[test]
    fn agent_rows_show_a_bar_and_rules_match_its_segments() {
        let config = sidebar(
            r##"
[agents]
rows = [[{ token = "$cache_span", format = "bar", rules = [{ equals = "▰▱▱▱▱▱", fg = "#ff9e64" }] }]]
"##,
        )
        .unwrap();
        let now = unix_now();
        let span = |from: u64, until: u64| {
            HashMap::from([("cache_span".to_string(), format!("{from}/{until}"))])
        };

        let tokens = span(now - 60, now + 3_540);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom("▰▰▰▰▰▰".into()));
        assert_eq!(rows[0][0].style.fg, None);

        let tokens = span(now - 3_300, now + 300);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom("▰▱▱▱▱▱".into()));
        assert!(rows[0][0].style.fg.is_some());
    }

    #[test]
    fn an_expired_bar_can_be_struck_through_by_a_rule() {
        use ratatui::style::{Modifier, Style};
        let config = sidebar(
            r##"
[agents]
rows = [[{ token = "$cache_span", format = "bar", rules = [{ equals = "▱▱▱▱▱▱", dim = true, strikethrough = true }] }]]
"##,
        )
        .unwrap();
        let now = unix_now();
        let tokens = HashMap::from([(
            "cache_span".to_string(),
            format!("{}/{}", now - 7_200, now - 3_600),
        )]);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom("▱▱▱▱▱▱".into()));
        assert_eq!(rows[0][0].style.strikethrough, Some(true));

        let plain = Style::default();
        let spans = super::super::resolved_token_spans(
            &rows[0],
            ("*", plain),
            plain,
            plain,
            plain,
            plain,
            &super::super::Palette::catppuccin(),
            40,
        );
        assert!(spans[0]
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT | Modifier::DIM));

        let tokens = HashMap::from([("cache_span".to_string(), format!("{now}/{}", now + 3_600))]);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(rows[0][0].style.strikethrough, None);
    }

    #[test]
    fn a_rule_can_show_other_text_in_place_of_the_value() {
        let config = sidebar(
            r##"
[agents]
rows = [[{ token = "$cache_span", format = "bar_left", rules = [
  { starts_with = "▰▱▱▱▱▱", fg = "#ff9e64" },
  { equals = "▱▱▱▱▱▱", text = "Cache: expired", dim = true },
] }]]
[spaces]
rows = [[{ token = "$cache_span", format = "bar_left", rules = [{ equals = "▱▱▱▱▱▱", text = "cold" }] }]]
"##,
        )
        .unwrap();
        let now = unix_now();
        let span = |from: u64, until: u64| {
            HashMap::from([("cache_span".to_string(), format!("{from}/{until}"))])
        };

        let tokens = span(now - 3_300, now + 299);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(
            rows[0][0].kind,
            ResolvedTokenKind::Custom("▰▱▱▱▱▱ 4m".into())
        );
        assert!(rows[0][0].style.fg.is_some());
        assert_eq!(rows[0][0].style.dim, None);

        let tokens = span(now - 7_200, now - 3_600);
        let rows = agent_rows(&config.agents, agent_context(&tokens), "idle");
        assert_eq!(
            rows[0][0].kind,
            ResolvedTokenKind::Custom("Cache: expired".into())
        );
        assert_eq!(rows[0][0].style.dim, Some(true));
        assert_eq!(rows[0][0].style.fg, None);

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
        assert_eq!(rows[0][0].kind, ResolvedTokenKind::Custom("cold".into()));
    }

    #[test]
    fn rule_text_round_trips() {
        let config = sidebar(
            "[agents]\nrows = [[{ token = \"$t\", rules = [{ equals = \"x\", text = \"y\" }] }]]\n",
        )
        .unwrap();
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("text = \"y\""), "{text}");
        assert_eq!(sidebar(&text).unwrap(), config);
    }

    #[test]
    fn strikethrough_round_trips_on_tokens_and_rules() {
        let config = sidebar(
            "[agents]\nrows = [[{ token = \"$t\", strikethrough = true, rules = [{ equals = \"x\", strikethrough = false }] }]]\n",
        )
        .unwrap();
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("strikethrough = true"), "{text}");
        assert!(text.contains("strikethrough = false"), "{text}");
        assert_eq!(sidebar(&text).unwrap(), config);
    }

    #[test]
    fn a_working_agent_shows_working_in_place_of_its_ago_times_only() {
        let config = sidebar(
            "[agents]\nrows = [[\"workspace\", \"$note\", { token = \"$last_at\", format = \"ago\" }, { token = \"$until\", format = \"left\" }]]\n",
        )
        .unwrap();
        let tokens = HashMap::from([
            ("note".to_string(), "1700000000".to_string()),
            ("last_at".to_string(), (unix_now() - 120).to_string()),
            (
                "until".to_string(),
                (unix_now() + 3 * 3_600 + 5).to_string(),
            ),
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
        assert_eq!(rows[0][3].kind, ResolvedTokenKind::Custom("3h left".into()));
    }

    #[test]
    fn format_applies_only_to_custom_tokens_and_takes_only_ago_left_or_bar() {
        for format in ["ago", "left", "bar", "bar_left"] {
            assert!(sidebar(&format!(
                "[agents]\nrows = [[{{ token = \"tab\", format = \"{format}\" }}]]\n"
            ))
            .is_err());
        }
        assert!(sidebar("[agents]\nrows = [[{ token = \"$t\", format = \"since\" }]]\n").is_err());
    }

    #[test]
    fn format_round_trips_through_serialization() {
        for format in ["ago", "left", "bar", "bar_left"] {
            let config = sidebar(&format!(
                "[agents]\nrows = [[{{ token = \"$t\", format = \"{format}\" }}]]\n"
            ))
            .unwrap();
            let text = toml::to_string(&config).unwrap();
            assert!(text.contains(&format!("format = \"{format}\"")), "{text}");
            assert_eq!(sidebar(&text).unwrap(), config);
        }
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
        assert!(uses(
            "[agents]\nrows = [[{ token = \"$t\", format = \"left\" }]]\n"
        ));
        assert!(uses(
            "[agents]\nrows = [[{ token = \"$t\", format = \"bar\" }]]\n"
        ));
        assert!(uses(
            "[agents]\nrows = [[{ token = \"$t\", format = \"bar_left\" }]]\n"
        ));
    }
}
