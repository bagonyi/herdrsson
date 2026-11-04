//! Fork: the popup of the agents panel title's command (`title_command`'s `popup`). A click on
//! the title opens it, and in the window whose click opened it, it hangs from the title instead
//! of sitting in the middle of the panes. The server sizes its terminal as it does any popup's,
//! so only where it's drawn changes: that's this client's business alone.

use super::*;
use crate::popup_size::PopupResolvedGeometry;

/// A click on the title, waiting for its popup, then the popup's terminal once it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TitlePopupAnchor {
    terminal_id: Option<String>,
}

impl ClientShellState {
    /// A click on the title opens its popup, as a popup key command opens its own. False when
    /// the endpoint has none, so the click goes on.
    pub(super) fn open_agent_panel_title_popup(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let Some(command_id) = snapshot.agent_panel_title_command_id.clone() else {
            return false;
        };
        let params = crate::api::schema::CommandInvokeParams {
            command_id,
            workspace_id: snapshot.focused_workspace_id.clone(),
            tab_id: snapshot.focused_tab_id.clone(),
            pane_id: snapshot.focused_pane_id.clone(),
            selection: None,
        };
        self.popup_pending = true;
        self.popup_pending_deadline = None;
        if self.push_endpoint_method_with_kind(
            crate::api::schema::Method::CommandInvoke(params),
            PendingEndpointKind::PopupCommand,
            outcome,
        ) {
            self.title_popup_anchor = Some(TitlePopupAnchor { terminal_id: None });
        } else {
            self.popup_pending = false;
        }
        outcome.repaint = true;
        true
    }

    /// Ties a click's anchor to the popup that shows while it waits, and lets it go once that
    /// popup has closed, another one shows, or none came. Call it with the surface's popup before
    /// `popup_pending` is cleared.
    pub(super) fn track_title_popup(&mut self, popup: Option<&str>) {
        let Some(anchor) = self.title_popup_anchor.as_mut() else {
            return;
        };
        match (popup, anchor.terminal_id.as_deref()) {
            (Some(popup), None) if self.popup_pending => {
                anchor.terminal_id = Some(popup.to_owned())
            }
            (Some(popup), Some(opened)) if popup == opened => {}
            (None, None) if self.popup_pending => {}
            _ => self.title_popup_anchor = None,
        }
    }

    /// Forgets a click still waiting for its popup: the wait ended without one (an error, the
    /// wait's deadline), or another popup command was sent, so the next popup isn't its.
    pub(super) fn forget_waiting_title_popup(&mut self) {
        if self
            .title_popup_anchor
            .as_ref()
            .is_some_and(|anchor| anchor.terminal_id.is_none())
        {
            self.title_popup_anchor = None;
        }
    }

    /// Forgets the click when a popup command fails: it opened nothing, so a popup that showed
    /// meanwhile (another window's) isn't its.
    pub(super) fn forget_failed_title_popup(&mut self) {
        self.title_popup_anchor = None;
    }

    /// Whether the popup on screen is the title's, opened here.
    pub(super) fn title_popup_open(&self) -> bool {
        self.title_popup_anchor.as_ref().is_some_and(|anchor| {
            anchor.terminal_id.is_some() && anchor.terminal_id == self.popup_terminal_id
        })
    }

    /// Where the popup goes: the title's popup hangs from the title as drawn now, its left edge
    /// under the title's, moved left as far as it takes to fit the window. Without room below
    /// the title it sits above it, and without room there either, as low as it fits. When the
    /// title isn't drawn (it printed nothing, or a narrow window has no sidebar) it stays
    /// centred, as any other popup does.
    pub(super) fn place_popup(
        &self,
        terminal_id: &str,
        geometry: PopupResolvedGeometry,
        cols: u16,
        rows: u16,
    ) -> PopupResolvedGeometry {
        let title = self.hits.agent_panel_title;
        let anchored = self
            .title_popup_anchor
            .as_ref()
            .is_some_and(|anchor| anchor.terminal_id.as_deref() == Some(terminal_id));
        if !anchored || title.is_empty() {
            return geometry;
        }
        let outer = geometry.outer;
        let lowest = rows.saturating_sub(outer.height);
        let x = title.x.min(cols.saturating_sub(outer.width));
        let y = if title.bottom().saturating_add(outer.height) <= rows {
            title.bottom()
        } else if title.y >= outer.height {
            title.y - outer.height
        } else {
            lowest
        }
        // Inside the window whatever the title's place, as drawing past its edge panics.
        .min(lowest);
        PopupResolvedGeometry {
            outer: Rect { x, y, ..outer },
            inner: Rect {
                x: x + geometry.inner.x.saturating_sub(outer.x),
                y: y + geometry.inner.y.saturating_sub(outer.y),
                ..geometry.inner
            },
        }
    }
}
