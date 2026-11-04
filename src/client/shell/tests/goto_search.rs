use super::*;

fn open_goto() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::OpenNavigator),
        &mut open,
    );
    assert!(open.repaint);
    state
}

fn navigator(state: &ClientShellState) -> Option<&ClientNavigatorOverlay> {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::Navigator(navigator)) => Some(navigator),
        _ => None,
    }
}

#[test]
fn goto_key_opens_with_search_focused_and_typing_searches() {
    let mut state = open_goto();
    assert!(navigator(&state).is_some_and(|navigator| navigator.search_focused));
    let frame = state.compose(106, 30).expect("goto frame");
    assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));

    // `j` and `b` are list keys outside search; here they are text.
    assert!(state.handle_input_bytes(b"jb").actions.is_empty());
    let navigator = navigator(&state).expect("goto stays open");
    assert!(navigator.search_focused);
    assert_eq!(navigator.query.as_str(), "jb");
    assert_eq!(navigator.filter, None);
}

#[test]
fn goto_esc_closes_at_once_typed_or_not() {
    let escape = || {
        RawInputEvent::Key(crate::input::TerminalKey::new(
            KeyCode::Esc,
            KeyModifiers::empty(),
        ))
    };
    let mut state = open_goto();
    let close = state.handle_raw_events(vec![escape()]);
    assert!(close.repaint);
    assert!(state.overlay.is_none());

    let mut state = open_goto();
    state.handle_input_bytes(b"client");
    state.handle_raw_events(vec![escape()]);
    assert!(state.overlay.is_none());

    // From the list too, as before.
    let mut state = open_goto();
    if let Some(ClientShellOverlay::Navigator(navigator)) = state.overlay.as_mut() {
        navigator.search_focused = false;
    }
    state.handle_raw_events(vec![escape()]);
    assert!(state.overlay.is_none());
}
