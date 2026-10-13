//! Fork: the server's side of counting a finished agent as seen (see `unseen_frame`).

use super::*;

impl HeadlessServer {
    /// The tab the focused window shows counts as looked at once it has been on screen for
    /// `ui.seen_delay_ms`, however it got there; a tab can also come on screen without a switch,
    /// such as when the one in front of it closes; a pane hidden behind its zoom waits until it
    /// shows. Every other tab's wait ends. Runs on every pass of the server loop before it
    /// renders, so a window coming to the front, or typing in it, counts its tab as seen before
    /// the next render. Agents newly seen are announced the way switching to the tab would, so
    /// plugins tracking unseen agents catch up. Returns whether a pane was marked seen: its green
    /// frame needs a full render to go away.
    pub(super) fn mark_shown_tab_seen(&mut self, now: Instant) -> bool {
        let shown = self.foreground_client_id.and_then(|client_id| {
            let focused = self
                .clients
                .get(&client_id)
                .is_some_and(|client| client.outer_terminal_focus == Some(true));
            focused
                .then(|| self.shell_target_for_client(client_id))
                .flatten()
        });
        self.app.state.stop_seen_waits_except(shown);
        let delay = self.app.state.seen_delay;
        let Some(target) = shown else {
            return false;
        };
        let Some(tab) = self
            .app
            .state
            .workspaces
            .get_mut(target.workspace_index)
            .and_then(|ws| ws.tabs.get_mut(target.tab_index))
        else {
            return false;
        };
        let seen = tab.see_shown_panes(now, delay);
        if seen {
            let pane_id = tab.layout.focused();
            self.app
                .emit_focus_api_events(target.workspace_index, pane_id);
        }
        seen
    }
}
