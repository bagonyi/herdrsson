//! Fork: the agent switcher behind `keys.agent_switcher` (e.g. `alt+tab`), as Cmd+Tab switches
//! apps: a popup listing the agents, the one in front first, then the latest visited. Held with a
//! modifier, each press of the key moves down the list (with Shift, up), and letting go of the
//! modifier switches to the highlighted agent. Enter or a click switches too, Esc cancels. Each
//! agent is drawn as the agents panel draws it.

use super::*;
use crate::input::{KeybindAction, KeybindMatch};
use crossterm::event::{
    KeyEventKind, KeyModifiers, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

/// Enough history to order every agent a window is likely to have.
const AGENT_VISIT_LIMIT: usize = 256;
/// The popup's width at most, its frame included.
const SWITCHER_WIDTH: u16 = 64;

/// An agent's pane and its machine: each server numbers its panes from the same start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentTarget {
    endpoint_id: ClientEndpointId,
    pane_id: String,
}

/// The agents the window has focused, most recent first. A window keeps its own, from when it
/// opened.
#[derive(Default)]
pub(super) struct AgentVisitHistory {
    entries: Vec<AgentTarget>,
}

/// The open switcher.
pub(super) struct AgentSwitcher {
    selected: AgentTarget,
    /// Where `selected` was, for when its agent goes while the switcher is open.
    selected_index: usize,
    /// The modifiers held with the key that opened it: letting go of them switches. Empty when
    /// it opened otherwise (after the prefix key, from the help), and it then stays open until
    /// Enter, a click or Esc.
    hold: KeyModifiers,
    scroll: usize,
    area: Rect,
    row_hits: Vec<(Rect, usize, AgentTarget)>,
}

struct SwitcherEntry {
    target: AgentTarget,
    row: super::agent_sidebar::AgentRow,
}

impl ClientShellState {
    /// Moves the focused pane to the front of the agent history when an agent runs in it.
    pub(super) fn note_agent_visit(&mut self) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(pane_id) = snapshot.focused_pane_id.as_deref() else {
            return;
        };
        if !snapshot.agents.iter().any(|agent| agent.pane_id == pane_id) {
            return;
        }
        let visit = AgentTarget {
            endpoint_id: self.active_endpoint_id.clone(),
            pane_id: pane_id.to_owned(),
        };
        let entries = &mut self.agent_visits.entries;
        if entries.first() == Some(&visit) {
            return;
        }
        entries.retain(|entry| entry != &visit);
        entries.insert(0, visit);
        entries.truncate(AGENT_VISIT_LIMIT);
    }

    /// The agents in the switcher's order: the focused one, then those visited latest first,
    /// then the rest in the agents panel's order. Machines not online are left out.
    fn agent_switcher_entries(&self) -> Vec<SwitcherEntry> {
        let mut entries = if self.multi_endpoint_active() {
            super::endpoint_agents::agent_rows(
                &self.endpoints,
                &self.active_endpoint_id,
                &self.config,
            )
            .into_iter()
            .filter(|row| !row.stale)
            .map(|row| SwitcherEntry {
                target: AgentTarget {
                    endpoint_id: row.endpoint_id,
                    pane_id: row.agent.pane_id.clone(),
                },
                row: row.agent,
            })
            .collect::<Vec<_>>()
        } else {
            let Some(snapshot) = self.snapshot.as_deref() else {
                return Vec::new();
            };
            super::agent_sidebar::agent_rows(snapshot, &self.config, None)
                .into_iter()
                .map(|row| SwitcherEntry {
                    target: AgentTarget {
                        endpoint_id: self.active_endpoint_id.clone(),
                        pane_id: row.pane_id.clone(),
                    },
                    row,
                })
                .collect()
        };
        // A stable sort, so the agents never visited keep the panel's order.
        entries.sort_by_key(|entry| {
            if entry.row.focused {
                return 0;
            }
            self.agent_visits
                .entries
                .iter()
                .position(|visit| visit == &entry.target)
                .map_or(usize::MAX, |index| index + 1)
        });
        entries
    }

    /// Opens on the agent before the focused one, or on the latest visited when the focused pane
    /// runs none, so a quick press and release goes back to the previous agent. `backwards`
    /// (the key with Shift) opens on the last agent instead.
    fn open_agent_switcher(
        &mut self,
        hold: KeyModifiers,
        backwards: bool,
        outcome: &mut ClientShellInput,
    ) {
        let entries = self.agent_switcher_entries();
        let Some(first) = entries.first() else {
            return;
        };
        let index = if backwards {
            entries.len() - 1
        } else {
            usize::from(first.row.focused) % entries.len()
        };
        self.agent_switcher = Some(AgentSwitcher {
            selected: entries[index].target.clone(),
            selected_index: index,
            hold,
            scroll: 0,
            area: Rect::default(),
            row_hits: Vec::new(),
        });
        outcome.repaint = true;
    }

    fn step_agent_switcher(&mut self, delta: isize, outcome: &mut ClientShellInput) {
        let entries = self.agent_switcher_entries();
        let Some(switcher) = self.agent_switcher.as_mut() else {
            return;
        };
        if entries.is_empty() {
            return;
        }
        let current = selected_position(&entries, switcher);
        let next = (current as isize + delta).rem_euclid(entries.len() as isize) as usize;
        switcher.selected = entries[next].target.clone();
        switcher.selected_index = next;
        outcome.repaint = true;
    }

    /// Closes the switcher and focuses the highlighted agent, revealing it in the agents panel.
    fn commit_agent_switcher(&mut self, outcome: &mut ClientShellInput) {
        let entries = self.agent_switcher_entries();
        let Some(switcher) = self.agent_switcher.take() else {
            return;
        };
        outcome.repaint = true;
        if entries.is_empty() {
            return;
        }
        let entry = &entries[selected_position(&entries, &switcher)];
        if entry.row.focused {
            return;
        }
        let AgentTarget {
            endpoint_id,
            pane_id,
        } = entry.target.clone();
        if !self.focus_or_activate(
            endpoint_id.clone(),
            ClientEndpointFocusTarget::Pane(pane_id.clone()),
            outcome,
        ) {
            return;
        }
        if endpoint_id != self.active_endpoint_id {
            self.pending_agent_reveal = Some((endpoint_id, pane_id));
        } else if self.multi_endpoint_active() {
            self.reveal_endpoint_agent(&endpoint_id, &pane_id, self.hits.agent_body.height);
        } else if let Some(snapshot) = self.snapshot.as_deref() {
            // As the next agent key does.
            let index = super::agent_sidebar::ordered_agent_pane_ids(
                snapshot,
                self.config.agent_panel_sort,
            )
            .iter()
            .position(|candidate| candidate == &pane_id);
            if let Some(index) = index {
                if !self
                    .hits
                    .agents
                    .iter()
                    .any(|(_, visible_pane_id)| visible_pane_id == &pane_id)
                {
                    self.agent_scroll = index.min(self.hits.agent_max_scroll);
                }
            }
        }
    }

    /// Closes the switcher without switching, when the window loses focus.
    pub(super) fn cancel_agent_switcher(&mut self) -> bool {
        self.agent_switcher.take().is_some()
    }

    /// The agent switcher's key opened by another way than its own held key: after the prefix
    /// key, or from the help. It stays open until Enter, a click or Esc.
    pub(super) fn handle_agent_switcher(
        &mut self,
        action: KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if action != KeybindAction::AgentSwitcher {
            return false;
        }
        if self.agent_switcher.is_none() {
            self.open_agent_switcher(KeyModifiers::empty(), false, outcome);
        }
        true
    }

    /// Key presses (and repeats) while the switcher is open, and its own key, which opens it.
    /// True when the key was the switcher's.
    pub(super) fn route_agent_switcher_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(hold) = self.agent_switcher.as_ref().map(|switcher| switcher.hold) else {
            return self.open_agent_switcher_on_key(key, outcome);
        };
        match key.code {
            KeyCode::Modifier(_) => {}
            KeyCode::Esc => {
                self.agent_switcher = None;
                outcome.repaint = true;
            }
            KeyCode::Enter => self.commit_agent_switcher(outcome),
            KeyCode::BackTab | KeyCode::Up => self.step_agent_switcher(-1, outcome),
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.step_agent_switcher(-1, outcome)
            }
            KeyCode::Tab | KeyCode::Down => self.step_agent_switcher(1, outcome),
            // The modifier was let go without its release reaching the window (a terminal that
            // doesn't report releases): the next key switches.
            _ if !hold.is_empty() && !key.modifiers.intersects(hold) => {
                self.commit_agent_switcher(outcome)
            }
            _ => {}
        }
        true
    }

    /// Its own key, held with a modifier, opens the switcher in the terminal, as a direct key
    /// binding would, remembering the modifier. A Tab key also opens it with Shift added
    /// (alt+shift+tab for alt+tab), going backwards, as Cmd+Shift+Tab does, rather than reaching
    /// the pane as Shift+Tab.
    fn open_agent_switcher_on_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if key.kind != KeyEventKind::Press
            || self.mode != ClientShellMode::Terminal
            || self.overlay.is_some()
            || self.popup_pending
            || self.popup_terminal_id.is_some()
        {
            return false;
        }
        let binds = |key: &crate::input::TerminalKey| {
            matches!(
                crate::input::resolve_direct_binding(&self.config.keybinds.keybinds, key),
                Some(KeybindMatch::Action(KeybindAction::AgentSwitcher))
            )
        };
        let hold = key.modifiers.difference(KeyModifiers::SHIFT);
        let backwards = if binds(key) {
            false
        } else if !hold.is_empty()
            && (key.code == KeyCode::BackTab
                || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT)))
            && binds(&crate::input::TerminalKey::new(KeyCode::Tab, hold))
        {
            true
        } else {
            return false;
        };
        self.open_agent_switcher(hold, backwards, outcome);
        true
    }

    /// A key let go while the switcher is open: letting go of its modifier switches. Its own
    /// release arrives while the host reports every key (asked for while the switcher is open,
    /// see `host_keyboard_report_all_requested`); any other key reported without it means it was
    /// let go too.
    pub(super) fn agent_switcher_key_release(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let Some(hold) = self
            .agent_switcher
            .as_ref()
            .map(|switcher| switcher.hold)
            .filter(|hold| !hold.is_empty())
        else {
            return;
        };
        let let_go = match key.code {
            KeyCode::Modifier(modifier) => {
                modifier_flag(modifier).is_some_and(|flag| hold.intersects(flag))
            }
            _ => !key.modifiers.intersects(hold),
        };
        if let_go {
            self.commit_agent_switcher(outcome);
        }
    }

    /// The mouse while the switcher is open: a click on an agent switches to it, one outside
    /// closes the switcher, pointing at an agent highlights it and the wheel moves the
    /// highlight. True when the switcher took the event.
    pub(super) fn handle_agent_switcher_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(switcher) = self.agent_switcher.as_mut() else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        let hit = switcher
            .row_hits
            .iter()
            .find(|(rect, _, _)| super::contains(*rect, point))
            .map(|(_, index, target)| (*index, target.clone()));
        match mouse.kind {
            MouseEventKind::Down(button) => match hit {
                Some((index, target)) if button == MouseButton::Left => {
                    switcher.selected = target;
                    switcher.selected_index = index;
                    self.commit_agent_switcher(outcome);
                }
                Some(_) => {}
                None if !super::contains(switcher.area, point) => {
                    self.agent_switcher = None;
                    outcome.repaint = true;
                }
                None => {}
            },
            MouseEventKind::Moved => {
                if let Some((index, target)) =
                    hit.filter(|(_, target)| target != &switcher.selected)
                {
                    switcher.selected = target;
                    switcher.selected_index = index;
                    outcome.repaint = true;
                }
            }
            MouseEventKind::ScrollUp => self.step_agent_switcher(-1, outcome),
            MouseEventKind::ScrollDown => self.step_agent_switcher(1, outcome),
            _ => {}
        }
        true
    }

    /// Draws the switcher in the middle of the window: each agent as the agents panel draws it,
    /// the highlighted one in the Navigate cursor's colour with an accent bar at its left.
    /// Returns the area it covers.
    pub(super) fn render_agent_switcher(&mut self, buffer: &mut Buffer) -> Option<Rect> {
        let mut entries = self.agent_switcher_entries();
        let config = &self.config;
        let switcher = self.agent_switcher.as_mut()?;
        switcher.area = Rect::default();
        switcher.row_hits.clear();
        if entries.is_empty() {
            return None;
        }
        let palette = &config.palette;
        let heights = entries
            .iter()
            .map(|entry| entry.row.rows.len().clamp(1, usize::from(u16::MAX)) as u16)
            .collect::<Vec<_>>();
        let older = entries
            .iter()
            .map(|entry| entry.row.older)
            .collect::<Vec<_>>();
        let (gaps, divider_after) = super::agent_sidebar::agent_list_gaps(&older, &config.agents);
        let content = heights
            .iter()
            .chain(&gaps)
            .fold(0u16, |total, lines| total.saturating_add(*lines));
        let window = buffer.area;
        let width = window.width.saturating_sub(4).min(SWITCHER_WIDTH);
        let height = content
            .saturating_add(2)
            .min(window.height.saturating_sub(2));
        if width < 12 || height < 3 {
            return None;
        }
        let area = Rect::new(
            window.x + (window.width - width) / 2,
            window.y + (window.height - height) / 2,
            width,
            height,
        );
        draw_frame(buffer, area, palette);
        put_text(
            buffer,
            area.x + 2,
            area.y,
            area.width.saturating_sub(4),
            " agents ",
            Style::default().fg(palette.accent).bg(palette.panel_bg),
        );
        let hint = if switcher.hold.is_empty() {
            " tab next · enter switch · esc cancel ".to_owned()
        } else {
            format!(
                " tab next · release {} to switch · esc cancel ",
                hold_label(switcher.hold)
            )
        };
        if display_width(&hint) + 4 <= area.width {
            put_text(
                buffer,
                area.x + 2,
                area.bottom() - 1,
                area.width - 4,
                &hint,
                Style::default().fg(palette.overlay0).bg(palette.panel_bg),
            );
        }

        let body = Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2);
        let selected = selected_position(&entries, switcher);
        switcher.scroll = super::scroll::list_scroll_start_to_reveal(
            &heights,
            &gaps,
            body.height,
            switcher.scroll,
            selected,
        );
        let metrics =
            super::scroll::list_scroll_metrics(&heights, &gaps, body.height, switcher.scroll);
        switcher.scroll = metrics
            .max_offset_from_bottom
            .saturating_sub(metrics.offset_from_bottom);
        let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
        let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
        let selection_bg = if palette.selection_bg == ratatui::style::Color::Reset {
            palette.active_row_bg
        } else {
            palette.selection_bg
        };
        let mut y = body.y;
        for (index, entry) in entries.iter_mut().enumerate().skip(switcher.scroll) {
            let height = heights[index].min(body.height);
            if y.saturating_add(height) > body.bottom() {
                break;
            }
            let rect = Rect::new(body.x, y, content_width, height);
            if index == selected {
                // Drawn as the focused row (brighter names), then given the cursor's colour.
                entry.row.focused = true;
                super::agent_sidebar::render_agent_row(buffer, rect, &entry.row, config);
                buffer.set_style(rect, Style::default().bg(selection_bg));
                for line in rect.y..rect.bottom() {
                    put_text(
                        buffer,
                        rect.x,
                        line,
                        1,
                        "▌",
                        Style::default().fg(palette.accent).bg(selection_bg),
                    );
                }
            } else {
                super::agent_sidebar::render_agent_row(buffer, rect, &entry.row, config);
            }
            switcher.row_hits.push((rect, index, entry.target.clone()));
            y = y.saturating_add(height);
            if divider_after == Some(index) {
                let divider_y = y.saturating_add(config.agents.row_gap.div_ceil(2));
                if divider_y < body.bottom() {
                    super::agent_sidebar::render_older_divider(
                        buffer,
                        body.x,
                        divider_y,
                        content_width,
                        config,
                    );
                }
            }
            y = y.saturating_add(gaps[index]);
        }
        if show_scrollbar {
            let track = Rect::new(body.right() - 1, body.y, 1, body.height);
            super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
        }
        switcher.area = area;
        Some(area)
    }
}

fn selected_position(entries: &[SwitcherEntry], switcher: &AgentSwitcher) -> usize {
    entries
        .iter()
        .position(|entry| entry.target == switcher.selected)
        .unwrap_or_else(|| switcher.selected_index.min(entries.len().saturating_sub(1)))
}

/// The modifier a modifier key sets; Shift never holds the switcher.
fn modifier_flag(modifier: ModifierKeyCode) -> Option<KeyModifiers> {
    match modifier {
        ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => Some(KeyModifiers::ALT),
        ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl => Some(KeyModifiers::CONTROL),
        ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper => Some(KeyModifiers::SUPER),
        ModifierKeyCode::LeftHyper | ModifierKeyCode::RightHyper => Some(KeyModifiers::HYPER),
        ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta => Some(KeyModifiers::META),
        _ => None,
    }
}

fn hold_label(hold: KeyModifiers) -> &'static str {
    if hold.contains(KeyModifiers::ALT) {
        "alt"
    } else if hold.contains(KeyModifiers::CONTROL) {
        "ctrl"
    } else if hold.contains(KeyModifiers::SUPER) {
        "cmd"
    } else {
        "the key"
    }
}

/// The popup's background and frame, in the accent colour as Go to's. Cells are reset first, so
/// nothing of the pane under it (underlines, dimming) shows through.
fn draw_frame(buffer: &mut Buffer, area: Rect, palette: &Palette) {
    let background = Style::default().bg(palette.panel_bg);
    let border = Style::default().fg(palette.accent).bg(palette.panel_bg);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let (top, bottom) = (y == area.y, y + 1 == area.bottom());
            let (left, right) = (x == area.x, x + 1 == area.right());
            let (symbol, style) = match (top, bottom, left, right) {
                (true, _, true, _) => ("┌", border),
                (true, _, _, true) => ("┐", border),
                (_, true, true, _) => ("└", border),
                (_, true, _, true) => ("┘", border),
                (true, ..) | (_, true, ..) => ("─", border),
                (_, _, true, _) | (_, _, _, true) => ("│", border),
                _ => (" ", background),
            };
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(symbol).set_style(style);
            }
        }
    }
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    super::render::put_text(buffer, x, y, width, text, style);
}

fn display_width(text: &str) -> u16 {
    super::render::display_width(text)
}

#[cfg(test)]
mod tests {
    use super::super::tests::snapshot;
    use super::*;
    use crate::api::schema::{AgentStatus, Method};
    use crate::config::Config;
    use crate::protocol::{ClientShellAgent, ClientShellPane};

    const ALT_TAB: &[u8] = b"\x1b[9;3u";
    const ALT_SHIFT_TAB: &[u8] = b"\x1b[9;4u";
    const LEFT_ALT_RELEASE: &[u8] = b"\x1b[57443;1:3u";

    fn state() -> ClientShellState {
        let config: Config = toml::from_str(
            "[keys]\nagent_switcher = \"alt+tab\"\n\
             [ui.sidebar.agents]\nrows = [[\"state_icon\", \"tab\"], [\"terminal_title_stripped\"]]\n",
        )
        .unwrap();
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.set_host_reports_key_releases(true);
        state
    }

    /// One space with a tab per pane: agents run in pane_1 to pane_3, a shell in pane_4.
    fn focused(revision: u64, pane: usize) -> Box<ClientShellSnapshot> {
        let mut snapshot = snapshot();
        let tab = snapshot.tabs[0].clone();
        let pane_template = snapshot.panes[0].clone();
        snapshot.revision = revision;
        snapshot.tabs = (1..=4)
            .map(|n| ClientShellTab {
                tab_id: format!("tab_{n}"),
                number: n,
                label: n.to_string(),
                focused: n == pane,
                ..tab.clone()
            })
            .collect();
        snapshot.panes = (1..=4)
            .map(|n| ClientShellPane {
                pane_id: format!("pane_{n}"),
                tab_id: format!("tab_{n}"),
                focused: n == pane,
                ..pane_template.clone()
            })
            .collect();
        snapshot.agents = (1..=3)
            .map(|n| ClientShellAgent {
                pane_id: format!("pane_{n}"),
                workspace_id: "ws_1".into(),
                tab_id: format!("tab_{n}"),
                name: None,
                display_agent: None,
                agent: Some("claude".into()),
                title: None,
                terminal_title: None,
                terminal_title_stripped: Some(format!("conversation {n}")),
                agent_status: AgentStatus::Idle,
                state_change_seq: 1,
                state_labels: Vec::new(),
                tokens: Vec::new(),
                focused: n == pane,
            })
            .collect();
        snapshot.focused_tab_id = Some(format!("tab_{pane}"));
        snapshot.focused_pane_id = Some(format!("pane_{pane}"));
        Box::new(snapshot)
    }

    fn visit(state: &mut ClientShellState, panes: &[usize]) {
        let revision = state
            .snapshot
            .as_deref()
            .map_or(0, |snapshot| snapshot.revision);
        for (offset, pane) in (1..).zip(panes) {
            state.set_snapshot(focused(revision + offset, *pane));
        }
    }

    fn order(state: &ClientShellState) -> Vec<String> {
        state
            .agent_switcher_entries()
            .into_iter()
            .map(|entry| entry.target.pane_id)
            .collect()
    }

    fn focused_panes(outcome: &ClientShellInput) -> Vec<String> {
        outcome
            .actions
            .iter()
            .filter_map(|action| match action {
                ClientShellAction::Endpoint { request, .. } => match &request.method {
                    Method::PaneFocus(target) => Some(target.pane_id.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn pane_keys(outcome: &ClientShellInput) -> usize {
        outcome
            .requests
            .iter()
            .filter(|request| matches!(request, ClientMessage::ClientShellPaneInput { .. }))
            .count()
    }

    fn selected(state: &ClientShellState) -> Option<&str> {
        state
            .agent_switcher
            .as_ref()
            .map(|switcher| switcher.selected.pane_id.as_str())
    }

    #[test]
    fn a_quick_press_goes_back_to_the_previous_agent_and_again_comes_back() {
        let mut state = state();
        visit(&mut state, &[1, 2, 3]);
        assert_eq!(order(&state), ["pane_3", "pane_2", "pane_1"]);

        let open = state.handle_input_bytes(ALT_TAB);
        assert_eq!(selected(&state), Some("pane_2"));
        assert!(state.host_keyboard_report_all_requested());
        assert_eq!(pane_keys(&open), 0);

        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(focused_panes(&release), ["pane_2"]);
        assert!(state.agent_switcher.is_none());
        assert!(!state.host_keyboard_report_all_requested());

        visit(&mut state, &[2]);
        state.handle_input_bytes(ALT_TAB);
        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(focused_panes(&release), ["pane_3"]);
    }

    #[test]
    fn tab_moves_down_shift_tab_back_and_both_wrap() {
        let mut state = state();
        visit(&mut state, &[1, 2, 3]);
        state.handle_input_bytes(ALT_TAB);
        state.handle_input_bytes(ALT_TAB);
        assert_eq!(selected(&state), Some("pane_1"));
        state.handle_input_bytes(ALT_TAB);
        assert_eq!(selected(&state), Some("pane_3"));
        state.handle_input_bytes(ALT_SHIFT_TAB);
        state.handle_input_bytes(ALT_SHIFT_TAB);
        assert_eq!(selected(&state), Some("pane_2"));
        state.handle_input_bytes(b"\x1b[1;3B"); // alt+down
        assert_eq!(selected(&state), Some("pane_1"));

        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(focused_panes(&release), ["pane_1"]);
    }

    #[test]
    fn alt_shift_tab_opens_backwards_and_never_reaches_the_pane() {
        let mut state = state();
        visit(&mut state, &[1, 2, 3]);
        let open = state.handle_input_bytes(ALT_SHIFT_TAB);
        assert_eq!(selected(&state), Some("pane_1"));
        assert_eq!(pane_keys(&open), 0);
        state.handle_input_bytes(ALT_SHIFT_TAB);
        assert_eq!(selected(&state), Some("pane_2"));
        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(focused_panes(&release), ["pane_2"]);
        assert_eq!(pane_keys(&release), 0);
    }

    #[test]
    fn the_highlight_stays_in_sight_while_stepping_through_a_long_list() {
        let mut state = state();
        let mut many = focused(1, 1);
        let agent = many.agents[0].clone();
        let tab = many.tabs[0].clone();
        let pane = many.panes[0].clone();
        many.tabs = (1..=10)
            .map(|n| ClientShellTab {
                tab_id: format!("tab_{n}"),
                number: n,
                label: n.to_string(),
                focused: n == 1,
                ..tab.clone()
            })
            .collect();
        many.panes = (1..=10)
            .map(|n| ClientShellPane {
                pane_id: format!("pane_{n}"),
                tab_id: format!("tab_{n}"),
                focused: n == 1,
                ..pane.clone()
            })
            .collect();
        many.agents = (1..=10)
            .map(|n| ClientShellAgent {
                pane_id: format!("pane_{n}"),
                tab_id: format!("tab_{n}"),
                terminal_title_stripped: Some(format!("conversation {n}")),
                focused: n == 1,
                ..agent.clone()
            })
            .collect();
        state.set_snapshot(many);
        state.handle_input_bytes(ALT_TAB);
        // Two lines a row: 10 agents need 20 lines, 8 fit.
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 12));
        let mut seen = Vec::new();
        for step in 0..25 {
            if step > 0 {
                state.handle_input_bytes(if step < 13 { ALT_TAB } else { ALT_SHIFT_TAB });
            }
            let area = state.render_agent_switcher(&mut buffer).unwrap();
            let switcher = state.agent_switcher.as_ref().unwrap();
            let (rect, _, target) = switcher
                .row_hits
                .iter()
                .find(|(_, _, target)| target == &switcher.selected)
                .unwrap_or_else(|| panic!("step {step}: the highlighted agent isn't drawn"));
            assert!(
                rect.y > area.y && rect.bottom() < area.bottom(),
                "step {step}"
            );
            assert_eq!(buffer[(rect.x, rect.y)].symbol(), "▌", "step {step}");
            seen.push(target.pane_id.clone());
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 10);
    }

    #[test]
    fn pane_output_while_open_redraws_the_window_instead_of_patching_over_the_popup() {
        let patch = |base: u64| {
            let mut pane = super::super::tests::surface().panes.remove(0);
            pane.content_revision = base + 1;
            crate::protocol::PaneSurfacePatch {
                boot_id: "boot-1".into(),
                projection_revision: 1,
                base_surface_revision: base,
                surface_revision: base + 1,
                rows: vec![crate::protocol::PaneSurfacePatchRow {
                    x: 0,
                    y: 0,
                    cells: vec![super::super::tests::surface().frame.cells[0].clone()],
                }],
                panes: vec![pane],
                cursor: None,
            }
        };
        let mut state = state();
        state.set_snapshot(focused(1, 1));
        state.set_pane_surface(super::super::tests::surface());
        state.handle_input_bytes(ALT_TAB);
        assert!(state.agent_switcher.is_some());
        state.compose(106, 20).unwrap();
        assert!(matches!(
            state.apply_pane_surface_patch(patch(1)),
            ClientPaneSurfacePatchOutcome::Applied(None)
        ));

        state.handle_input_bytes(b"\x1b[27;3u");
        assert!(state.agent_switcher.is_none());
        state.compose(106, 20).unwrap();
        assert!(matches!(
            state.apply_pane_surface_patch(patch(2)),
            ClientPaneSurfacePatchOutcome::Applied(Some(_))
        ));
    }

    #[test]
    fn the_agent_in_front_selected_again_switches_nowhere() {
        let mut state = state();
        visit(&mut state, &[1, 2]);
        state.handle_input_bytes(ALT_TAB);
        state.handle_input_bytes(ALT_TAB);
        state.handle_input_bytes(ALT_TAB);
        assert_eq!(selected(&state), Some("pane_2"));
        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert!(focused_panes(&release).is_empty());
        assert!(state.agent_switcher.is_none());
    }

    #[test]
    fn unvisited_agents_follow_in_panel_order_and_a_shell_in_front_selects_the_latest() {
        let mut state = state();
        visit(&mut state, &[2, 4]);
        assert_eq!(order(&state), ["pane_2", "pane_1", "pane_3"]);
        state.handle_input_bytes(ALT_TAB);
        assert_eq!(selected(&state), Some("pane_2"));
        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(focused_panes(&release), ["pane_2"]);
    }

    #[test]
    fn closed_agents_drop_out_of_the_order() {
        let mut state = state();
        visit(&mut state, &[1, 2, 3]);
        let mut closed = focused(10, 3);
        closed.agents.retain(|agent| agent.pane_id != "pane_2");
        state.set_snapshot(closed);
        assert_eq!(order(&state), ["pane_3", "pane_1"]);
    }

    #[test]
    fn esc_cancels_and_no_release_reaches_a_pane() {
        let mut state = state();
        visit(&mut state, &[1, 2]);
        state.handle_input_bytes(ALT_TAB);
        let esc = state.handle_input_bytes(b"\x1b[27;3u");
        assert!(state.agent_switcher.is_none());
        let tab_release = state.handle_input_bytes(b"\x1b[9;3:3u");
        let alt_release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        for outcome in [&esc, &tab_release, &alt_release] {
            assert!(focused_panes(outcome).is_empty());
            assert_eq!(pane_keys(outcome), 0);
        }
    }

    #[test]
    fn alt_let_go_before_tab_switches_and_tab_never_reaches_the_new_pane() {
        let mut state = state();
        visit(&mut state, &[1, 2]);
        state.handle_input_bytes(ALT_TAB);
        let tab_release = state.handle_input_bytes(b"\x1b[9;1:3u");
        assert_eq!(focused_panes(&tab_release), ["pane_1"]);
        assert_eq!(pane_keys(&tab_release), 0);
        visit(&mut state, &[1]);
        let alt_release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(pane_keys(&alt_release), 0);
        assert!(state.agent_switcher.is_none());
    }

    #[test]
    fn a_held_tab_repeating_after_the_switch_stays_out_of_the_new_pane() {
        let mut state = state();
        visit(&mut state, &[1, 2]);
        state.handle_input_bytes(ALT_TAB);
        let release = state.handle_input_bytes(LEFT_ALT_RELEASE);
        assert_eq!(focused_panes(&release), ["pane_1"]);
        visit(&mut state, &[1]);
        let repeat = state.handle_input_bytes(b"\x1b[9;1:2u");
        assert_eq!(pane_keys(&repeat), 0);
        assert!(state.agent_switcher.is_none());
    }

    #[test]
    fn without_release_reports_the_next_key_without_alt_switches() {
        let mut state = state();
        state.set_host_reports_key_releases(false);
        visit(&mut state, &[1, 2, 3]);
        state.handle_input_bytes(b"\x1b\t");
        state.handle_input_bytes(b"\x1b\t");
        assert_eq!(selected(&state), Some("pane_1"));
        let key = state.handle_input_bytes(b"x");
        assert_eq!(focused_panes(&key), ["pane_1"]);
        assert_eq!(pane_keys(&key), 0);
    }

    #[test]
    fn opened_without_its_held_key_it_stays_open_until_enter() {
        let mut state = state();
        visit(&mut state, &[1, 2, 3]);
        let mut outcome = ClientShellInput::default();
        state.record_binding(
            KeybindMatch::Action(KeybindAction::AgentSwitcher),
            &mut outcome,
        );
        assert_eq!(selected(&state), Some("pane_2"));
        state.handle_input_bytes(LEFT_ALT_RELEASE);
        state.handle_input_bytes(b"x");
        state.handle_input_bytes(b"\x1b[9u");
        assert_eq!(selected(&state), Some("pane_1"));
        let enter = state.handle_input_bytes(b"\r");
        assert_eq!(focused_panes(&enter), ["pane_1"]);
    }

    #[test]
    fn losing_focus_cancels() {
        let mut state = state();
        visit(&mut state, &[1, 2]);
        state.handle_input_bytes(ALT_TAB);
        let lost = state.handle_raw_events(vec![crate::raw_input::RawInputEvent::OuterFocusLost]);
        assert!(state.agent_switcher.is_none());
        assert!(focused_panes(&lost).is_empty());
    }

    #[test]
    fn draws_the_panel_rows_in_order_and_a_click_switches() {
        let mut state = state();
        visit(&mut state, &[1, 2, 3]);
        state.handle_input_bytes(ALT_TAB);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 30));
        let area = state.render_agent_switcher(&mut buffer).unwrap();
        let text = (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(at("conversation 3") < at("conversation 2"));
        assert!(at("conversation 2") < at("conversation 1"));
        assert!(text.contains(" agents "));

        let switcher = state.agent_switcher.as_ref().unwrap();
        let (rect, _, _) = switcher
            .row_hits
            .iter()
            .find(|(_, _, target)| target.pane_id == "pane_2")
            .unwrap()
            .clone();
        assert_eq!(buffer[(rect.x, rect.y)].symbol(), "▌");
        let (rect, _, _) = switcher
            .row_hits
            .iter()
            .find(|(_, _, target)| target.pane_id == "pane_1")
            .unwrap()
            .clone();
        let mut outcome = ClientShellInput::default();
        state.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: rect.x + 2,
                row: rect.y,
                modifiers: KeyModifiers::empty(),
            },
            &mut outcome,
        );
        assert_eq!(focused_panes(&outcome), ["pane_1"]);
        assert!(state.agent_switcher.is_none());
    }
}
