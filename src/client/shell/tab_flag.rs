//! Fork: a flag on a tab to come back to (`keys.flag_tab`). It is `⚑ ` at the start of the
//! tab's name, so it survives restarts and needs nothing new from the server; the tab bar and
//! the agents panel draw it as a mark in its own colour, after the tab's status mark.

use super::*;
use crate::input::KeybindAction;

const FLAG: &str = "⚑";
const FLAG_PREFIX: &str = "⚑ ";

/// The flag's mark and colour, if `label` is flagged.
pub(super) fn flag_mark(
    label: &str,
    palette: &crate::app::state::Palette,
) -> Option<(&'static str, ratatui::style::Color)> {
    label
        .starts_with(FLAG_PREFIX)
        .then_some((FLAG, palette.red))
}

/// `label` without its flag.
pub(super) fn unflagged(label: &str) -> &str {
    label.strip_prefix(FLAG_PREFIX).unwrap_or(label)
}

impl ClientShellState {
    /// `keys.flag_tab`: flags the tab in front of you, or takes its flag off.
    pub(super) fn handle_flag_tab(
        &mut self,
        action: KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if action != KeybindAction::FlagTab {
            return false;
        }
        let Some((tab_id, label)) = self.snapshot.as_deref().and_then(|snapshot| {
            let focused = snapshot.focused_tab_id.as_deref()?;
            let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == focused)?;
            let label = match tab.label.strip_prefix(FLAG_PREFIX) {
                Some(name) => name.to_owned(),
                None => format!("{FLAG_PREFIX}{}", tab.label),
            };
            Some((tab.tab_id.clone(), label))
        }) else {
            return true;
        };
        self.push_rename(
            crate::api::schema::Method::TabRename(crate::api::schema::TabRenameParams {
                tab_id,
                label,
            }),
            outcome,
        );
        outcome.repaint = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{snapshot, surface};
    use super::*;
    use crate::api::schema::{AgentStatus, Method};
    use crate::config::Config;

    fn flag(state: &mut ClientShellState) -> String {
        let mut outcome = ClientShellInput::default();
        state.record_binding(
            crate::input::KeybindMatch::Action(KeybindAction::FlagTab),
            &mut outcome,
        );
        match &outcome.actions[..] {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                Method::TabRename(rename) => {
                    assert_eq!(rename.tab_id, "tab_1");
                    rename.label.clone()
                }
                method => panic!("expected a tab rename, got {method:?}"),
            },
            actions => panic!("expected one endpoint request, got {actions:?}"),
        }
    }

    #[test]
    fn the_key_flags_the_focused_tab_and_takes_the_flag_off_again() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut projected = snapshot();
        for (label, renamed) in [
            ("review", "⚑ review"),
            ("⚑ review", "review"),
            ("1", "⚑ 1"),
            ("", "⚑ "),
            ("⚑ ", ""),
            ("⚑review", "⚑ ⚑review"),
        ] {
            projected.tabs[0].label = label.into();
            state.set_snapshot(Box::new(projected.clone()));
            assert_eq!(flag(&mut state), renamed, "{label:?}");
        }
    }

    #[test]
    fn a_flag_is_drawn_as_a_mark_after_the_status_mark() {
        let mut projected = snapshot();
        projected.tabs[0].label = "⚑ review".into();
        projected.tabs[0].custom_label = true;
        projected.tabs[0].agent_status = AgentStatus::Blocked;
        let config = ClientShellConfig::from_config(&Config::default());
        let palette = config.palette.clone();
        let color = crate::protocol::color_to_u32;
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let mut tab_bar = |snapshot: &ClientShellSnapshot| {
            state.set_snapshot(Box::new(snapshot.clone()));
            state.set_pane_surface(surface());
            let frame = state.compose(80, 20).expect("tab bar");
            let rect = state.hits.tabs[0].0;
            (rect.x..rect.right())
                .map(|x| &frame.cells[(rect.y * frame.width + x) as usize])
                .filter(|cell| cell.symbol != " ")
                .map(|cell| (cell.symbol.clone(), cell.fg))
                .collect::<Vec<_>>()
        };
        let drawn = tab_bar(&projected);
        assert_eq!(drawn[0], ("●".into(), color(palette.red)));
        assert_eq!(drawn[1], ("⚑".into(), color(palette.red)));
        assert_eq!(
            drawn[2..]
                .iter()
                .map(|(symbol, _)| symbol.as_str())
                .collect::<String>(),
            "review"
        );
        projected.tabs[0].agent_status = AgentStatus::Idle;
        assert_eq!(tab_bar(&projected)[0], ("⚑".into(), color(palette.red)));

        // The agents panel's tab name gets the same marks.
        projected.tabs[0].agent_status = AgentStatus::Blocked;
        projected.agents = vec![crate::protocol::ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: None,
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Blocked,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        }];
        let row = super::super::agent_sidebar::agent_row(&projected, "pane_1", &config, None)
            .expect("agent row");
        let spans = crate::ui::resolved_token_spans(
            &row.rows[0],
            ("●", Style::default().fg(palette.red)),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            &palette,
            80,
        );
        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "● client-shell · ● ⚑ review"
        );
        assert!(spans
            .iter()
            .any(|span| span.content == "⚑" && span.style.fg == Some(palette.red)));
    }
}
