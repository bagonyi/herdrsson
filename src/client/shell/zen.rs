//! Fork: zen mode. The window shows the focused pane alone, centred at `ui.zen_width` columns,
//! with the sidebar, the tab bar and agent notifications hidden. Only the zen mode key turns it
//! off. In zen mode, the `zen_narrower` and `zen_wider` keys make it narrower and wider.

use super::*;
use crate::api::schema::{Method, PaneZoomMode, PaneZoomParams};
use crate::input::KeybindAction;

/// Columns one press of a width key takes off or adds.
const ZEN_WIDTH_STEP: u16 = 10;
/// The narrowest the keys make it, and the narrowest `ui.zen_width` or a saved width gives.
const ZEN_MIN_WIDTH: u16 = 40;
/// How long the window keeps zen mode's last frame after leaving it, waiting for the panes at
/// their usual size.
const ZEN_LEAVE_HOLD: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Default)]
pub(super) struct ZenMode {
    on: bool,
    /// The width the keys set, saved with the window's preferences. None uses `ui.zen_width`.
    width: Option<u16>,
    /// Tabs zen mode zoomed. Each is unzoomed the next time it is on screen with zen mode off.
    zoomed: Vec<ZenTab>,
    /// Tabs you zoomed or unzoomed yourself in this zen mode: zen mode leaves them as you set
    /// them. Any other tab the server unzooms (a split, a pane closing) is zoomed again.
    yours: Vec<ZenTab>,
    /// The last zoom asked for and the snapshot it was asked on: not asked again for that tab
    /// until a newer snapshot, so the zoom on its way isn't asked for on every pass.
    asked: Option<ZenAsked>,
    /// When zen mode was last turned off.
    left: Option<std::time::Instant>,
}

/// A tab in a session. A restarted session keeps its tab IDs and their zoom (both are saved
/// with the session), so zen mode's records outlive a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ZenTab {
    endpoint_id: ClientEndpointId,
    tab_id: String,
}

/// A snapshot is newer than the one a zoom was asked on when it has a higher revision or comes
/// from another boot or connection: the server counts revisions from 1 again on each connection.
struct ZenAsked {
    tab: ZenTab,
    boot_id: String,
    generation: Option<u64>,
    revision: u64,
}

impl ZenMode {
    pub(super) fn new(width: Option<u16>) -> Self {
        Self {
            width,
            ..Self::default()
        }
    }

    pub(super) fn saved_width(&self) -> Option<u16> {
        self.width
    }
}

impl ClientShellState {
    /// Zen mode's layout: no sidebar or tab bar, and the pane area centred at the zen width.
    pub(super) fn zen_layout(&self, cols: u16, rows: u16) -> Option<ClientShellLayout> {
        if !self.zen.on {
            return None;
        }
        let width = self.zen_width().min(cols);
        Some(ClientShellLayout {
            sidebar: Rect::default(),
            tab_bar: Rect::default(),
            mobile_header: Rect::default(),
            pane_surface: Rect::new((cols - width) / 2, 0, width, rows),
        })
    }

    pub(super) fn zen_on(&self) -> bool {
        self.zen.on
    }

    /// Whether the window keeps showing what it shows while the pane is on its way. Herdr draws
    /// a stand-in with the sidebar until the pane arrives at a new size; with zen mode's hidden
    /// sidebar that stand-in fills the window, so every width step and turning zen mode on or
    /// off would flash it.
    pub(super) fn zen_holds_screen(&self) -> bool {
        (self.zen.on
            || self
                .zen
                .left
                .is_some_and(|left| left.elapsed() < ZEN_LEAVE_HOLD))
            && self.snapshot.is_some()
            && self.endpoint_is_online(&self.active_endpoint_id)
    }

    fn zen_width(&self) -> u16 {
        self.zen
            .width
            .unwrap_or(self.config.zen_width)
            .max(ZEN_MIN_WIDTH)
    }

    /// Whether a key bound to Herdr acts. Zen mode's width keys act only in zen mode, so outside
    /// it a key such as ctrl+l still reaches the pane.
    pub(super) fn zen_binding_applies(&self, binding: &crate::input::KeybindMatch) -> bool {
        self.zen.on
            || !matches!(
                binding,
                crate::input::KeybindMatch::Action(
                    KeybindAction::ZenNarrower | KeybindAction::ZenWider
                )
            )
    }

    /// The zen mode key turns zen mode on and off, and the width keys set its width. Resize mode,
    /// the pane resize keys, the sidebar key and the space picker do nothing in zen mode: they
    /// would only change what it hides.
    pub(super) fn handle_zen(
        &mut self,
        action: KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        match action {
            KeybindAction::ZenMode => {
                self.zen.on = !self.zen.on;
                self.zen.left = (!self.zen.on).then(std::time::Instant::now);
                self.zen.yours.clear();
                self.zen.asked = None;
                // A drag or press on the chrome zen mode hides or brings back ends here. Its
                // click targets go with the pane surface until the next frame.
                self.chrome_drag = None;
                self.workspace_press = None;
                self.tab_press = None;
                self.group_header_press = None;
                self.invalidate_pane_surface();
                outcome.repaint = true;
                outcome.resize = true;
                self.sync_zen_zoom(outcome);
                true
            }
            KeybindAction::ZenNarrower | KeybindAction::ZenWider => {
                if self.zen.on {
                    self.resize_zen(action == KeybindAction::ZenWider, outcome);
                }
                true
            }
            KeybindAction::Zoom => {
                self.note_your_zoom(None);
                false
            }
            KeybindAction::EnterResizeMode
            | KeybindAction::ResizePaneLeft
            | KeybindAction::ResizePaneRight
            | KeybindAction::ResizePaneUp
            | KeybindAction::ResizePaneDown
            | KeybindAction::ToggleSidebar
            | KeybindAction::WorkspacePicker => self.zen.on,
            _ => false,
        }
    }

    /// The zoom key (on the focused pane), or the pane menu's zoom (on its pane), in zen mode:
    /// that pane's tab is left as you set it, now and after zen mode.
    pub(super) fn note_your_zoom(&mut self, pane_id: Option<&str>) {
        if !self.zen.on {
            return;
        }
        let Some(tab_id) = self.snapshot.as_deref().and_then(|snapshot| match pane_id {
            Some(pane_id) => snapshot
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id)
                .map(|pane| pane.tab_id.clone()),
            None => snapshot.focused_tab_id.clone(),
        }) else {
            return;
        };
        let tab = ZenTab {
            endpoint_id: self.active_endpoint_id.clone(),
            tab_id,
        };
        self.zen.zoomed.retain(|zoomed| zoomed != &tab);
        if !self.zen.yours.contains(&tab) {
            self.zen.yours.push(tab);
        }
    }

    /// Esc on a close confirmation goes back to the space picker, which zen mode hides: in zen
    /// mode it goes back to the pane.
    pub(super) fn zen_skip_space_picker(&mut self) {
        if self.zen.on && self.mode == ClientShellMode::Navigate {
            self.mode = self.copy_or_terminal_mode();
            self.navigate_workspace_id = None;
        }
    }

    fn resize_zen(&mut self, wider: bool, outcome: &mut ClientShellInput) {
        let cols = self.last_composed_size.map_or(u16::MAX, |(cols, _)| cols);
        let current = self.zen_width().min(cols);
        let width = if wider {
            current.saturating_add(ZEN_WIDTH_STEP).min(cols)
        } else {
            current
                .saturating_sub(ZEN_WIDTH_STEP)
                .max(ZEN_MIN_WIDTH.min(current))
        };
        if width == current {
            return;
        }
        self.zen.width = Some(width);
        self.invalidate_pane_surface();
        outcome.repaint = true;
        outcome.resize = true;
        self.persist_chrome_preferences(outcome);
    }

    /// Zen mode's zoom requests for the tab on screen, so it follows the focus wherever it goes.
    fn take_zen_zoom(&mut self) -> Vec<ClientShellAction> {
        let mut outcome = ClientShellInput::default();
        self.sync_zen_zoom(&mut outcome);
        outcome.actions
    }

    /// Zooms the tab on screen while zen mode is on, and unzooms a tab zen mode zoomed once it
    /// is on screen with zen mode off. A zoom request without a pane acts on the server's
    /// focused pane, so a request that arrives after a quick switch never pulls focus back.
    /// Runs between the client's events, so it allocates nothing unless it asks for a zoom.
    fn sync_zen_zoom(&mut self, outcome: &mut ClientShellInput) {
        if (!self.zen.on && self.zen.zoomed.is_empty())
            || !self.endpoint_is_online(&self.active_endpoint_id)
        {
            return;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let endpoint_id = &self.active_endpoint_id;
        // Forget tabs that are gone.
        self.zen.zoomed.retain(|tab| {
            &tab.endpoint_id != endpoint_id
                || snapshot.tabs.iter().any(|shown| shown.tab_id == tab.tab_id)
        });
        let Some(tab) = snapshot
            .focused_tab_id
            .as_ref()
            .and_then(|tab_id| snapshot.tabs.iter().find(|tab| &tab.tab_id == tab_id))
        else {
            return;
        };
        let on_screen =
            |zen_tab: &ZenTab| &zen_tab.endpoint_id == endpoint_id && zen_tab.tab_id == tab.tab_id;
        let mode = if self.zen.on {
            // A tab zoomed already, or one you zoomed or unzoomed yourself, is left alone.
            let panes = snapshot
                .panes
                .iter()
                .filter(|pane| pane.tab_id == tab.tab_id)
                .count();
            let asked = self.zen.asked.as_ref().is_some_and(|asked| {
                on_screen(&asked.tab)
                    && asked.boot_id == snapshot.boot_id
                    && asked.generation == self.active_snapshot_generation
                    && asked.revision >= snapshot.revision
            });
            if tab.zoomed || panes < 2 || asked || self.zen.yours.iter().any(on_screen) {
                return;
            }
            let focused = ZenTab {
                endpoint_id: endpoint_id.clone(),
                tab_id: tab.tab_id.clone(),
            };
            self.zen.asked = Some(ZenAsked {
                tab: focused.clone(),
                boot_id: snapshot.boot_id.clone(),
                generation: self.active_snapshot_generation,
                revision: snapshot.revision,
            });
            if !self.zen.zoomed.contains(&focused) {
                self.zen.zoomed.push(focused);
            }
            PaneZoomMode::On
        } else {
            let Some(index) = self.zen.zoomed.iter().position(on_screen) else {
                return;
            };
            self.zen.zoomed.remove(index);
            // Also when the snapshot doesn't show the zoom yet: it may still be on its way, and
            // the server ignores an unzoom of a tab that isn't zoomed.
            PaneZoomMode::Off
        };
        self.push_endpoint_method(
            Method::PaneZoom(PaneZoomParams {
                pane_id: None,
                mode,
            }),
            outcome,
        );
    }
}

impl crate::client::state::ClientState {
    /// Called by the client loop between events, so a snapshot that shows a split or a closed
    /// pane gets its tab zoomed again, and a tab zen mode zoomed is unzoomed once on screen with
    /// zen mode off. Waits while a machine switch is under way.
    pub(in crate::client) fn send_zen_zoom(
        &mut self,
        pending_activation: &Option<crate::client::endpoint::PendingEndpointActivation>,
        scheduled_activation: &mut Option<crate::client::events::ClientLoopEvent>,
        endpoint_commands: &mut crate::client::endpoint_commands::EndpointCommands,
        endpoints: &mut crate::client::endpoint::EndpointRegistry,
    ) -> Result<(), crate::client::ClientError> {
        if pending_activation.is_some()
            || scheduled_activation.is_some()
            || self.deferred_local_activation.is_some()
        {
            return Ok(());
        }
        let Some(shell) = self.shell.as_mut() else {
            return Ok(());
        };
        let actions = shell.take_zen_zoom();
        if actions.is_empty() {
            return Ok(());
        }
        let (_, repaint) = crate::client::shell_runtime::dispatch_client_shell_actions(
            actions,
            endpoint_commands,
            endpoints,
            Some(shell),
            &mut self.detached_process_children,
            scheduled_activation,
        )?;
        if let Some(frame) = repaint
            .then(|| shell.compose(self.reported_size.0, self.reported_size.1))
            .flatten()
        {
            self.present_frozen_chrome(frame);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{frame_rows, snapshot, surface};
    use super::*;
    use crate::config::Config;

    fn zen_config(width: u16) -> ClientShellConfig {
        let mut config = Config::default();
        config.ui.zen_width = width;
        ClientShellConfig::from_config(&config)
    }

    /// Tab tab_1 with panes pane_1 (focused) and pane_2, and tab tab_2 with pane_3.
    fn two_tabs(revision: u64, focused_tab: &str, zoomed: bool) -> Box<ClientShellSnapshot> {
        let mut snapshot = snapshot();
        snapshot.revision = revision;
        let tab = snapshot.tabs[0].clone();
        snapshot.tabs = ["tab_1", "tab_2"]
            .into_iter()
            .map(|tab_id| crate::protocol::ClientShellTab {
                tab_id: tab_id.into(),
                zoomed: zoomed && tab_id == focused_tab,
                focused: tab_id == focused_tab,
                ..tab.clone()
            })
            .collect();
        let pane = snapshot.panes[0].clone();
        snapshot.panes = [
            ("pane_1", "tab_1"),
            ("pane_2", "tab_1"),
            ("pane_3", "tab_2"),
        ]
        .into_iter()
        .map(|(pane_id, tab_id)| crate::protocol::ClientShellPane {
            pane_id: pane_id.into(),
            tab_id: tab_id.into(),
            focused: pane_id == "pane_1",
            ..pane.clone()
        })
        .collect();
        snapshot.focused_tab_id = Some(focused_tab.into());
        snapshot.focused_pane_id = Some(
            if focused_tab == "tab_1" {
                "pane_1"
            } else {
                "pane_3"
            }
            .into(),
        );
        Box::new(snapshot)
    }

    fn press(state: &mut ClientShellState, action: KeybindAction) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
        outcome
    }

    fn zooms(actions: &[ClientShellAction]) -> Vec<PaneZoomMode> {
        actions
            .iter()
            .map(|action| match action {
                ClientShellAction::Endpoint { request, .. } => match &request.method {
                    Method::PaneZoom(params) => {
                        assert_eq!(params.pane_id, None);
                        params.mode
                    }
                    method => panic!("expected a pane zoom, got {method:?}"),
                },
                action => panic!("expected an endpoint request, got {action:?}"),
            })
            .collect()
    }

    #[test]
    fn zen_layout_hides_the_chrome_and_centres_the_pane() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let normal = state.layout(200, 50);
        assert!(!normal.sidebar.is_empty());
        assert!(!normal.tab_bar.is_empty());

        let outcome = press(&mut state, KeybindAction::ZenMode);
        assert!(outcome.resize && outcome.repaint);
        let zen = state.layout(200, 50);
        assert!(zen.sidebar.is_empty() && zen.tab_bar.is_empty() && zen.mobile_header.is_empty());
        assert_eq!(zen.pane_surface, Rect::new(25, 0, 150, 50));
        // A narrower window gets the whole width, the mobile layout included.
        assert_eq!(state.layout(90, 30).pane_surface, Rect::new(0, 0, 90, 30));
        assert_eq!(state.layout(40, 20).pane_surface, Rect::new(0, 0, 40, 20));

        press(&mut state, KeybindAction::ZenMode);
        assert_eq!(state.layout(200, 50), normal);
    }

    #[test]
    fn width_keys_stop_at_the_window_and_the_narrowest() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        // Outside zen mode they do nothing.
        assert!(!press(&mut state, KeybindAction::ZenWider).resize);

        press(&mut state, KeybindAction::ZenMode);
        state.compose(200, 50);
        let wider = press(&mut state, KeybindAction::ZenWider);
        assert!(wider.actions.is_empty() && wider.resize);
        assert_eq!(
            state.layout(200, 50).pane_surface,
            Rect::new(20, 0, 160, 50)
        );
        press(&mut state, KeybindAction::ZenNarrower);
        press(&mut state, KeybindAction::ZenNarrower);
        assert_eq!(
            state.layout(200, 50).pane_surface,
            Rect::new(30, 0, 140, 50)
        );

        for _ in 0..20 {
            press(&mut state, KeybindAction::ZenWider);
        }
        assert_eq!(state.layout(200, 50).pane_surface, Rect::new(0, 0, 200, 50));
        assert!(!press(&mut state, KeybindAction::ZenWider).resize);
        for _ in 0..30 {
            press(&mut state, KeybindAction::ZenNarrower);
        }
        assert_eq!(state.layout(200, 50).pane_surface.width, ZEN_MIN_WIDTH);
        assert_eq!(state.zen.saved_width(), Some(ZEN_MIN_WIDTH));
    }

    #[test]
    fn resize_mode_and_resize_keys_do_nothing_in_zen_mode() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert!(matches!(
            press(&mut state, KeybindAction::ResizePaneRight).actions.as_slice(),
            [ClientShellAction::Endpoint { request, .. }] if matches!(request.method, Method::PaneResize(_))
        ));

        press(&mut state, KeybindAction::ZenMode);
        for action in [
            KeybindAction::EnterResizeMode,
            KeybindAction::ResizePaneLeft,
            KeybindAction::ResizePaneRight,
            KeybindAction::ResizePaneUp,
            KeybindAction::ResizePaneDown,
        ] {
            let outcome = press(&mut state, action);
            assert!(outcome.actions.is_empty() && !outcome.resize, "{action:?}");
            assert_eq!(state.mode, ClientShellMode::Terminal, "{action:?}");
        }
    }

    #[test]
    fn zen_mode_zooms_the_tab_on_screen_and_unzooms_it_once_off() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert!(state.take_zen_zoom().is_empty());

        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [PaneZoomMode::On]
        );
        // Asked once, not again while the answer is on its way.
        assert!(state.take_zen_zoom().is_empty());
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(state.take_zen_zoom().is_empty());

        // A tab with a single pane needs no zoom.
        state.set_snapshot(two_tabs(3, "tab_2", false));
        assert!(state.take_zen_zoom().is_empty());

        // Off: tab_1 is unzoomed when it is back on screen, not before.
        press(&mut state, KeybindAction::ZenMode);
        assert!(state.take_zen_zoom().is_empty());
        let mut back = two_tabs(4, "tab_1", true);
        back.tabs[1].zoomed = false;
        state.set_snapshot(back);
        assert_eq!(zooms(&state.take_zen_zoom()), [PaneZoomMode::Off]);
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn zen_mode_leaves_a_zoom_it_did_not_make() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn a_tab_you_unzoom_in_zen_mode_stays_unzoomed() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        press(&mut state, KeybindAction::ZenMode);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        press(&mut state, KeybindAction::Zoom);
        state.set_snapshot(two_tabs(3, "tab_1", false));
        assert!(state.take_zen_zoom().is_empty());
        // Yours now: leaving zen mode doesn't touch it.
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
    }

    #[test]
    fn a_tab_you_unzoom_and_zoom_again_in_zen_mode_stays_zoomed_after_it() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [PaneZoomMode::On]
        );
        state.set_snapshot(two_tabs(2, "tab_1", true));
        press(&mut state, KeybindAction::Zoom);
        state.set_snapshot(two_tabs(3, "tab_1", false));
        press(&mut state, KeybindAction::Zoom);
        state.set_snapshot(two_tabs(4, "tab_1", true));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // Nor when the window shows the tab again later.
        state.set_snapshot(two_tabs(5, "tab_2", false));
        state.set_snapshot(two_tabs(6, "tab_1", true));
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn a_new_connection_counts_as_a_newer_snapshot() {
        let mut state = ClientShellState::new(zen_config(150));
        let local = ClientEndpointId::Local;
        state.set_endpoint_snapshot_for_generation(&local, 1, two_tabs(1, "tab_1", false));
        press(&mut state, KeybindAction::ZenMode);
        state.set_endpoint_snapshot_for_generation(&local, 1, two_tabs(2, "tab_1", true));
        state.set_endpoint_snapshot_for_generation(&local, 1, two_tabs(5, "tab_1", false));
        assert_eq!(zooms(&state.take_zen_zoom()), [PaneZoomMode::On]);
        // The connection drops before that zoom lands. The next one counts revisions from 1.
        state.set_endpoint_snapshot_for_generation(&local, 2, two_tabs(1, "tab_1", false));
        assert_eq!(zooms(&state.take_zen_zoom()), [PaneZoomMode::On]);
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn esc_on_a_close_confirmation_goes_back_to_the_pane_in_zen_mode() {
        let mut state = ClientShellState::new(zen_config(150));
        state.config.confirm_close = true;
        state.set_snapshot(Box::new(snapshot()));
        press(&mut state, KeybindAction::ZenMode);
        press(&mut state, KeybindAction::CloseTab);
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::ConfirmClose(_))
        ));
        state.handle_input_bytes(b"\x1b");
        assert!(state.overlay.is_none());
        assert_eq!(state.mode, ClientShellMode::Terminal);
        assert!(state.navigate_workspace_id.is_none());

        // Outside zen mode it goes back to the space picker, as before.
        press(&mut state, KeybindAction::ZenMode);
        press(&mut state, KeybindAction::CloseTab);
        state.handle_input_bytes(b"\x1b");
        assert_eq!(state.mode, ClientShellMode::Navigate);
    }

    #[test]
    fn zen_width_is_at_least_the_narrowest_the_keys_give() {
        for width in [0, 1, ZEN_MIN_WIDTH - 1] {
            let mut state = ClientShellState::new(zen_config(width));
            state.set_snapshot(two_tabs(1, "tab_1", false));
            press(&mut state, KeybindAction::ZenMode);
            assert_eq!(
                state.layout(200, 50).pane_surface,
                Rect::new(80, 0, ZEN_MIN_WIDTH, 50),
                "{width}"
            );
            // A saved width too.
            state.zen.width = Some(width);
            assert_eq!(state.layout(200, 50).pane_surface.width, ZEN_MIN_WIDTH);
            // A narrower window still gets the whole width.
            assert_eq!(state.layout(30, 20).pane_surface, Rect::new(0, 0, 30, 20));
        }
    }

    #[test]
    fn your_zoom_key_in_zen_mode_sticks_on_a_tab_zoomed_before() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        press(&mut state, KeybindAction::Zoom);
        state.set_snapshot(two_tabs(2, "tab_1", false));
        assert!(state.take_zen_zoom().is_empty());
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
    }

    #[test]
    fn the_pane_menus_zoom_sticks_on_its_pane_s_tab_after_the_focus_moves() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        state.open_pane_context_menu("pane_1".into(), 50, 10);
        // Another window or the CLI focuses tab_2 while the menu stays open.
        let mut switched = two_tabs(2, "tab_2", false);
        switched.tabs[0].zoomed = true;
        state.set_snapshot(switched);
        let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
            panic!("the menu stays open");
        };
        let zoom = menu
            .items()
            .iter()
            .position(|item| item.action == ClientContextMenuAction::Zoom)
            .expect("zoom item");
        let mut outcome = ClientShellInput::default();
        state.activate_context_menu_item(zoom, &mut outcome);
        // The zoom focuses pane_1 and unzooms tab_1, which zen mode then leaves alone.
        state.set_snapshot(two_tabs(3, "tab_1", false));
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn a_tab_the_server_unzooms_in_zen_mode_is_zoomed_again() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [PaneZoomMode::On]
        );
        state.set_snapshot(two_tabs(2, "tab_1", true));
        // prefix+v: the server adds pane_4 and unzooms the tab, as any split or closed pane does.
        let mut split = two_tabs(3, "tab_1", false);
        let mut pane = split.panes[0].clone();
        pane.pane_id = "pane_4".into();
        split.panes.push(pane);
        state.set_snapshot(split);
        assert_eq!(zooms(&state.take_zen_zoom()), [PaneZoomMode::On]);
        // Asked once for this snapshot.
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn leaving_zen_mode_before_its_zoom_lands_still_unzooms() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [PaneZoomMode::On]
        );
        // Off again before the zoom shows: the unzoom goes after it.
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [PaneZoomMode::Off]
        );
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(state.take_zen_zoom().is_empty());
    }

    #[test]
    fn a_zoom_that_landed_on_another_tab_is_asked_again() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_2", false));
        press(&mut state, KeybindAction::ZenMode);
        // tab_1 is on screen for a moment; its zoom lands on the tab the server is on by then.
        state.set_snapshot(two_tabs(2, "tab_1", false));
        assert_eq!(zooms(&state.take_zen_zoom()), [PaneZoomMode::On]);
        state.set_snapshot(two_tabs(3, "tab_2", false));
        assert!(state.take_zen_zoom().is_empty());
        // Back on tab_1, still unzoomed: asked again.
        state.set_snapshot(two_tabs(4, "tab_1", false));
        assert_eq!(zooms(&state.take_zen_zoom()), [PaneZoomMode::On]);
    }

    #[test]
    fn zen_mode_keeps_its_zooms_across_a_session_restart() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        press(&mut state, KeybindAction::ZenMode);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        // The session restarts: a new boot, the same tabs, tab_1 restored zoomed.
        let mut restarted = two_tabs(1, "tab_1", true);
        restarted.boot_id = "boot-2".into();
        state.set_snapshot(restarted);
        assert!(state.take_zen_zoom().is_empty());
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [PaneZoomMode::Off]
        );
    }

    #[test]
    fn the_space_picker_and_the_sidebar_key_do_nothing_in_zen_mode() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        press(&mut state, KeybindAction::ZenMode);
        for action in [KeybindAction::WorkspacePicker, KeybindAction::ToggleSidebar] {
            let outcome = press(&mut state, action);
            assert!(outcome.actions.is_empty() && !outcome.resize, "{action:?}");
            assert_eq!(state.mode, ClientShellMode::Terminal, "{action:?}");
        }
        assert!(!state.sidebar_collapsed);
    }

    #[test]
    fn a_hidden_toast_leaves_the_surface_patch_fast_path_on() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        press(&mut state, KeybindAction::ZenMode);
        state.set_pane_surface(surface());
        state.compose(200, 50).expect("frame");
        state.visible_notification = Some(ClientVisibleNotification {
            endpoint_id: ClientEndpointId::Local,
            event: SemanticNotification {
                kind: SemanticNotificationKind::Finished,
                title: "claude finished".into(),
                body: None,
                sound: None,
                agent: None,
                workspace_id: None,
                tab_id: Some("tab_2".into()),
                pane_id: Some("pane_3".into()),
                position: None,
            },
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
        });
        let frame = frame_rows(&state.compose(200, 50).expect("frame")).join("\n");
        assert!(!frame.contains("claude finished"), "the toast is hidden");
        let surface = state.pane_surface.as_ref().expect("surface");
        let mut pane = surface.panes[0].clone();
        pane.content_revision += 1;
        let patch = crate::protocol::PaneSurfacePatch {
            boot_id: surface.boot_id.clone(),
            projection_revision: surface.projection_revision,
            base_surface_revision: surface.surface_revision,
            surface_revision: surface.surface_revision + 1,
            rows: vec![crate::protocol::PaneSurfacePatchRow {
                x: 0,
                y: 0,
                cells: vec![
                    crate::protocol::CellData {
                        symbol: "N".into(),
                        fg: 0,
                        bg: 0,
                        modifier: 0,
                        skip: false,
                        hyperlink: None,
                    };
                    4
                ],
            }],
            panes: vec![pane],
            cursor: None,
        };
        assert!(matches!(
            state.apply_pane_surface_patch(patch),
            super::super::surface_patch::ClientPaneSurfacePatchOutcome::Applied(Some(_))
        ));
    }

    #[test]
    fn offline_in_zen_mode_draws_the_usual_stand_in() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        state.set_pane_surface(surface());
        let id = state.active_endpoint_id.clone();
        state.mark_endpoint_disconnected(&id);
        state.invalidate_pane_surface();
        let usual = frame_rows(&state.compose(200, 20).expect("frame"));
        press(&mut state, KeybindAction::ZenMode);
        let zen = frame_rows(&state.compose(200, 20).expect("frame"));
        assert_eq!(zen, usual);
    }

    #[test]
    fn zen_mode_keeps_the_screen_while_the_pane_is_on_its_way() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        state.set_pane_surface(surface());
        assert!(state.compose(200, 50).is_some());

        // Turning zen mode on, and each width step, waits for the pane at its new size.
        press(&mut state, KeybindAction::ZenMode);
        assert!(state.compose(200, 50).is_none());
        state.set_pane_surface(surface());
        assert!(state.compose(200, 50).is_some());
        press(&mut state, KeybindAction::ZenWider);
        assert!(state.compose(200, 50).is_none());

        // So does turning it off, for a moment; then Herdr's stand-in shows as usual.
        press(&mut state, KeybindAction::ZenMode);
        assert!(state.compose(200, 50).is_none());
        if let Some(earlier) = std::time::Instant::now().checked_sub(2 * ZEN_LEAVE_HOLD) {
            state.zen.left = Some(earlier);
            assert!(state.compose(200, 50).is_some());
        }
    }

    #[test]
    fn prefix_bar_shows_the_zen_mode_key() {
        let mut config = Config::default();
        config.keys.zen_mode = crate::config::BindingConfig::one("prefix+shift+z");
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let bar = |state: &mut ClientShellState| {
            state.set_pane_surface(surface());
            state.mode = ClientShellMode::Prefix;
            let frame = state.compose(200, 50).expect("frame");
            state.mode = ClientShellMode::Terminal;
            frame_rows(&frame)[49].clone()
        };
        let normal = bar(&mut state);
        assert!(normal.contains("shift+z zen  ?"), "{normal}");
        press(&mut state, KeybindAction::ZenMode);
        let zen = bar(&mut state);
        assert!(zen.contains("shift+z leave zen  ?"), "{zen}");
        assert!(!zen.contains("workspace nav"), "{zen}");

        // Without a zen mode key there is no hint.
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert!(!bar(&mut state).contains("zen"));
    }

    #[test]
    fn width_keys_act_in_zen_mode_and_reach_the_pane_outside_it() {
        let mut config = Config::default();
        config.keys.zen_narrower = crate::config::BindingConfig::one("ctrl+shift+h");
        config.keys.zen_wider = crate::config::BindingConfig::one("ctrl+shift+l");
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        state.compose(200, 50);
        // Bytes as Ghostty sends them with Herdr's keyboard enhancements.
        let send = |state: &mut ClientShellState, bytes: &[u8]| {
            state.handle_raw_events(crate::raw_input::parse_raw_input_bytes_sync(bytes))
        };
        let (ctrl_shift_h, ctrl_shift_l, ctrl_l) = (b"\x1b[104;6u", b"\x1b[108;6u", b"\x0c");

        let typed = send(&mut state, ctrl_shift_l);
        assert!(!typed.requests.is_empty() && !typed.resize);

        press(&mut state, KeybindAction::ZenMode);
        let wider = send(&mut state, ctrl_shift_l);
        assert!(wider.requests.is_empty() && wider.resize);
        assert_eq!(
            state.layout(200, 50).pane_surface,
            Rect::new(20, 0, 160, 50)
        );
        send(&mut state, ctrl_shift_h);
        send(&mut state, ctrl_shift_h);
        assert_eq!(
            state.layout(200, 50).pane_surface,
            Rect::new(30, 0, 140, 50)
        );
        // Plain ctrl+l still reaches the pane in zen mode.
        let clear = send(&mut state, ctrl_l);
        assert!(!clear.requests.is_empty() && !clear.resize);
        assert_eq!(state.layout(200, 50).pane_surface.width, 140);
    }

    #[derive(Clone)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<crate::protocol::ClientMessage>>>);

    impl crate::client::endpoint::EndpointTransport for Capture {
        fn send(&mut self, message: &crate::protocol::ClientMessage) -> std::io::Result<()> {
            self.0.lock().unwrap().push(message.clone());
            Ok(())
        }
    }

    /// Runs the client loop's zen mode step and returns the zooms it sent to the server.
    fn loop_zooms(
        state: &mut crate::client::state::ClientState,
        endpoints: &mut crate::client::endpoint::EndpointRegistry,
        sent: &std::sync::Mutex<Vec<crate::protocol::ClientMessage>>,
        pending_activation: &Option<crate::client::endpoint::PendingEndpointActivation>,
        scheduled_activation: &mut Option<crate::client::events::ClientLoopEvent>,
    ) -> Vec<PaneZoomMode> {
        // A lane of its own each time, so no zoom waits behind the last one's answer.
        let mut commands = crate::client::endpoint_commands::EndpointCommands::default();
        state
            .send_zen_zoom(
                pending_activation,
                scheduled_activation,
                &mut commands,
                endpoints,
            )
            .unwrap();
        sent.lock()
            .unwrap()
            .drain(..)
            .map(|message| match message {
                crate::protocol::ClientMessage::ClientShellEndpointRequest { request, .. } => {
                    let request: crate::api::schema::Request =
                        serde_json::from_str(&request).unwrap();
                    match request.method {
                        Method::PaneZoom(params) => params.mode,
                        method => panic!("expected a pane zoom, got {method:?}"),
                    }
                }
                message => panic!("expected an endpoint request, got {message:?}"),
            })
            .collect()
    }

    /// A switch to Local, prepared but not started.
    fn pending_activation(
        state: &crate::client::state::ClientState,
    ) -> crate::client::endpoint::PendingEndpointActivation {
        let endpoints = crate::client::endpoint::EndpointRegistry::new(
            Capture(Default::default()),
            1,
            crate::client::endpoint::EndpointNegotiation::new(
                vec!["client_shell.surface.set".into()],
                vec![
                    crate::protocol::endpoint::SURFACE_INTEREST_CAPABILITY.into(),
                    crate::protocol::endpoint::PRESENTATION_EFFECTS_FENCE_CAPABILITY.into(),
                ],
            ),
        );
        let resize = crate::protocol::ClientMessage::ClientShellResize {
            cell_width_px: 8,
            cell_height_px: 16,
            surface_size: crate::protocol::ClientSurfaceSize { cols: 80, rows: 24 },
            pixel_mouse: false,
        };
        crate::client::endpoint::PendingEndpointActivation::prepare(
            state.shell.as_ref().expect("shell"),
            &endpoints,
            ClientEndpointId::Local,
            None,
            resize,
            1,
            std::time::Instant::now(),
        )
        .expect("activation")
    }

    #[test]
    fn the_client_loop_zooms_the_tab_again_after_a_split_or_a_closed_pane() {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut endpoints = crate::client::endpoint::EndpointRegistry::new(
            Capture(sent.clone()),
            1,
            crate::client::endpoint::EndpointNegotiation::default(),
        );
        let mut state = crate::client::state::ClientState::test_new();
        let shell = state.shell.as_mut().expect("shell");
        shell.set_snapshot(two_tabs(1, "tab_1", false));
        // The key's own zoom goes out with the key press.
        assert_eq!(
            zooms(&press(shell, KeybindAction::ZenMode).actions),
            [PaneZoomMode::On]
        );
        shell.set_snapshot(two_tabs(2, "tab_1", true));
        let mut idle = None;
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());

        // prefix+v: the server adds pane_4 and unzooms the tab.
        let mut split = two_tabs(3, "tab_1", false);
        let mut pane = split.panes[0].clone();
        pane.pane_id = "pane_4".into();
        split.panes.push(pane);
        state.shell.as_mut().expect("shell").set_snapshot(split);
        // Not while a machine switch waits to start, is under way or waits for Local.
        let mut scheduled = Some(crate::client::events::ClientLoopEvent::Timer);
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut scheduled).is_empty());
        assert!(scheduled.is_some());
        let switching = Some(pending_activation(&state));
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &switching, &mut idle).is_empty());
        state.deferred_local_activation = Some(crate::client::endpoint::EndpointActivationIntent {
            endpoint_id: ClientEndpointId::Local,
            target: None,
        });
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());
        state.deferred_local_activation = None;
        assert_eq!(
            loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle),
            [PaneZoomMode::On]
        );
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());

        // pane_4 closes: the server unzooms the tab again.
        let shell = state.shell.as_mut().expect("shell");
        shell.set_snapshot(two_tabs(4, "tab_1", true));
        shell.set_snapshot(two_tabs(5, "tab_1", false));
        assert_eq!(
            loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle),
            [PaneZoomMode::On]
        );

        // Zen mode off on tab_2: tab_1 is unzoomed once the window shows it.
        let shell = state.shell.as_mut().expect("shell");
        let mut elsewhere = two_tabs(6, "tab_2", false);
        elsewhere.tabs[0].zoomed = true;
        shell.set_snapshot(elsewhere);
        assert!(press(shell, KeybindAction::ZenMode).actions.is_empty());
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());
        state
            .shell
            .as_mut()
            .expect("shell")
            .set_snapshot(two_tabs(7, "tab_1", true));
        assert_eq!(
            loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle),
            [PaneZoomMode::Off]
        );
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());
    }

    /// Spaces `A/one` (focused, with tabs tab_1 and tab_2) and `A/two` under the group header
    /// `A`, then `plain` and `other`.
    fn grouped(config: ClientShellConfig) -> ClientShellState {
        let mut projected = snapshot();
        let template = projected.workspaces[0].clone();
        projected.workspaces = ["A/one", "A/two", "plain", "other"]
            .into_iter()
            .enumerate()
            .map(|(index, label)| ClientShellWorkspace {
                workspace_id: format!("ws_{}", index + 1),
                number: index + 1,
                label: label.into(),
                custom_label: true,
                branch: None,
                focused: index == 0,
                ..template.clone()
            })
            .collect();
        let mut tab = projected.tabs[0].clone();
        tab.tab_id = "tab_2".into();
        tab.number = 2;
        tab.focused = false;
        projected.tabs.push(tab);
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(projected));
        state.set_pane_surface(surface());
        state
    }

    fn mouse(
        state: &mut ClientShellState,
        kind: MouseEventKind,
        (column, row): (u16, u16),
    ) -> ClientShellInput {
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })])
    }

    #[test]
    fn zen_mode_hides_the_space_group_headers_and_their_hits() {
        let mut state = grouped(zen_config(150));
        let usual = frame_rows(&state.compose(200, 30).expect("frame")).join("\n");
        assert!(usual.contains("plain"), "{usual}");
        let header = state
            .hits
            .group_headers
            .iter()
            .find(|hit| hit.group == "A")
            .map(|hit| (hit.rect.x + 1, hit.rect.y))
            .expect("visible group header");

        press(&mut state, KeybindAction::ZenMode);
        state.set_pane_surface(surface());
        let zen = frame_rows(&state.compose(200, 30).expect("frame")).join("\n");
        for name in ["one", "two", "plain", "other"] {
            assert!(!zen.contains(name), "{name}: {zen}");
        }
        assert!(state.hits.group_headers.is_empty());
        assert!(state.hits.workspaces.is_empty() && state.hits.machines.is_empty());
        assert!(state.hits.tabs.is_empty() && state.hits.sidebar_divider.is_empty());

        // A click where the header was doesn't fold the group, nor does a drag from there start
        // a group drag.
        let mut actions =
            mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header).actions;
        assert!(state.group_header_press.is_none());
        actions.extend(mouse(&mut state, MouseEventKind::Up(MouseButton::Left), header).actions);
        assert!(state.collapsed_groups.is_empty());
        actions.extend(mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header).actions);
        let below = (header.0, header.1 + 3);
        actions.extend(mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), below).actions);
        assert!(state.chrome_drag.is_none());
        actions.extend(mouse(&mut state, MouseEventKind::Up(MouseButton::Left), below).actions);
        assert!(actions.is_empty());
        assert!(state.collapsed_groups.is_empty());
    }

    #[test]
    fn hover_highlights_do_not_draw_in_zen_mode() {
        let mut state = grouped(zen_config(150));
        let plain = state.compose(200, 30).expect("frame");
        let tab = state.hits.tabs.iter().find(|(_, id)| id == "tab_2");
        let space = state
            .hits
            .workspaces
            .iter()
            .find(|hit| hit.workspace_id == "ws_3");
        let rows = [
            state.hits.group_headers.first().expect("header").rect,
            space.expect("space").rect,
            tab.expect("tab").0,
            state.hits.sidebar_divider,
        ];
        // Outside zen mode each of them lights up under the mouse.
        for rect in rows {
            assert!(mouse(&mut state, MouseEventKind::Moved, (rect.x, rect.y)).repaint);
            assert_ne!(
                state.compose(200, 30).expect("frame").cells,
                plain.cells,
                "{rect:?}"
            );
        }
        mouse(&mut state, MouseEventKind::Moved, (199, 29));

        press(&mut state, KeybindAction::ZenMode);
        state.set_pane_surface(surface());
        let zen = state.compose(200, 30).expect("frame");
        for rect in rows {
            assert!(!mouse(&mut state, MouseEventKind::Moved, (rect.x, rect.y)).repaint);
            assert_eq!(
                state.compose(200, 30).expect("frame").cells,
                zen.cells,
                "{rect:?}"
            );
        }
    }

    #[test]
    fn turning_zen_mode_on_ends_a_drag_or_press_on_the_hidden_chrome() {
        let mut state = grouped(zen_config(150));
        state.compose(200, 30).expect("frame");
        let divider = state.hits.sidebar_divider;
        let width = state.sidebar_width;
        mouse(
            &mut state,
            MouseEventKind::Down(MouseButton::Left),
            (divider.x, divider.y),
        );
        assert!(matches!(
            state.chrome_drag,
            Some(ClientChromeDrag::SidebarWidth)
        ));
        press(&mut state, KeybindAction::ZenMode);
        assert!(state.chrome_drag.is_none());
        let further = (divider.x + 10, divider.y);
        mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), further);
        mouse(&mut state, MouseEventKind::Up(MouseButton::Left), further);
        assert_eq!(state.sidebar_width, width);

        // A press on a group header ends too, and a click on the frame still on screen finds
        // nothing.
        press(&mut state, KeybindAction::ZenMode);
        state.set_pane_surface(surface());
        state.compose(200, 30).expect("frame");
        let header = state
            .hits
            .group_headers
            .first()
            .map(|hit| (hit.rect.x + 1, hit.rect.y))
            .expect("visible group header");
        mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header);
        assert!(state.group_header_press.is_some());
        press(&mut state, KeybindAction::ZenMode);
        assert!(state.group_header_press.is_none());
        mouse(&mut state, MouseEventKind::Up(MouseButton::Left), header);
        let stale = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header);
        mouse(&mut state, MouseEventKind::Up(MouseButton::Left), header);
        assert!(stale.actions.is_empty());
        assert!(state.collapsed_groups.is_empty());
    }
}
