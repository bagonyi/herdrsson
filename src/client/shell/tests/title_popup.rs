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
