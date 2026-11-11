//! Fork: the agents panel title's popup (`title_popup.rs`): the click, where it's drawn, the
//! title's tint, and the click's anchor through failures and resizes.

use super::*;

/// A window whose agents panel title has a popup, and a `prefix+t` popup key command.
fn title_state(cols: u16, rows: u16) -> ClientShellState {
    let mut projected = snapshot();
    projected.agent_panel_title = Some("7d 42%".into());
    projected.agent_panel_title_command_id = Some("cmd_title".into());
    projected
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_popup".into(),
            binding_label: "prefix+t".into(),
            binding_labels: crate::config::ActionKeybinds::prefix("t").labels(),
            action: crate::protocol::ClientShellCommandAction::Popup,
            description: None,
        });
    let mut config = Config::default();
    config.ui.sidebar_max_width = 60;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.sidebar_width = 40;
    state.sidebar_width_manual = true;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(cols, rows).expect("frame");
    state
}

/// Clicks the title; the id of the request it sends.
fn click_title(state: &mut ClientShellState) -> String {
    let title = state.hits.agent_panel_title;
    assert!(!title.is_empty(), "title drawn");
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: title.x + 1,
        row: title.y,
        modifiers: KeyModifiers::empty(),
    })]);
    match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected the title's invoke, got {other:?}"),
    }
}

fn open_key_popup(state: &mut ClientShellState) {
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("t"),
        label: "prefix+t".into(),
        command: "lazygit".into(),
        action: crate::config::CustomCommandAction::Popup,
        description: None,
        width: None,
        height: None,
    };
    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);
    assert!(state.popup_pending, "the key command waits for its popup");
}

/// A surface with a popup the title's click didn't open.
fn other_popup() -> PaneSurfaceFrame {
    let mut surface = surface_with_popup();
    surface.surface_revision = 2;
    if let Some(popup) = surface.popup.as_mut() {
        popup.terminal_id = "terminal-lazygit".into();
    }
    surface
}

/// Where a popup nobody clicked the title for sits.
fn centred(cols: u16, rows: u16) -> Rect {
    let mut fresh = title_state(cols, rows);
    fresh.set_pane_surface(other_popup());
    fresh.compose(cols, rows).expect("popup");
    fresh.hits.popup.as_ref().expect("popup").rect
}

fn popup_rect(state: &ClientShellState) -> Rect {
    state.hits.popup.as_ref().expect("popup").rect
}

fn failed(state: &mut ClientShellState, request: &str) {
    state.handle_endpoint_result(
        "boot-1",
        request,
        Err(ClientShellEndpointError {
            code: Some("command_failed".into()),
            message: "popup already open".into(),
        }),
    );
    assert!(!state.popup_pending);
}

#[test]
fn a_click_on_the_agents_panel_title_opens_its_popup() {
    let mut projected = snapshot();
    projected.agent_panel_title = Some("7d 42%".into());
    let mut config = Config::default();
    config.ui.sidebar_max_width = 60;
    let click = |projected: &ClientShellSnapshot| {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.sidebar_width = 40;
        state.sidebar_width_manual = true;
        state.set_snapshot(Box::new(projected.clone()));
        state.set_pane_surface(surface());
        state.compose(106, 30).expect("agent sidebar frame");
        let title = state.hits.agent_panel_title;
        assert_eq!(title.width, 6, "the title's text is the target");
        let outcome =
            state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: title.x + 2,
                row: title.y,
                modifiers: KeyModifiers::empty(),
            })]);
        (state.popup_pending, outcome.actions)
    };

    let (pending, actions) = click(&projected);
    assert!(!pending && actions.is_empty(), "no popup, nothing to open");

    projected.agent_panel_title_command_id = Some("cmd_title".into());
    let (pending, actions) = click(&projected);
    assert!(pending, "waits for the popup, as a popup key command does");
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected the popup's command, got {actions:?}");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command invocation");
    };
    assert_eq!(params.command_id, "cmd_title");
}

#[test]
fn the_agents_panel_title_popup_hangs_from_the_title_which_lights_up() {
    let mut projected = snapshot();
    projected.agent_panel_title = Some("7d 42%".into());
    projected.agent_panel_title_command_id = Some("cmd_title".into());
    let mut config = Config::default();
    config.ui.sidebar_max_width = 60;
    let fresh = || {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.sidebar_width = 40;
        state.sidebar_width_manual = true;
        state.set_snapshot(Box::new(projected.clone()));
        state.set_pane_surface(surface());
        let frame = state.compose(106, 30).expect("agent sidebar frame");
        (state, frame)
    };
    let (mut state, frame) = fresh();
    let title = state.hits.agent_panel_title;
    let fg_at = |frame: &crate::protocol::FrameData, x: u16| {
        frame.cells[usize::from(title.y) * usize::from(frame.width) + usize::from(x)].fg
    };
    let plain = fg_at(&frame, title.x);
    let mouse = |state: &mut ClientShellState, kind, column| {
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row: title.y,
            modifiers: KeyModifiers::empty(),
        })])
    };

    mouse(&mut state, MouseEventKind::Moved, title.x + 1);
    let frame = state.compose(106, 30).expect("hovered");
    assert_ne!(fg_at(&frame, title.x), plain, "tinted under the mouse");
    mouse(&mut state, MouseEventKind::Moved, title.x.saturating_sub(4));
    let frame = state.compose(106, 30).expect("left");
    assert_eq!(
        fg_at(&frame, title.x),
        plain,
        "and not once the mouse leaves"
    );

    let (mut popup_elsewhere, _) = fresh();
    popup_elsewhere.set_pane_surface(surface_with_popup());
    popup_elsewhere
        .compose(106, 30)
        .expect("someone else's popup");
    let centred = popup_elsewhere.hits.popup.as_ref().expect("popup").rect;
    assert_ne!(
        (centred.x, centred.y),
        (title.x, title.bottom()),
        "stays centred"
    );

    mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        title.x + 1,
    );
    assert!(state.popup_pending);
    state.set_pane_surface(surface_with_popup());
    let frame = state.compose(106, 30).expect("popup");
    let popup = state.hits.popup.as_ref().expect("popup").clone();
    assert_eq!(
        (
            popup.rect.x,
            popup.rect.y,
            popup.rect.width,
            popup.rect.height
        ),
        (title.x, title.bottom(), centred.width, centred.height),
        "hangs from the title, at the size the server gave it"
    );
    assert_eq!(
        (popup.inner_rect.x, popup.inner_rect.y),
        (title.x + 1, title.bottom() + 1)
    );
    assert_ne!(
        fg_at(&frame, title.x),
        plain,
        "the title stays lit while it's open"
    );

    // Without room below the title, it sits above it.
    let mut flipped = false;
    for rows in 12..30 {
        state.compose(106, rows).expect("window");
        let (title, placed) = (state.hits.agent_panel_title, state.hits.popup.as_ref());
        let placed = placed.expect("popup").rect;
        if title.bottom() + placed.height <= rows {
            assert_eq!(
                (placed.x, placed.y),
                (title.x, title.bottom()),
                "{rows} rows"
            );
        } else if title.y >= placed.height {
            assert_eq!(
                (placed.x, placed.bottom()),
                (title.x, title.y),
                "{rows} rows"
            );
            flipped = true;
        }
    }
    assert!(flipped, "some window was too short below the title");

    state.set_pane_surface(surface());
    state.compose(106, 30).expect("closed");
    assert!(
        state.title_popup_anchor.is_none(),
        "forgotten once it closes"
    );
}

#[test]
fn the_popup_stays_inside_a_window_that_shrinks_and_loses_the_title() {
    // Narrow enough for the mobile layout, which has no sidebar, and shorter than the title's row.
    let mut state = title_state(106, 50);
    click_title(&mut state);
    state.set_pane_surface(surface_with_popup());
    state.compose(106, 50).expect("popup under the title");
    state.compose(60, 20).expect("no panic");
    let placed = popup_rect(&state);
    assert!(placed.bottom() <= 20 && placed.right() <= 60, "{placed:?}");
    assert_eq!(
        placed,
        centred(60, 20),
        "centred with no title to hang from"
    );

    // The title command printed nothing this round, and the window got shorter.
    let mut state = title_state(106, 50);
    click_title(&mut state);
    state.set_pane_surface(surface_with_popup());
    state.compose(106, 50).expect("popup under the title");
    state.snapshot.as_mut().expect("snapshot").agent_panel_title = None;
    state.compose(106, 20).expect("no panic");
    assert_eq!(popup_rect(&state), centred(106, 20));
}

#[test]
fn a_click_whose_popup_never_shows_leaves_no_anchor_for_the_next_popup() {
    // Its command failed; then a key command's popup opens.
    let mut state = title_state(106, 30);
    let request = click_title(&mut state);
    failed(&mut state, &request);
    open_key_popup(&mut state);
    state.set_pane_surface(other_popup());
    state.compose(106, 30).expect("popup");
    assert_eq!(popup_rect(&state), centred(106, 30));
    assert!(!state.title_popup_open(), "and the title isn't lit");

    // Its popup never showed before the wait's deadline.
    let mut state = title_state(106, 30);
    let request = click_title(&mut state);
    state.handle_endpoint_result(
        "boot-1",
        &request,
        Ok(crate::api::schema::ResponseResult::Ok {}),
    );
    let mut plain = surface();
    plain.surface_revision = 2;
    state.set_pane_surface(plain);
    let deadline = state.popup_pending_deadline.expect("deadline");
    state.tick_popup_pending(deadline);
    open_key_popup(&mut state);
    state.set_pane_surface(other_popup());
    state.compose(106, 30).expect("popup");
    assert_eq!(popup_rect(&state), centred(106, 30));

    // Another window's popup showed while the click waited, and the click then failed.
    let mut state = title_state(106, 30);
    let request = click_title(&mut state);
    state.set_pane_surface(other_popup());
    failed(&mut state, &request);
    state.compose(106, 30).expect("popup");
    assert_eq!(popup_rect(&state), centred(106, 30));
    assert!(!state.title_popup_open());
}

/// A window whose spaces header shows the system usage, with a popup when `command_id` is given.
fn system_usage_state(command_id: Option<&str>) -> ClientShellState {
    let mut projected = snapshot();
    projected.agent_panel_title = Some("7d 42%".into());
    projected.agent_panel_title_command_id = Some("cmd_title".into());
    projected.system_usage = Some(crate::protocol::ClientShellSystemUsage {
        cpu_percent: Some(78),
        memory_used_bytes: 13_300 << 20,
        memory_total_bytes: 16 << 30,
        memory_pressure: crate::protocol::ClientShellMemoryPressure::Warning,
    });
    projected.system_usage_command_id = command_id.map(str::to_owned);
    let mut config = Config::default();
    config.ui.sidebar_max_width = 60;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.sidebar_width = 40;
    state.sidebar_width_manual = true;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("frame");
    state
}

fn click_at(state: &mut ClientShellState, rect: Rect) -> Vec<ClientShellAction> {
    state
        .handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 2,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })])
        .actions
}

#[test]
fn the_spaces_header_shows_the_system_usage_coloured_by_load_and_pressure() {
    let mut state = system_usage_state(None);
    let frame = state.compose(106, 30).expect("frame");
    let rows = frame_rows(&frame);
    let header = rows[0].chars().take(40).collect::<String>();
    assert!(header.starts_with(" spaces"), "{header:?}");
    assert!(
        header.contains(" 78%  \u{f035b} 13.0G "),
        "CPU, then memory in GiB: {header:?}"
    );
    let palette = &state.config.palette;
    let colour_of = |needle: &str| {
        let (x, y) = cell_symbol_position(&frame, Rect::new(0, 0, 40, 1), needle);
        frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)].fg
    };
    assert_eq!(
        colour_of("78%"),
        crate::protocol::color_to_u32(palette.peach),
        "78% busy is orange"
    );
    assert_eq!(
        colour_of("13.0G"),
        crate::protocol::color_to_u32(palette.peach),
        "memory takes the pressure's colour"
    );
    assert!(
        state.hits.system_usage.is_empty(),
        "nothing to click without a popup"
    );
    assert!(click_at(&mut state, Rect::new(30, 0, 1, 1)).is_empty());
}

#[test]
fn a_narrow_sidebar_shows_the_system_usage_without_its_icons() {
    let mut state = system_usage_state(None);
    let header = |state: &mut ClientShellState, width: u16| {
        state.sidebar_width = width;
        let frame = state.compose(106, 30).expect("frame");
        frame_rows(&frame)[0]
            .chars()
            .take(usize::from(width))
            .collect::<String>()
    };
    let narrow = header(&mut state, 24);
    assert!(narrow.contains(" 78%  13.0G "), "{narrow:?}");
    assert!(!narrow.contains('\u{f035b}'), "{narrow:?}");
    let too_narrow = header(&mut state, 20);
    assert!(!too_narrow.contains('%'), "left out: {too_narrow:?}");
}

#[test]
fn a_click_on_the_system_usage_opens_its_popup_and_another_closes_it() {
    let mut state = system_usage_state(Some("cmd_system"));
    let pill = state.hits.system_usage;
    // The sidebar's 40 columns end in its divider; the pill ends a column before that, where the
    // agents panel's title pill ends without a sort label.
    assert_eq!((pill.y, pill.right()), (0, 38), "a column from the edge");
    let actions = click_at(&mut state, pill);
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected the popup's command, got {actions:?}");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command invocation");
    };
    assert_eq!(params.command_id, "cmd_system");
    state.set_pane_surface(surface_with_popup());
    state.compose(106, 30).expect("popup");
    let placed = popup_rect(&state);
    assert_eq!(
        (placed.x, placed.y),
        (pill.x, pill.bottom()),
        "hangs from it"
    );
    assert!(state.title_popup_open());

    // A click on the other title leaves it be; one on its own closes it.
    let title = state.hits.agent_panel_title;
    assert!(click_at(&mut state, title).is_empty());
    let actions = click_at(&mut state, pill);
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected the popup to close, got {actions:?}");
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::PopupClose(_)
    ));
}

#[test]
fn a_second_click_on_the_agents_panel_title_closes_its_popup() {
    let mut state = title_state(106, 30);
    click_title(&mut state);
    state.set_pane_surface(surface_with_popup());
    state.compose(106, 30).expect("popup");
    let title = state.hits.agent_panel_title;
    let actions = click_at(&mut state, title);
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected the popup to close, got {actions:?}");
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::PopupClose(_)
    ));

    // Another popup (a key command's) isn't the title's to close.
    let mut state = title_state(106, 30);
    open_key_popup(&mut state);
    state.set_pane_surface(other_popup());
    state.compose(106, 30).expect("popup");
    let title = state.hits.agent_panel_title;
    assert!(click_at(&mut state, title).is_empty());
}
