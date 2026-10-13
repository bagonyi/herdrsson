//! Fork: when a finished agent counts as seen, and a green frame around its pane until then.
//!
//! Herdr marks a finished ("done") agent seen when its tab comes on screen in the focused window:
//! you switch to it, the window comes back to it, or the tab in front of it closes. This fork
//! waits until the tab has stayed on screen for `AppState::seen_delay` (`ui.seen_delay_ms`), so
//! passing a tab on the way to another leaves its agents unseen. A zero delay marks them at once.
//! A pane hidden behind its tab's zoom (zen mode zooms too) isn't on screen, so it waits until it
//! shows.
//!
//! While a finished agent is unseen, its pane is framed in green wherever it shows, a window in
//! the background included, so the frame goes the moment the agent counts as seen, together with
//! the API's "done" and the window's red count. A pane's border turns green where focus would
//! colour it (`render_pane_borders`). A pane without a border, such as a lone pane, gets the frame
//! drawn over its outermost cells instead: the pane keeps its size and the program in it doesn't
//! redraw.
//!
//! The frame follows the pane's state and has no clock of its own. Whatever changes that state
//! asks for a full render: agent state changes come as internal events or API requests, the
//! server's tick reports the panes it marks seen, and so does a window coming to the front.

use std::time::{Duration, Instant};

use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::AppState;
use crate::detect::AgentState;
use crate::layout::{PaneId, PaneInfo};
use crate::pane::PaneState;
use crate::ui::TabSurfaceTarget;
use crate::workspace::{Tab, Workspace};

impl PaneState {
    /// The pane's tab is on screen. A pane is unseen only while its agent has finished and no one
    /// has looked yet: it is marked seen once its tab has stayed on screen for `delay`. Returns
    /// whether it was marked seen.
    pub fn see(&mut self, now: Instant, delay: Duration) -> bool {
        if self.seen {
            self.seen_wait_since = None;
            return false;
        }
        let since = *self.seen_wait_since.get_or_insert(now);
        if now.saturating_duration_since(since) < delay {
            return false;
        }
        self.seen = true;
        self.seen_wait_since = None;
        true
    }
}

impl Tab {
    /// The tab is on screen: each pane that shows is looked at (`PaneState::see`), and a pane
    /// hidden behind the tab's zoom starts its wait over. Returns whether a pane was marked seen.
    pub(crate) fn see_shown_panes(&mut self, now: Instant, delay: Duration) -> bool {
        let shown = self.zoomed.then(|| self.layout.focused());
        let mut seen = false;
        for (pane_id, pane) in &mut self.panes {
            if shown.is_some_and(|shown| shown != *pane_id) {
                pane.seen_wait_since = None;
            } else {
                seen |= pane.see(now, delay);
            }
        }
        seen
    }
}

/// Whether a pane's agent has finished and no one has looked at it yet, which the API reports as
/// "done". The pane is framed meanwhile.
pub(crate) fn is_unseen(app: &AppState, ws: &Workspace, pane_id: PaneId) -> bool {
    ws.pane_state(pane_id).is_some_and(|pane| {
        !pane.seen
            && app
                .terminals
                .get(&pane.attached_terminal_id)
                .is_some_and(|terminal| {
                    terminal.is_agent_terminal() && matches!(terminal.state, AgentState::Idle)
                })
    })
}

/// Whether a pane's frame would sit on cells its output writes to: true for a borderless pane,
/// whose frame is drawn over its outer cells. A pane with a top or left border has its frame on
/// the border. Panes with only a right or bottom border count as borderless, to be safe.
pub(crate) fn frame_covers_output(pane: &crate::protocol::PaneSurfacePane) -> bool {
    pane.inner_rect.x == pane.rect.x && pane.inner_rect.y == pane.rect.y
}

impl AppState {
    pub(crate) fn pane_is_unseen(&self, ws_idx: usize, pane_id: PaneId) -> bool {
        self.workspaces
            .get(ws_idx)
            .is_some_and(|ws| is_unseen(self, ws, pane_id))
    }

    /// When the next wait to count a pane as seen is over.
    pub(crate) fn next_seen_wait_deadline(&self) -> Option<Instant> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .flat_map(|tab| tab.panes.values())
            .filter_map(|pane| pane.seen_wait_since?.checked_add(self.seen_delay))
            .min()
    }

    /// Ends the waits of panes in every tab but `shown`, the one the focused window shows, if
    /// any. Their agents stay unseen, and framed, until their tab is back on screen for the whole
    /// delay.
    pub(crate) fn stop_seen_waits_except(&mut self, shown: Option<TabSurfaceTarget>) {
        for (workspace_index, ws) in self.workspaces.iter_mut().enumerate() {
            for (tab_index, tab) in ws.tabs.iter_mut().enumerate() {
                let target = TabSurfaceTarget {
                    workspace_index,
                    tab_index,
                };
                if shown == Some(target) {
                    continue;
                }
                for pane in tab.panes.values_mut() {
                    pane.seen_wait_since = None;
                }
            }
        }
    }
}

/// Draws the frame over the outermost cells of borderless panes that have one, after their
/// content. Bordered panes get theirs from `render_pane_borders`.
pub(crate) fn render_unseen_frames(
    app: &AppState,
    ws: &Workspace,
    pane_infos: &[PaneInfo],
    frame: &mut Frame,
) {
    let buf = frame.buffer_mut();
    let area = buf.area;
    for info in pane_infos {
        if !info.borders.is_empty() || !is_unseen(app, ws, info.id) {
            continue;
        }
        let rect = info.rect.intersection(area);
        if rect.width < 2 || rect.height < 2 {
            continue;
        }
        let right = rect.right() - 1;
        let bottom = rect.bottom() - 1;
        let edges = (rect.x..=right)
            .flat_map(|x| [(x, rect.y), (x, bottom)])
            .chain((rect.y + 1..bottom).flat_map(|y| [(rect.x, y), (right, y)]));
        for (x, y) in edges {
            let symbol = match (x == rect.x, x == right, y == rect.y, y == bottom) {
                (true, _, true, _) => "┌",
                (_, true, true, _) => "┐",
                (true, _, _, true) => "└",
                (_, true, _, true) => "┘",
                (_, _, true, _) | (_, _, _, true) => "─",
                _ => "│",
            };
            // Blank any half of a wide character the frame cuts, or the row would shift.
            if x == rect.x {
                if x > area.x && buf[(x - 1, y)].symbol().width() > 1 {
                    buf[(x - 1, y)].set_symbol(" ");
                }
                if buf[(x, y)].symbol().width() > 1 {
                    buf[(x + 1, y)].set_symbol(" ");
                }
            }
            if x == right && buf[(x - 1, y)].symbol().width() > 1 {
                buf[(x - 1, y)].set_symbol(" ");
            }
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_symbol(symbol).set_fg(app.palette.green);
        }
    }
}

#[cfg(test)]
impl AppState {
    /// The pane's agent has finished and no one has looked at it yet.
    pub(crate) fn test_finish_unseen(&mut self, ws_idx: usize, pane_id: PaneId) {
        let ws = &mut self.workspaces[ws_idx];
        let cwd = ws.identity_cwd.clone();
        let pane = ws.pane_state_mut(pane_id).unwrap();
        pane.seen = false;
        let terminal_id = pane.attached_terminal_id.clone();
        let terminal = self
            .terminals
            .entry(terminal_id.clone())
            .or_insert_with(|| crate::terminal::TerminalState::new(terminal_id, cwd));
        terminal.detected_agent = Some(crate::detect::Agent::Claude);
        terminal.state = AgentState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Direction, Rect};
    use ratatui::widgets::Borders;

    fn app_with(ws: Workspace) -> AppState {
        let mut app = AppState::test_new();
        app.workspaces = vec![ws];
        app.active = Some(0);
        app
    }

    /// Renders the active tab through the pane renderer and returns the buffer and pane layout.
    fn render(app: &AppState, width: u16, height: u16) -> (Buffer, Vec<PaneInfo>) {
        let runtimes = TerminalRuntimeRegistry::new();
        let surface = crate::ui::compute_tab_surface(
            app,
            &runtimes,
            Rect::new(0, 0, width, height),
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let view = crate::ui::TabSurfaceView {
            target: surface.target,
            pane_infos: &surface.pane_infos,
            split_borders: &surface.split_borders,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::render_tab_surface(app, &runtimes, view, frame))
            .unwrap();
        (terminal.backend().buffer().clone(), surface.pane_infos)
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn only_a_finished_agent_no_one_has_looked_at_is_framed() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].terminal_id(pane_id).unwrap().clone();

        // A pane without an agent is never framed.
        app.workspaces[0].pane_state_mut(pane_id).unwrap().seen = false;
        assert!(!app.pane_is_unseen(0, pane_id));

        app.test_finish_unseen(0, pane_id);
        assert!(app.pane_is_unseen(0, pane_id));

        // Working again, or looked at.
        app.terminals.get_mut(&terminal_id).unwrap().state = AgentState::Working;
        assert!(!app.pane_is_unseen(0, pane_id));
        app.test_finish_unseen(0, pane_id);
        app.workspaces[0].pane_state_mut(pane_id).unwrap().seen = true;
        assert!(!app.pane_is_unseen(0, pane_id));
    }

    #[test]
    fn a_finished_agent_is_framed_until_its_tab_has_stayed_on_screen_for_the_delay() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        app.test_finish_unseen(0, pane_id);
        let delay = Duration::from_secs(1);
        let now = Instant::now();

        let pane = app.workspaces[0].pane_state_mut(pane_id).unwrap();
        assert!(!pane.see(now, delay));
        assert!(!pane.see(now + delay - Duration::from_millis(1), delay));
        assert!(app.pane_is_unseen(0, pane_id));

        let pane = app.workspaces[0].pane_state_mut(pane_id).unwrap();
        assert!(pane.see(now + delay, delay));
        assert_eq!(pane.seen_wait_since, None);
        assert!(!pane.see(now + delay, delay));
        assert!(!app.pane_is_unseen(0, pane_id));
    }

    #[test]
    fn switching_to_a_tab_without_a_delay_counts_its_finished_agent_as_seen() {
        let mut ws = Workspace::test_new("test");
        let tab = ws.test_add_tab(None);
        let finished = ws.tabs[tab].root_pane;
        let mut app = app_with(ws);
        app.test_finish_unseen(0, finished);
        assert!(app.pane_is_unseen(0, finished));

        app.switch_workspace_tab(0, tab);

        // Seen at once, so its frame never shows.
        assert!(app.workspaces[0].pane_state(finished).unwrap().seen);
        assert!(!app.pane_is_unseen(0, finished));
        assert_eq!(app.next_seen_wait_deadline(), None);
    }

    #[test]
    fn leaving_a_tab_before_the_delay_keeps_its_agent_unseen_and_framed() {
        let mut ws = Workspace::test_new("test");
        let tab = ws.test_add_tab(None);
        let finished = ws.tabs[tab].root_pane;
        let mut app = app_with(ws);
        app.seen_delay = Duration::from_secs(1);
        app.test_finish_unseen(0, finished);

        app.switch_workspace_tab(0, tab);
        assert!(app.pane_is_unseen(0, finished));
        // The loop wakes when the wait is over.
        assert!(app.next_seen_wait_deadline().is_some());

        app.switch_workspace_tab(0, 0);
        app.stop_seen_waits_except(Some(TabSurfaceTarget {
            workspace_index: 0,
            tab_index: 0,
        }));
        assert!(app.pane_is_unseen(0, finished));
        // Nothing left to wake the loop for: the frame needs no clock while it waits.
        assert_eq!(app.next_seen_wait_deadline(), None);
    }

    #[test]
    fn a_pane_behind_the_zoom_is_neither_seen_nor_framed_until_it_shows() {
        let mut ws = Workspace::test_new("test");
        let left = ws.tabs[0].root_pane;
        let right = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(left);
        ws.tabs[0].zoomed = true;
        let mut app = app_with(ws);
        app.test_finish_unseen(0, right);
        let palette_green = app.palette.green;
        let green = |buffer: &Buffer| buffer.content().iter().any(|cell| cell.fg == palette_green);

        // Behind the zoom, even without a delay: not seen, no wait, nothing drawn.
        for delay in [Duration::ZERO, Duration::from_secs(1)] {
            app.seen_delay = delay;
            assert!(!app.mark_active_tab_seen());
            assert!(app.pane_is_unseen(0, right));
            assert_eq!(app.next_seen_wait_deadline(), None);
        }
        let (buffer, infos) = render(&app, 20, 6);
        assert_eq!(infos.len(), 1);
        assert!(!green(&buffer));

        // Unzoomed, it shows framed and its wait starts.
        app.workspaces[0].tabs[0].zoomed = false;
        let (buffer, infos) = render(&app, 20, 6);
        let rect = infos.iter().find(|info| info.id == right).unwrap().rect;
        assert_eq!(buffer[(rect.x, 2)].fg, app.palette.green);
        assert!(!app.mark_active_tab_seen());
        assert!(app.next_seen_wait_deadline().is_some());

        // Zoomed again on the other pane: its wait ends.
        app.workspaces[0].tabs[0].zoomed = true;
        assert!(!app.mark_active_tab_seen());
        assert_eq!(app.next_seen_wait_deadline(), None);
        assert!(app.pane_is_unseen(0, right));
    }

    #[tokio::test]
    async fn lone_pane_gets_a_green_frame_over_its_edges() {
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.insert_test_runtime(
            pane_id,
            TerminalRuntime::test_with_screen_bytes(8, 4, b"abcdefgh\r\nijklmnop"),
        );
        let mut app = app_with(ws);
        let (before, infos) = render(&app, 8, 4);
        assert_eq!(infos[0].borders, Borders::NONE);
        // The last column is the scrollbar's.
        assert_eq!(row(&before, 0), "abcdefg ");

        app.test_finish_unseen(0, pane_id);
        let (buffer, _) = render(&app, 8, 4);

        assert_eq!(row(&buffer, 0), "┌──────┐");
        assert_eq!(row(&buffer, 1), "│jklmno│");
        assert_eq!(row(&buffer, 3), "└──────┘");
        assert_eq!(buffer[(0, 2)].fg, app.palette.green);
        assert_eq!(buffer[(3, 0)].fg, app.palette.green);
        assert_ne!(buffer[(3, 1)].fg, app.palette.green);
    }

    #[test]
    fn split_pane_border_turns_green_and_its_neighbour_keeps_its_own() {
        for gaps in [true, false] {
            let mut ws = Workspace::test_new("test");
            let left = ws.tabs[0].root_pane;
            ws.test_split(Direction::Horizontal);
            let mut app = app_with(ws);
            app.pane_gaps = gaps;
            app.test_finish_unseen(0, left);

            let (buffer, infos) = render(&app, 20, 6);

            let rect = infos.iter().find(|info| info.id == left).unwrap().rect;
            let other = infos.iter().find(|info| info.id != left).unwrap().rect;
            // The left pane's own border, and the divider it shares without gaps.
            let divider = if gaps { rect.right() - 1 } else { rect.right() };
            for (x, y) in [(rect.x, 2), (divider, 2), (rect.x + 2, 0)] {
                assert_eq!(
                    buffer[(x, y)].fg,
                    app.palette.green,
                    "gaps={gaps} ({x},{y})"
                );
                assert_ne!(buffer[(x, y)].symbol(), " ", "gaps={gaps} ({x},{y})");
            }
            // Nothing is drawn over the pane's content, and the neighbour keeps its colours.
            assert_eq!(buffer[(divider - 1, 2)].symbol(), " ", "gaps={gaps}");
            assert_ne!(buffer[(other.right() - 1, 2)].fg, app.palette.green);
        }
    }

    #[test]
    fn wide_characters_cut_by_the_frame_are_blanked() {
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let mut app = app_with(ws);
        app.test_finish_unseen(0, pane_id);
        let info = PaneInfo {
            id: pane_id,
            rect: Rect::new(2, 0, 6, 3),
            inner_rect: Rect::new(2, 0, 6, 3),
            scrollbar_rect: None,
            borders: Borders::NONE,
            is_focused: true,
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(8, 3)).unwrap();
        terminal
            .draw(|frame| {
                let buf = frame.buffer_mut();
                // As the terminal renderer writes them: a wide character, then an empty cell.
                for (x, y) in [(1, 0), (2, 1), (6, 2)] {
                    buf[(x, y)].set_symbol("中");
                    buf[(x + 1, y)].set_symbol("");
                }
                render_unseen_frames(&app, &app.workspaces[0], &[info], frame);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();

        // Left of the frame, under its left edge, and under its right edge.
        assert_eq!(buffer[(1, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].symbol(), "┌");
        assert_eq!(buffer[(2, 1)].symbol(), "│");
        assert_eq!(buffer[(3, 1)].symbol(), " ");
        assert_eq!(buffer[(6, 2)].symbol(), "─");
        assert_eq!(buffer[(7, 2)].symbol(), "┘");
    }
}
