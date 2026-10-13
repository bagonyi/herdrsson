//! Fork: zen mode. The window shows the focused pane alone, centred at `ui.zen_width` columns,
//! with the sidebar, the tab bar and agent notifications hidden. Only the zen mode key turns it
//! off. In zen mode, the `zen_narrower` and `zen_wider` keys make it narrower and wider.

use super::*;
use crate::api::schema::{Method, PaneZoomMode, PaneZoomParams, ResponseResult};
use crate::input::KeybindAction;

/// Columns one press of a width key takes off or adds.
const ZEN_WIDTH_STEP: u16 = 10;
/// The narrowest the keys make it, and the narrowest `ui.zen_width` or a saved width gives.
const ZEN_MIN_WIDTH: u16 = 40;
/// How long the window keeps zen mode's last frame after leaving it, waiting for the panes at
/// their usual size.
const ZEN_LEAVE_HOLD: std::time::Duration = std::time::Duration::from_secs(1);
/// Zen mode's requests whose answers it waits for, at most: a request whose answer never comes
/// (the connection dropped) is forgotten once this many newer ones are on their way.
const ZEN_REQUESTS_KEPT: usize = 16;
/// The window's own requests that never take it to another tab or pane: once they are answered,
/// its snapshot still shows where the server has it. After any other, zen mode waits for the
/// next snapshot.
const ZEN_STILL_METHODS: &[&str] = &[
    "integration.install",
    "integration.list",
    "layout.set_split_ratio",
    "pane.copy_motion",
    "pane.copy_search",
    "pane.input.set",
    "pane.link.activate",
    "pane.link.resolve",
    "pane.rename",
    "pane.resize",
    "pane.scroll",
    "pane.selection.read",
    "product_announcement.dismiss",
    "release_notes.dismiss",
    "server.reload_config",
    "tab.rename",
    "workspace.rename",
    "worktree.list",
];

#[derive(Default)]
pub(super) struct ZenMode {
    on: bool,
    /// The width the keys set, saved with the window's preferences. None uses `ui.zen_width`.
    width: Option<u16>,
    /// Zooms zen mode made and still has to undo, saved with the window's preferences so that a
    /// window closing before it undid them leaves them to the next window.
    zoomed: Vec<ZenZoom>,
    /// Whether `zoomed` changed since the preferences were last saved.
    unsaved: bool,
    /// Tabs you zoomed or unzoomed yourself in this zen mode: zen mode leaves them as you set
    /// them. Any other tab the server unzooms (a split, a pane closing) is zoomed again.
    yours: Vec<ZenTab>,
    /// The last zoom asked for and the snapshot it was asked on: not asked again for that tab
    /// until a newer snapshot, so the zoom on its way isn't asked for on every pass.
    asked: Option<ZenAsked>,
    /// Zen mode's requests on their way: their answers come back to zen mode, and a failed one
    /// shows no notice.
    requests: Vec<(String, ClientEndpointId)>,
    /// The snapshot on screen when one of the window's own requests that can take it elsewhere
    /// was answered: until a newer one, the snapshot may show where the window was before.
    moved_on: Option<ZenSnapshotMark>,
    /// The snapshot `zoomed` was last checked against, so the client loop checks it again only
    /// when there is something new.
    tracked: Option<ZenSnapshotMark>,
    /// When zen mode was last turned off.
    left: Option<std::time::Instant>,
}

/// A tab in a session. A restarted session keeps its tab and pane IDs and its tabs' zoom (all
/// are saved with the session), so zen mode's records outlive a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ZenTab {
    endpoint_id: ClientEndpointId,
    tab_id: String,
}

/// A zoom zen mode made, still its to undo while the tab is zoomed on one of the panes it had
/// when zen mode zoomed it: the zoom follows you from pane to pane. A pane the tab didn't have
/// then, such as a plugin's overlay, shows only because the zoom is now its: zen mode leaves
/// the zoom alone while that pane is in the tab, and the overlay hands it back when it closes.
struct ZenZoom {
    tab: ZenTab,
    /// The pane the zoom shows.
    pane_id: String,
    /// The tab's panes when zen mode zoomed it.
    panes: Vec<String>,
    /// The snapshot on screen when the zoom was asked for or answered, until one shows the tab
    /// zoomed: an older snapshot showing it unzoomed predates the zoom.
    answered_on: Option<ZenSnapshotMark>,
    /// The request that asked for the zoom, until its answer: till then the record is zen mode's
    /// guess, the tab and pane on screen. If no answer comes, the guess stays.
    asked_by: Option<String>,
}

/// A zoom zen mode has yet to undo, as saved with the window's preferences.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(super) struct SavedZenZoom {
    /// `ClientEndpointId::storage_key`.
    endpoint: String,
    tab_id: String,
    pane_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    panes: Vec<String>,
}

/// A zoom asked for, not asked again for its tab until a newer snapshot.
struct ZenAsked {
    tab: ZenTab,
    on: ZenSnapshotMark,
}

/// A snapshot, which a later one is newer than when it has a higher revision or comes from
/// another boot or connection: the server counts revisions from 1 again on each connection.
struct ZenSnapshotMark {
    boot_id: String,
    generation: Option<u64>,
    revision: u64,
}

impl ZenSnapshotMark {
    fn of(snapshot: &ClientShellSnapshot, generation: Option<u64>) -> Self {
        Self {
            boot_id: snapshot.boot_id.clone(),
            generation,
            revision: snapshot.revision,
        }
    }

    /// Whether `snapshot` is this one or older.
    fn covers(&self, snapshot: &ClientShellSnapshot, generation: Option<u64>) -> bool {
        self.boot_id == snapshot.boot_id
            && self.generation == generation
            && self.revision >= snapshot.revision
    }
}

impl ZenMode {
    pub(super) fn new(width: Option<u16>, saved: Vec<SavedZenZoom>) -> Self {
        let zoomed = saved
            .into_iter()
            .filter_map(|saved| {
                let endpoint_id = match saved.endpoint.split_once(':') {
                    None if saved.endpoint == "local" => ClientEndpointId::Local,
                    Some(("ssh", profile_id)) => ClientEndpointId::Ssh(
                        crate::client::endpoint::ProfileId::parse(profile_id).ok()?,
                    ),
                    _ => return None,
                };
                Some(ZenZoom {
                    tab: ZenTab {
                        endpoint_id,
                        tab_id: saved.tab_id,
                    },
                    pane_id: saved.pane_id,
                    panes: saved.panes,
                    answered_on: None,
                    asked_by: None,
                })
            })
            .collect();
        Self {
            width,
            zoomed,
            ..Self::default()
        }
    }

    pub(super) fn saved_width(&self) -> Option<u16> {
        self.width
    }

    pub(super) fn saved_zooms(&self) -> Vec<SavedZenZoom> {
        self.zoomed
            .iter()
            .map(|zoom| SavedZenZoom {
                endpoint: zoom.tab.endpoint_id.storage_key(),
                tab_id: zoom.tab.tab_id.clone(),
                pane_id: zoom.pane_id.clone(),
                panes: zoom.panes.clone(),
            })
            .collect()
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
                self.switch_zen(outcome);
                self.sync_zen_zoom(outcome);
                true
            }
            KeybindAction::ZenNarrower | KeybindAction::ZenWider => {
                if self.zen.on {
                    self.resize_zen(action == KeybindAction::ZenWider, outcome);
                }
                true
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

    /// Turns zen mode on or off.
    fn switch_zen(&mut self, outcome: &mut ClientShellInput) {
        self.zen.on = !self.zen.on;
        self.zen.left = (!self.zen.on).then(std::time::Instant::now);
        self.zen.yours.clear();
        self.zen.asked = None;
        self.zen.tracked = None;
        // A drag or press on the chrome zen mode hides or brings back ends here. Its click
        // targets go with the pane surface until the next frame.
        self.chrome_drag = None;
        self.workspace_press = None;
        self.tab_press = None;
        self.group_header_press = None;
        self.invalidate_pane_surface();
        outcome.repaint = true;
        outcome.resize = true;
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

    /// Zen mode's work for the client loop, between events: zoom requests, and leaving zen mode
    /// when a config reload took its key away.
    fn take_zen_zoom(&mut self) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        self.sync_zen_zoom(&mut outcome);
        outcome
    }

    /// Zooms the tab on screen while zen mode is on, and keeps track of the zooms it made: once
    /// zen mode is off, each is undone, by its pane, as soon as a window shows its tab zoomed.
    /// A zoom request without a pane acts on the pane the server shows this window, and the
    /// answer names it. A request naming a pane also focuses it, so zen mode sends nothing while
    /// the window's own requests may have taken it elsewhere: the server then shows the window
    /// the tab its snapshot shows. Runs between the client's events, so it allocates nothing
    /// unless something changed.
    fn sync_zen_zoom(&mut self, outcome: &mut ClientShellInput) {
        // Only the zen mode key leaves zen mode: a reload that unbinds it leaves it too.
        if self.zen.on && self.config.keybinds.keybinds.zen_mode.bindings.is_empty() {
            self.switch_zen(outcome);
        }
        self.track_zen_zooms(outcome);
        if self.zen.on
            && self.endpoint_is_online(&self.active_endpoint_id)
            && self.zen_may_zoom()
            && self.zen_settled()
        {
            self.ask_zen_zoom(outcome);
        }
        if self.zen.unsaved {
            self.zen.unsaved = false;
            self.persist_chrome_preferences(outcome);
        }
    }

    /// Asks for the zoom of the tab on screen, and records it as a guess, the tab and its
    /// focused pane, until the answer names the pane.
    fn ask_zen_zoom(&mut self, outcome: &mut ClientShellInput) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let (Some(tab_id), Some(pane_id)) = (
            snapshot.focused_tab_id.clone(),
            snapshot.focused_pane_id.clone(),
        ) else {
            return;
        };
        let panes = snapshot
            .panes
            .iter()
            .filter(|pane| pane.tab_id == tab_id)
            .map(|pane| pane.pane_id.clone())
            .collect();
        let mark = || ZenSnapshotMark::of(snapshot, self.active_snapshot_generation);
        let tab = ZenTab {
            endpoint_id: self.active_endpoint_id.clone(),
            tab_id,
        };
        let (asked, answered_on) = (mark(), mark());
        self.zen.asked = Some(ZenAsked {
            tab: tab.clone(),
            on: asked,
        });
        let Some(request_id) = self.push_zen_zoom(None, PaneZoomMode::On, outcome) else {
            return;
        };
        self.zen.zoomed.retain(|zoom| zoom.tab != tab);
        self.zen.zoomed.push(ZenZoom {
            tab,
            pane_id,
            panes,
            answered_on: Some(answered_on),
            asked_by: Some(request_id),
        });
        self.zen.unsaved = true;
    }

    /// Whether the tab on screen needs zen mode's zoom: it shows several panes side by side, it
    /// isn't one you zoomed or unzoomed yourself, and its zoom wasn't asked for on this
    /// snapshot already.
    fn zen_may_zoom(&self) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let Some(tab) = snapshot
            .focused_tab_id
            .as_ref()
            .and_then(|tab_id| snapshot.tabs.iter().find(|tab| &tab.tab_id == tab_id))
        else {
            return false;
        };
        let on_screen = |zen_tab: &ZenTab| {
            zen_tab.endpoint_id == self.active_endpoint_id && zen_tab.tab_id == tab.tab_id
        };
        let panes = snapshot
            .panes
            .iter()
            .filter(|pane| pane.tab_id == tab.tab_id)
            .count();
        let asked = self.zen.asked.as_ref().is_some_and(|asked| {
            on_screen(&asked.tab) && asked.on.covers(snapshot, self.active_snapshot_generation)
        });
        !tab.zoomed && panes >= 2 && !asked && !self.zen.yours.iter().any(on_screen)
    }

    /// Whether the server shows the window the tab and pane its snapshot shows, as far as the
    /// window's own requests go: none is on its way, and since the last answer to one that can
    /// take the window elsewhere, a newer snapshot came.
    fn zen_settled(&self) -> bool {
        let moved = self
            .zen
            .moved_on
            .as_ref()
            .zip(self.snapshot.as_deref())
            .is_some_and(|(moved_on, snapshot)| {
                moved_on.covers(snapshot, self.active_snapshot_generation)
            });
        !moved
            && self
                .pending_requests
                .keys()
                .all(|id| self.zen.requests.iter().any(|(zen_id, _)| zen_id == id))
    }

    /// Follows zen mode's zooms on the machine on screen through the snapshot, and undoes the
    /// one on screen if it is still zen mode's and zen mode is off or the tab is one you set
    /// yourself. Forgets a zoom whose tab or pane is gone or whose tab someone unzoomed. A zoom
    /// it can't undo yet is looked at again with the next snapshot or answer.
    fn track_zen_zooms(&mut self, outcome: &mut ClientShellInput) {
        if self.zen.zoomed.is_empty() || !self.endpoint_is_online(&self.active_endpoint_id) {
            return;
        }
        let settled = self.zen_settled();
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let generation = self.active_snapshot_generation;
        if self
            .zen
            .tracked
            .as_ref()
            .is_some_and(|tracked| tracked.covers(snapshot, generation))
        {
            return;
        }
        self.zen.tracked = Some(ZenSnapshotMark::of(snapshot, generation));
        let endpoint_id = &self.active_endpoint_id;
        let on = self.zen.on;
        let yours = &self.zen.yours;
        let requests = &self.zen.requests;
        let mut unsaved = false;
        let mut undo = None;
        self.zen.zoomed.retain_mut(|zoom| {
            if &zoom.tab.endpoint_id != endpoint_id {
                return true;
            }
            let Some(tab) = snapshot
                .tabs
                .iter()
                .find(|tab| tab.tab_id == zoom.tab.tab_id)
            else {
                unsaved = true;
                return false;
            };
            let tab_panes = || {
                snapshot
                    .panes
                    .iter()
                    .filter(|pane| pane.tab_id == tab.tab_id)
            };
            if !tab_panes().any(|pane| pane.pane_id == zoom.pane_id) {
                unsaved = true;
                return false;
            }
            if zoom.panes.is_empty() {
                zoom.panes = tab_panes().map(|pane| pane.pane_id.clone()).collect();
            }
            // An answer forgotten among too many requests isn't coming.
            if zoom
                .asked_by
                .as_ref()
                .is_some_and(|asked_by| !requests.iter().any(|(id, _)| id == asked_by))
            {
                zoom.asked_by = None;
            }
            if !tab.zoomed {
                // Unzoomed since: nothing left to undo.
                let unzoomed = zoom.asked_by.is_none()
                    && zoom
                        .answered_on
                        .as_ref()
                        .is_none_or(|answered_on| !answered_on.covers(snapshot, generation));
                unsaved |= unzoomed;
                return !unzoomed;
            }
            zoom.answered_on = None;
            let Some(focused) = snapshot
                .focused_pane_id
                .as_deref()
                .filter(|_| snapshot.focused_tab_id.as_deref() == Some(tab.tab_id.as_str()))
            else {
                return true;
            };
            if focused != zoom.pane_id && zoom.panes.iter().any(|pane| pane == focused) {
                // You moved to another of its panes, and the zoom went with you.
                zoom.pane_id = focused.to_owned();
                unsaved = true;
            }
            let overlaid = tab_panes().any(|pane| !zoom.panes.contains(&pane.pane_id));
            if focused == zoom.pane_id
                && (!on || yours.contains(&zoom.tab))
                && !overlaid
                && zoom.asked_by.is_none()
                && settled
            {
                undo = Some(zoom.pane_id.clone());
                unsaved = true;
                return false;
            }
            true
        });
        self.zen.unsaved |= unsaved;
        if let Some(pane_id) = undo {
            self.push_zen_zoom(Some(pane_id), PaneZoomMode::Off, outcome);
        }
    }

    /// Sends one of zen mode's zoom requests and returns its ID.
    fn push_zen_zoom(
        &mut self,
        pane_id: Option<String>,
        mode: PaneZoomMode,
        outcome: &mut ClientShellInput,
    ) -> Option<String> {
        let method = Method::PaneZoom(PaneZoomParams { pane_id, mode });
        if !self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome) {
            return None;
        }
        let Some(ClientShellAction::Endpoint {
            endpoint_id,
            request,
            ..
        }) = outcome.actions.last()
        else {
            return None;
        };
        if self.zen.requests.len() == ZEN_REQUESTS_KEPT {
            self.zen.requests.remove(0);
        }
        self.zen
            .requests
            .push((request.id.clone(), endpoint_id.clone()));
        Some(request.id.clone())
    }

    /// Every answer to the window's requests passes here. Takes the answer to one of zen mode's
    /// zooms, which names the pane it zoomed: perhaps on a tab other than the one on screen when
    /// it was asked for, as the window moved on meanwhile. Zen mode keeps it to undo like any
    /// other. Failures show no notice, and without an answer zen mode's guess stays. Returns
    /// whether the answer was zen mode's.
    ///
    /// The answer to your own zoom, by key or pane menu, makes its tab yours: the answer names
    /// the tab, which the snapshot may not show yet.
    pub(super) fn zen_answer(
        &mut self,
        boot_id: &str,
        request_id: &str,
        method_name: &str,
        result: &Result<ResponseResult, ClientShellEndpointError>,
    ) -> bool {
        self.zen.tracked = None;
        let Some(index) = self
            .zen
            .requests
            .iter()
            .position(|(id, _)| id == request_id)
        else {
            self.note_window_answer(method_name, result);
            return false;
        };
        let (_, endpoint_id) = self.zen.requests.remove(index);
        let guess = self
            .zen
            .zoomed
            .iter()
            .position(|zoom| zoom.asked_by.as_deref() == Some(request_id));
        let zoom = match result {
            Ok(ResponseResult::PaneZoom { zoom }) => zoom,
            _ => {
                if let Some(guess) = guess {
                    self.zen.zoomed[guess].asked_by = None;
                }
                return true;
            }
        };
        if let Some(guess) = guess {
            self.zen.zoomed.remove(guess);
            self.zen.unsaved = true;
        }
        if !zoom.zoom_changed || !zoom.zoomed {
            return true;
        }
        let answered_on = self
            .snapshot
            .as_deref()
            .filter(|snapshot| {
                endpoint_id == self.active_endpoint_id && snapshot.boot_id == boot_id
            })
            .map(|snapshot| ZenSnapshotMark::of(snapshot, self.active_snapshot_generation));
        let tab = ZenTab {
            endpoint_id,
            tab_id: zoom.layout.tab_id.clone(),
        };
        self.zen.zoomed.retain(|zoomed| zoomed.tab != tab);
        self.zen.zoomed.push(ZenZoom {
            tab,
            pane_id: zoom.pane_id.clone(),
            panes: zoom
                .layout
                .panes
                .iter()
                .map(|pane| pane.pane_id.clone())
                .collect(),
            answered_on,
            asked_by: None,
        });
        self.zen.unsaved = true;
        true
    }

    /// Whether the snapshot already shows where a focus request's answer says the window is: a
    /// click on the tab, space or pane on screen takes the window nowhere, and no snapshot
    /// follows it.
    fn shows_the_answer(&self, result: &ResponseResult) -> bool {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        match result {
            ResponseResult::TabInfo { tab } => {
                snapshot.focused_tab_id.as_deref() == Some(tab.tab_id.as_str())
            }
            ResponseResult::PaneInfo { pane } => {
                snapshot.focused_pane_id.as_deref() == Some(pane.pane_id.as_str())
            }
            ResponseResult::WorkspaceInfo { workspace } => {
                snapshot.focused_tab_id.as_deref() == Some(workspace.active_tab_id.as_str())
            }
            _ => false,
        }
    }

    /// The answer to one of the window's own requests: one that can have taken the window
    /// elsewhere holds zen mode back until the next snapshot, and your own zoom in zen mode
    /// makes its tab yours.
    fn note_window_answer(
        &mut self,
        method_name: &str,
        result: &Result<ResponseResult, ClientShellEndpointError>,
    ) {
        let Ok(result) = result else {
            return;
        };
        if !ZEN_STILL_METHODS.contains(&method_name) && !self.shows_the_answer(result) {
            self.zen.moved_on = self
                .snapshot
                .as_deref()
                .map(|snapshot| ZenSnapshotMark::of(snapshot, self.active_snapshot_generation));
        }
        let ResponseResult::PaneZoom { zoom } = result else {
            return;
        };
        if !self.zen.on {
            return;
        }
        let tab = ZenTab {
            endpoint_id: self.active_endpoint_id.clone(),
            tab_id: zoom.layout.tab_id.clone(),
        };
        let before = self.zen.zoomed.len();
        self.zen.zoomed.retain(|zoom| zoom.tab != tab);
        self.zen.unsaved |= self.zen.zoomed.len() != before;
        if !self.zen.yours.contains(&tab) {
            self.zen.yours.push(tab);
        }
    }
}

impl crate::client::state::ClientState {
    /// Called by the client loop between events, so a snapshot that shows a split or a closed
    /// pane gets its tab zoomed again, a tab zen mode zoomed is unzoomed once on screen with zen
    /// mode off, and a reload that unbinds the zen mode key leaves zen mode. Waits while a
    /// machine switch is under way.
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
        let outcome = shell.take_zen_zoom();
        if outcome.resize {
            let resize = crate::client::shell_runtime::client_shell_resize_message(
                shell,
                self.reported_size.0,
                self.reported_size.1,
                self.reported_cell_size.0,
                self.reported_cell_size.1,
                self.pixel_geometry_exact,
            );
            crate::client::transport::write_to_server(endpoints, &resize)
                .map_err(crate::client::ClientError::ConnectionLost)?;
        }
        if outcome.actions.is_empty() && !outcome.repaint {
            return Ok(());
        }
        let (_, repaint) = crate::client::shell_runtime::dispatch_client_shell_actions(
            outcome.actions,
            endpoint_commands,
            endpoints,
            Some(shell),
            &mut self.detached_process_children,
            scheduled_activation,
        )?;
        if let Some(frame) = (repaint || outcome.repaint)
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
    use crate::api::schema::{
        PaneLayoutPane, PaneLayoutRect, PaneLayoutSnapshot, PaneZoomResult, TabInfo,
    };
    use crate::config::Config;

    /// The zen mode key, which a config of zen mode's tests binds.
    fn with_zen_key(mut config: Config) -> Config {
        config.keys.zen_mode = crate::config::BindingConfig::one("prefix+shift+z");
        config
    }

    fn zen_config(width: u16) -> ClientShellConfig {
        let mut config = with_zen_key(Config::default());
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

    /// Tab tab_1 with panes pane_1 (focused) and pane_2, tab tab_2 with pane_3, and tab tab_3
    /// with pane_5 (focused) and pane_6. The tabs in `zoomed` are zoomed.
    fn three_tabs(revision: u64, focused_tab: &str, zoomed: &[&str]) -> Box<ClientShellSnapshot> {
        let mut snapshot = two_tabs(revision, focused_tab, false);
        let mut tab = snapshot.tabs[1].clone();
        tab.tab_id = "tab_3".into();
        snapshot.tabs.push(tab);
        for tab in &mut snapshot.tabs {
            tab.zoomed = zoomed.contains(&tab.tab_id.as_str());
            tab.focused = tab.tab_id == focused_tab;
        }
        for pane_id in ["pane_5", "pane_6"] {
            let mut pane = snapshot.panes[2].clone();
            pane.pane_id = pane_id.into();
            pane.tab_id = "tab_3".into();
            snapshot.panes.push(pane);
        }
        let focused_pane = match focused_tab {
            "tab_1" => "pane_1",
            "tab_2" => "pane_3",
            _ => "pane_5",
        };
        for pane in &mut snapshot.panes {
            pane.focused = pane.pane_id == focused_pane;
        }
        snapshot.focused_pane_id = Some(focused_pane.into());
        snapshot
    }

    fn press(state: &mut ClientShellState, action: KeybindAction) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
        outcome
    }

    /// Zen mode's work between two events.
    fn take(state: &mut ClientShellState) -> Vec<ClientShellAction> {
        state.take_zen_zoom().actions
    }

    /// Zen mode's zoom requests: the pane each names, if any, and on or off.
    fn zooms(actions: &[ClientShellAction]) -> Vec<(Option<String>, PaneZoomMode)> {
        actions
            .iter()
            .map(|action| match action {
                ClientShellAction::Endpoint { request, .. } => match &request.method {
                    Method::PaneZoom(params) => (params.pane_id.clone(), params.mode),
                    method => panic!("expected a pane zoom, got {method:?}"),
                },
                action => panic!("expected an endpoint request, got {action:?}"),
            })
            .collect()
    }

    /// A zoom of the pane the server shows the window.
    fn on() -> (Option<String>, PaneZoomMode) {
        (None, PaneZoomMode::On)
    }

    fn off(pane_id: &str) -> (Option<String>, PaneZoomMode) {
        (Some(pane_id.into()), PaneZoomMode::Off)
    }

    /// The server's answer to request `request_id`: it zoomed `pane_id`, on `tab_id` with
    /// `panes`.
    fn answer_request(
        state: &mut ClientShellState,
        request_id: &str,
        (tab_id, pane_id, panes): (&str, &str, &[&str]),
    ) {
        let area = PaneLayoutRect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let zoom = PaneZoomResult {
            changed: true,
            zoom_changed: true,
            focus_changed: false,
            reason: None,
            pane_id: pane_id.into(),
            focused_pane_id: pane_id.into(),
            zoomed: true,
            layout: PaneLayoutSnapshot {
                workspace_id: "ws_1".into(),
                tab_id: tab_id.into(),
                zoomed: true,
                area,
                focused_pane_id: pane_id.into(),
                panes: panes
                    .iter()
                    .map(|pane| PaneLayoutPane {
                        pane_id: (*pane).into(),
                        focused: *pane == pane_id,
                        rect: area,
                    })
                    .collect(),
                splits: Vec::new(),
            },
        };
        let boot_id = state.snapshot.as_ref().expect("snapshot").boot_id.clone();
        let (_, actions) = state.handle_endpoint_result(
            &boot_id,
            request_id,
            Ok(ResponseResult::PaneZoom { zoom }),
        );
        assert!(actions.is_empty());
    }

    /// The server's answer to the one request in `asked`, zen mode's zoom or your own.
    fn answer(
        state: &mut ClientShellState,
        asked: &[ClientShellAction],
        zoomed: (&str, &str, &[&str]),
    ) {
        let [ClientShellAction::Endpoint { request, .. }] = asked else {
            panic!("expected one request, got {asked:?}");
        };
        answer_request(state, &request.id, zoomed);
    }

    /// The server's answer to the one request in `asked`.
    fn answer_with(
        state: &mut ClientShellState,
        asked: &[ClientShellAction],
        result: Result<ResponseResult, ClientShellEndpointError>,
    ) {
        let [ClientShellAction::Endpoint { request, .. }] = asked else {
            panic!("expected one request, got {asked:?}");
        };
        let boot_id = state.snapshot.as_ref().expect("snapshot").boot_id.clone();
        let (_, actions) = state.handle_endpoint_result(&boot_id, &request.id, result);
        assert!(actions.is_empty());
    }

    /// A request of the window's own, such as a click.
    fn window_request(state: &mut ClientShellState, method: Method) -> Vec<ClientShellAction> {
        let mut outcome = ClientShellInput::default();
        state.push_endpoint_method(method, &mut outcome);
        outcome.actions
    }

    /// The answer to a click on `tab_id`.
    fn tab_focused(tab_id: &str) -> Result<ResponseResult, ClientShellEndpointError> {
        Ok(ResponseResult::TabInfo {
            tab: TabInfo {
                tab_id: tab_id.into(),
                workspace_id: "ws_1".into(),
                number: 1,
                label: "tab".into(),
                focused: true,
                pane_count: 2,
                agent_status: crate::api::schema::AgentStatus::Idle,
            },
        })
    }

    /// The server zoomed tab_1's pane_1.
    const TAB_1: (&str, &str, &[&str]) = ("tab_1", "pane_1", &["pane_1", "pane_2"]);
    /// The server zoomed tab_3's pane_5.
    const TAB_3: (&str, &str, &[&str]) = ("tab_3", "pane_5", &["pane_5", "pane_6"]);

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
        assert!(take(&mut state).is_empty());

        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        assert_eq!(zooms(&asked), [on()]);
        // Asked once, not again while the answer is on its way.
        assert!(take(&mut state).is_empty());
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());

        // A tab with a single pane needs no zoom.
        state.set_snapshot(three_tabs(3, "tab_2", &["tab_1"]));
        assert!(take(&mut state).is_empty());

        // Off: tab_1 is unzoomed, by its pane, when it is back on screen, not before.
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        assert!(take(&mut state).is_empty());
        state.set_snapshot(three_tabs(4, "tab_1", &["tab_1"]));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn zen_mode_leaves_a_zoom_it_did_not_make() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", true));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn a_tab_you_unzoom_in_zen_mode_stays_unzoomed() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        let yours = press(&mut state, KeybindAction::Zoom).actions;
        answer(&mut state, &yours, TAB_1);
        state.set_snapshot(two_tabs(3, "tab_1", false));
        assert!(take(&mut state).is_empty());
        // Yours now: leaving zen mode doesn't touch it.
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
    }

    #[test]
    fn a_tab_you_unzoom_and_zoom_again_in_zen_mode_stays_zoomed_after_it() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        assert_eq!(zooms(&asked), [on()]);
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        let yours = press(&mut state, KeybindAction::Zoom).actions;
        answer(&mut state, &yours, TAB_1);
        state.set_snapshot(two_tabs(3, "tab_1", false));
        assert!(take(&mut state).is_empty());
        let yours = press(&mut state, KeybindAction::Zoom).actions;
        answer(&mut state, &yours, TAB_1);
        state.set_snapshot(two_tabs(4, "tab_1", true));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // Nor when the window shows the tab again later.
        state.set_snapshot(three_tabs(5, "tab_2", &["tab_1"]));
        state.set_snapshot(three_tabs(6, "tab_1", &["tab_1"]));
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn a_new_connection_counts_as_a_newer_snapshot() {
        let mut state = ClientShellState::new(zen_config(150));
        let local = ClientEndpointId::Local;
        state.set_endpoint_snapshot_for_generation(&local, 1, two_tabs(1, "tab_1", false));
        press(&mut state, KeybindAction::ZenMode);
        state.set_endpoint_snapshot_for_generation(&local, 1, two_tabs(2, "tab_1", true));
        state.set_endpoint_snapshot_for_generation(&local, 1, two_tabs(5, "tab_1", false));
        assert_eq!(zooms(&take(&mut state)), [on()]);
        // The connection drops before that zoom lands. The next one counts revisions from 1.
        state.set_endpoint_snapshot_for_generation(&local, 2, two_tabs(1, "tab_1", false));
        assert_eq!(zooms(&take(&mut state)), [on()]);
        assert!(take(&mut state).is_empty());
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
        let yours = press(&mut state, KeybindAction::Zoom).actions;
        answer(&mut state, &yours, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", false));
        assert!(take(&mut state).is_empty());
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
        answer(&mut state, &outcome.actions, TAB_1);
        // The zoom focuses pane_1 and unzooms tab_1, which zen mode then leaves alone.
        state.set_snapshot(two_tabs(3, "tab_1", false));
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn a_tab_the_server_unzooms_in_zen_mode_is_zoomed_again() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        assert_eq!(zooms(&asked), [on()]);
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        // prefix+v: the server adds pane_4 and unzooms the tab, as any split or closed pane does.
        let mut split = two_tabs(3, "tab_1", false);
        let mut pane = split.panes[0].clone();
        pane.pane_id = "pane_4".into();
        split.panes.push(pane);
        state.set_snapshot(split);
        assert_eq!(zooms(&take(&mut state)), [on()]);
        // Asked once for this snapshot.
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn leaving_zen_mode_before_its_zoom_lands_still_unzooms() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        assert_eq!(zooms(&asked), [on()]);
        // Off again before the zoom lands: the pane it zoomed is unzoomed once it shows.
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        answer(&mut state, &asked, TAB_1);
        assert!(take(&mut state).is_empty());
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn a_zoom_that_landed_on_another_tab_is_asked_again() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_2", false));
        press(&mut state, KeybindAction::ZenMode);
        // tab_1 is on screen for a moment; its zoom lands on the tab the server is on by then,
        // with a single pane, so it zooms nothing.
        state.set_snapshot(two_tabs(2, "tab_1", false));
        assert_eq!(zooms(&take(&mut state)), [on()]);
        state.set_snapshot(two_tabs(3, "tab_2", false));
        assert!(take(&mut state).is_empty());
        // Back on tab_1, still unzoomed: asked again.
        state.set_snapshot(two_tabs(4, "tab_1", false));
        assert_eq!(zooms(&take(&mut state)), [on()]);
    }

    #[test]
    fn zen_mode_keeps_its_zooms_across_a_session_restart() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());
        // The session restarts: a new boot, the same tabs, tab_1 restored zoomed.
        let mut restarted = two_tabs(1, "tab_1", true);
        restarted.boot_id = "boot-2".into();
        state.set_snapshot(restarted);
        assert!(take(&mut state).is_empty());
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [off("pane_1")]
        );
    }

    #[test]
    fn a_zoom_that_lands_on_another_tab_is_undone_there() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(three_tabs(1, "tab_1", &[]));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        assert_eq!(zooms(&asked), [on()]);
        // The window moved on to tab_3 before the zoom arrived, so it zoomed tab_3's pane.
        answer(&mut state, &asked, TAB_3);
        state.set_snapshot(three_tabs(2, "tab_3", &["tab_3"]));
        assert!(take(&mut state).is_empty());
        // Leaving zen mode there unzooms tab_3, by the pane it zoomed.
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [off("pane_5")]
        );
    }

    #[test]
    fn a_zoom_that_lands_on_a_tab_you_set_yourself_is_undone_at_once() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(three_tabs(1, "tab_3", &[]));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_3);
        state.set_snapshot(three_tabs(2, "tab_3", &["tab_3"]));
        // You unzoom tab_3: yours now.
        let yours = press(&mut state, KeybindAction::Zoom).actions;
        answer(&mut state, &yours, TAB_3);
        state.set_snapshot(three_tabs(3, "tab_3", &[]));
        assert!(take(&mut state).is_empty());
        // tab_1 shows for a moment, and its zoom lands on tab_3, where the window went back to.
        state.set_snapshot(three_tabs(4, "tab_1", &[]));
        let asked = take(&mut state);
        assert_eq!(zooms(&asked), [on()]);
        answer(&mut state, &asked, TAB_3);
        state.set_snapshot(three_tabs(5, "tab_3", &["tab_3"]));
        assert_eq!(zooms(&take(&mut state)), [off("pane_5")]);
        // Not zen mode's to undo again later.
        state.set_snapshot(three_tabs(6, "tab_3", &[]));
        assert!(take(&mut state).is_empty());
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
    }

    #[test]
    fn zen_mode_names_a_pane_only_once_the_windows_own_requests_are_through() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(three_tabs(1, "tab_1", &[]));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(three_tabs(2, "tab_1", &["tab_1"]));
        assert!(take(&mut state).is_empty());
        // A click on tab_3 is on its way when zen mode is left on tab_1: unzooming tab_1 by its
        // pane now would pull the window back to it.
        let mut click = ClientShellInput::default();
        state.push_endpoint_method(
            Method::TabFocus(crate::api::schema::TabTarget {
                tab_id: "tab_3".into(),
            }),
            &mut click,
        );
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // Its answer comes before the snapshot that shows tab_3: still nothing.
        answer_with(&mut state, &click.actions, tab_focused("tab_3"));
        assert!(take(&mut state).is_empty());
        // tab_3 shows; tab_1, off screen, waits until it shows again.
        state.set_snapshot(three_tabs(3, "tab_3", &["tab_1"]));
        assert!(take(&mut state).is_empty());
        state.set_snapshot(three_tabs(4, "tab_1", &["tab_1"]));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
    }

    /// A plugin's overlay pane, pane_9, opened in tab_1 over pane_1: it takes the zoom.
    fn overlay(revision: u64) -> Box<ClientShellSnapshot> {
        let mut snapshot = two_tabs(revision, "tab_1", true);
        let mut pane = snapshot.panes[0].clone();
        pane.pane_id = "pane_9".into();
        snapshot.panes[0].focused = false;
        snapshot.panes.push(pane);
        snapshot.focused_pane_id = Some("pane_9".into());
        snapshot
    }

    #[test]
    fn leaving_zen_mode_leaves_an_overlay_whole_and_unzooms_once_it_closes() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());
        state.set_snapshot(overlay(3));
        assert!(take(&mut state).is_empty());

        // The overlay keeps the whole tab.
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        state.set_snapshot(overlay(4));
        assert!(take(&mut state).is_empty());

        // It closes and hands the zoom back to pane_1, which zen mode zoomed: undone.
        state.set_snapshot(two_tabs(5, "tab_1", true));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
        assert!(take(&mut state).is_empty());
    }

    #[test]
    fn an_overlay_closing_while_its_tab_is_off_screen_hands_the_zoom_back() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(overlay(2));
        assert!(take(&mut state).is_empty());
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // Off screen, the overlay closes.
        state.set_snapshot(three_tabs(3, "tab_2", &["tab_1"]));
        assert!(take(&mut state).is_empty());
        state.set_snapshot(three_tabs(4, "tab_1", &["tab_1"]));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
    }

    #[test]
    fn moving_to_another_pane_of_a_zoomed_tab_takes_zen_modes_zoom_along() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());
        // prefix+arrow to pane_2: the tab stays zoomed, now on pane_2.
        let mut moved = two_tabs(3, "tab_1", true);
        moved.panes[0].focused = false;
        moved.panes[1].focused = true;
        moved.focused_pane_id = Some("pane_2".into());
        state.set_snapshot(moved);
        assert!(take(&mut state).is_empty());
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [off("pane_2")]
        );
    }

    #[test]
    fn a_new_window_undoes_the_zooms_a_closed_one_left() {
        let path =
            std::env::temp_dir().join(format!("herdr-zen-preferences-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let config = || zen_config(150).with_preferences_path(path.clone());
        let mut window = ClientShellState::new(config());
        window.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut window, KeybindAction::ZenMode).actions;
        answer(&mut window, &asked, TAB_1);
        assert!(take(&mut window).is_empty());
        // The window closes in zen mode.
        drop(window);

        let mut window = ClientShellState::new(config());
        assert!(!window.zen_on());
        // Meanwhile you moved to pane_2, which the zoom shows now.
        window.set_snapshot(on_pane_2(two_tabs(1, "tab_1", true)));
        assert_eq!(zooms(&take(&mut window)), [off("pane_2")]);
        assert!(take(&mut window).is_empty());
        let saved = std::fs::read_to_string(&path).expect("preferences");
        assert!(!saved.contains("pane_1"), "{saved}");
        std::fs::remove_file(path).expect("remove preferences");
    }

    /// `snapshot` with tab_1's pane_2 focused.
    fn on_pane_2(mut snapshot: Box<ClientShellSnapshot>) -> Box<ClientShellSnapshot> {
        for pane in &mut snapshot.panes {
            pane.focused = pane.pane_id == "pane_2";
        }
        for tab in &mut snapshot.tabs {
            tab.focused = tab.tab_id == "tab_1";
        }
        snapshot.focused_tab_id = Some("tab_1".into());
        snapshot.focused_pane_id = Some("pane_2".into());
        snapshot
    }

    #[test]
    fn a_jump_to_another_pane_of_a_zen_tab_after_zen_mode_unzooms_it_there() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(three_tabs(1, "tab_1", &[]));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(three_tabs(2, "tab_1", &["tab_1"]));
        assert!(take(&mut state).is_empty());
        state.set_snapshot(three_tabs(3, "tab_2", &["tab_1"]));
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // A notification or the agents panel takes you straight to pane_2: the server keeps
        // tab_1 zoomed, now on pane_2.
        state.set_snapshot(on_pane_2(three_tabs(4, "tab_1", &["tab_1"])));
        assert_eq!(zooms(&take(&mut state)), [off("pane_2")]);
    }

    #[test]
    fn moving_to_another_pane_behind_an_overlay_keeps_the_zoom_zen_modes() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        state.set_snapshot(overlay(3));
        assert!(take(&mut state).is_empty());
        // From the overlay to pane_2, the tab still zoomed, the overlay still open.
        let mut moved = overlay(4);
        for pane in &mut moved.panes {
            pane.focused = pane.pane_id == "pane_2";
        }
        moved.focused_pane_id = Some("pane_2".into());
        state.set_snapshot(moved.clone());
        assert!(take(&mut state).is_empty());
        // Off while the overlay is open: it keeps the zoom.
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // It closes while not focused, which leaves the tab zoomed on pane_2: zen mode's.
        moved.revision = 5;
        moved.panes.retain(|pane| pane.pane_id != "pane_9");
        state.set_snapshot(moved);
        assert_eq!(zooms(&take(&mut state)), [off("pane_2")]);
    }

    #[test]
    fn an_undo_held_back_by_a_scroll_goes_out_with_its_answer() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());
        let scroll = window_request(
            &mut state,
            Method::PaneScroll(crate::api::schema::PaneScrollParams {
                pane_id: "pane_1".into(),
                offset_from_bottom: 3,
            }),
        );
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        // A scroll changes nothing a snapshot shows, so none follows its answer.
        answer_with(
            &mut state,
            &scroll,
            Ok(ResponseResult::Pong {
                version: "test".into(),
                protocol: 0,
                capabilities: None,
            }),
        );
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
    }

    #[test]
    fn a_click_on_the_tab_on_screen_holds_nothing_back() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());
        let click = window_request(
            &mut state,
            Method::TabFocus(crate::api::schema::TabTarget {
                tab_id: "tab_1".into(),
            }),
        );
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        answer_with(&mut state, &click, tab_focused("tab_1"));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
    }

    #[test]
    fn a_zoom_whose_answer_never_comes_is_still_undone() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        // The connection drops with the answer on its way.
        answer_with(
            &mut state,
            &asked,
            Err(ClientShellEndpointError {
                code: Some("endpoint_cancelled".into()),
                message: "cancelled".into(),
            }),
        );
        assert!(state.visible_endpoint_notice.is_none());
        state.set_snapshot(two_tabs(2, "tab_1", true));
        assert!(take(&mut state).is_empty());
        assert_eq!(
            zooms(&press(&mut state, KeybindAction::ZenMode).actions),
            [off("pane_1")]
        );
    }

    #[test]
    fn your_zoom_right_after_a_tab_switch_counts_for_the_tab_switched_to() {
        let mut state = ClientShellState::new(zen_config(150));
        state.set_snapshot(three_tabs(1, "tab_1", &[]));
        let asked = press(&mut state, KeybindAction::ZenMode).actions;
        answer(&mut state, &asked, TAB_1);
        state.set_snapshot(three_tabs(2, "tab_1", &["tab_1"]));
        assert!(take(&mut state).is_empty());
        // A click on tab_3, and the zoom key before the snapshot shows tab_3.
        let click = window_request(
            &mut state,
            Method::TabFocus(crate::api::schema::TabTarget {
                tab_id: "tab_3".into(),
            }),
        );
        let yours = press(&mut state, KeybindAction::Zoom).actions;
        answer_with(&mut state, &click, tab_focused("tab_3"));
        answer(&mut state, &yours, TAB_3);
        state.set_snapshot(three_tabs(3, "tab_3", &["tab_1", "tab_3"]));
        // tab_3 is yours; tab_1 is still zen mode's, undone once it shows.
        assert!(take(&mut state).is_empty());
        assert!(press(&mut state, KeybindAction::ZenMode).actions.is_empty());
        state.set_snapshot(three_tabs(4, "tab_1", &["tab_1", "tab_3"]));
        assert_eq!(zooms(&take(&mut state)), [off("pane_1")]);
    }

    #[test]
    fn the_window_s_still_requests_are_endpoint_methods() {
        let methods = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/endpoint-method-shapes-v1.json"
        ));
        let methods: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(methods).expect("method shapes");
        let resolve = crate::api::api_method_name(&Method::PaneLinkResolve(
            crate::api::schema::PaneLinkActivateParams {
                pane_id: "pane_1".into(),
                viewport_row: 0,
                col: 0,
                content_revision: None,
                offset_from_bottom: None,
            },
        ));
        for method in ZEN_STILL_METHODS {
            assert!(
                methods.contains_key(*method) || *method == resolve,
                "{method}"
            );
        }
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
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(two_tabs(1, "tab_1", false));
        assert!(!bar(&mut state).contains("zen"));
    }

    #[test]
    fn width_keys_act_in_zen_mode_and_reach_the_pane_outside_it() {
        let mut config = with_zen_key(Config::default());
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

    /// Runs the client loop's zen mode step and returns what it sent to the server: its
    /// requests, and whether it resized the panes.
    fn run_loop_step(
        state: &mut crate::client::state::ClientState,
        endpoints: &mut crate::client::endpoint::EndpointRegistry,
        sent: &std::sync::Mutex<Vec<crate::protocol::ClientMessage>>,
        pending_activation: &Option<crate::client::endpoint::PendingEndpointActivation>,
        scheduled_activation: &mut Option<crate::client::events::ClientLoopEvent>,
    ) -> (Vec<crate::api::schema::Request>, bool) {
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
        let mut resized = false;
        let requests = sent
            .lock()
            .unwrap()
            .drain(..)
            .filter_map(|message| match message {
                crate::protocol::ClientMessage::ClientShellEndpointRequest { request, .. } => {
                    Some(serde_json::from_str(&request).unwrap())
                }
                crate::protocol::ClientMessage::ClientShellResize { .. } => {
                    resized = true;
                    None
                }
                message => panic!("expected an endpoint request, got {message:?}"),
            })
            .collect();
        (requests, resized)
    }

    /// Runs the client loop's zen mode step and returns the zooms it sent to the server.
    fn loop_zooms(
        state: &mut crate::client::state::ClientState,
        endpoints: &mut crate::client::endpoint::EndpointRegistry,
        sent: &std::sync::Mutex<Vec<crate::protocol::ClientMessage>>,
        pending_activation: &Option<crate::client::endpoint::PendingEndpointActivation>,
        scheduled_activation: &mut Option<crate::client::events::ClientLoopEvent>,
    ) -> Vec<(Option<String>, PaneZoomMode)> {
        let (requests, resized) = run_loop_step(
            state,
            endpoints,
            sent,
            pending_activation,
            scheduled_activation,
        );
        assert!(!resized);
        requests
            .into_iter()
            .map(|request| match request.method {
                Method::PaneZoom(params) => (params.pane_id, params.mode),
                method => panic!("expected a pane zoom, got {method:?}"),
            })
            .collect()
    }

    /// The server's answer to the zoom the client loop just asked for.
    fn answer_last(state: &mut crate::client::state::ClientState, zoomed: (&str, &str, &[&str])) {
        let shell = state.shell.as_mut().expect("shell");
        let (request_id, _) = shell
            .zen
            .requests
            .last()
            .cloned()
            .expect("a zoom on its way");
        answer_request(shell, &request_id, zoomed);
    }

    /// The window of `ClientState::test_new`, with the zen mode key bound.
    fn loop_state() -> crate::client::state::ClientState {
        let mut state = crate::client::state::ClientState::test_new();
        let shell = state.shell.as_mut().expect("shell");
        shell
            .config
            .apply_live_config(&with_zen_key(Config::default()), &[], &[]);
        state
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
        let mut state = loop_state();
        let shell = state.shell.as_mut().expect("shell");
        shell.set_snapshot(two_tabs(1, "tab_1", false));
        // The key's own zoom goes out with the key press.
        let asked = press(shell, KeybindAction::ZenMode).actions;
        assert_eq!(zooms(&asked), [on()]);
        answer(shell, &asked, TAB_1);
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
            [on()]
        );
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());
        answer_last(
            &mut state,
            ("tab_1", "pane_1", &["pane_1", "pane_2", "pane_4"]),
        );

        // pane_4 closes: the server unzooms the tab again.
        let shell = state.shell.as_mut().expect("shell");
        shell.set_snapshot(two_tabs(4, "tab_1", true));
        shell.set_snapshot(two_tabs(5, "tab_1", false));
        assert_eq!(
            loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle),
            [on()]
        );
        answer_last(&mut state, TAB_1);

        // Zen mode off on tab_2: tab_1 is unzoomed, by its pane, once the window shows it.
        let shell = state.shell.as_mut().expect("shell");
        shell.set_snapshot(three_tabs(6, "tab_2", &["tab_1"]));
        assert!(press(shell, KeybindAction::ZenMode).actions.is_empty());
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());
        state
            .shell
            .as_mut()
            .expect("shell")
            .set_snapshot(three_tabs(7, "tab_1", &["tab_1"]));
        assert_eq!(
            loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle),
            [off("pane_1")]
        );
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());
    }

    #[test]
    fn a_reload_that_unbinds_the_zen_mode_key_leaves_zen_mode() {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut endpoints = crate::client::endpoint::EndpointRegistry::new(
            Capture(sent.clone()),
            1,
            crate::client::endpoint::EndpointNegotiation::default(),
        );
        let mut state = loop_state();
        let shell = state.shell.as_mut().expect("shell");
        shell.set_snapshot(two_tabs(1, "tab_1", false));
        let asked = press(shell, KeybindAction::ZenMode).actions;
        answer(shell, &asked, TAB_1);
        shell.set_snapshot(two_tabs(2, "tab_1", true));
        let mut idle = None;
        assert!(loop_zooms(&mut state, &mut endpoints, &sent, &None, &mut idle).is_empty());

        // The reloaded config has no zen mode key, which was the only way out.
        let shell = state.shell.as_mut().expect("shell");
        shell.config.apply_live_config(&Config::default(), &[], &[]);
        let (requests, resized) =
            run_loop_step(&mut state, &mut endpoints, &sent, &None, &mut idle);
        let shell = state.shell.as_ref().expect("shell");
        assert!(!shell.zen_on());
        assert!(resized, "the panes go back to their usual size");
        assert!(matches!(
            requests.as_slice(),
            [crate::api::schema::Request {
                method: Method::PaneZoom(PaneZoomParams {
                    pane_id: Some(pane_id),
                    mode: PaneZoomMode::Off,
                }),
                ..
            }] if pane_id == "pane_1"
        ));
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
