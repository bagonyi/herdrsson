use super::*;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

impl ClientShellState {
    /// On macOS ctrl+click is a right click, but a host terminal reporting the mouse to Herdr
    /// sends it as a left click with Ctrl held. Outside terminal content that press, its drags
    /// and its release become a right click, so ctrl+click opens the menus a right click opens.
    /// On a pane's or popup's content it stays a ctrl+click, which opens links and reaches the
    /// program; with an overlay open it stays one too, so menus and dialogs take it as a click.
    pub(super) fn ctrl_click_as_right_click(&mut self, mouse: MouseEvent) -> MouseEvent {
        let right_click = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.ctrl_click_held = mouse.modifiers.contains(KeyModifiers::CONTROL)
                    && !self.replaying_url_click
                    && self.overlay.is_none()
                    && !self.on_terminal_content((mouse.column, mouse.row));
                self.ctrl_click_held
            }
            MouseEventKind::Drag(MouseButton::Left) => self.ctrl_click_held,
            MouseEventKind::Up(MouseButton::Left) => std::mem::take(&mut self.ctrl_click_held),
            _ => false,
        };
        if !right_click {
            return mouse;
        }
        let kind = match mouse.kind {
            MouseEventKind::Down(_) => MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Drag(_) => MouseEventKind::Drag(MouseButton::Right),
            _ => MouseEventKind::Up(MouseButton::Right),
        };
        MouseEvent {
            kind,
            modifiers: mouse.modifiers.difference(KeyModifiers::CONTROL),
            ..mouse
        }
    }

    fn on_terminal_content(&self, point: (u16, u16)) -> bool {
        self.popup_terminal_id.is_some()
            || (self.hits.popup.iter()).any(|hit| contains(hit.inner_rect, point))
            || (self.hits.panes.iter()).any(|hit| contains(hit.inner_rect, point))
    }
}
