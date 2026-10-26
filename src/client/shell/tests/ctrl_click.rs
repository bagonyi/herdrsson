use super::*;

fn state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");
    state
}

fn ctrl_mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    point: (u16, u16),
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind,
        column: point.0,
        row: point.1,
        modifiers: KeyModifiers::CONTROL,
    })])
}

fn menu_target(state: &ClientShellState) -> &ClientContextMenuTarget {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => &menu.target,
        _ => panic!("a context menu is open"),
    }
}

#[test]
fn ctrl_clicking_a_space_opens_its_menu_and_ctrl_clicking_an_item_chooses_it() {
    let mut state = state();
    let workspace = state.hits.workspaces[0].rect;
    let point = (workspace.x + 2, workspace.y);
    let press = ctrl_mouse(&mut state, MouseEventKind::Down(MouseButton::Left), point);
    assert!(press.actions.is_empty());
    assert!(matches!(
        menu_target(&state),
        ClientContextMenuTarget::Workspace { workspace_id, .. } if workspace_id == "ws_1"
    ));

    // The press's drag onto the menu and release there choose nothing.
    state.compose(106, 20).expect("workspace context menu");
    let rename = state.hits.context_menu_rows[0].0;
    let on_rename = (rename.x + 1, rename.y);
    ctrl_mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        on_rename,
    );
    let release = ctrl_mouse(&mut state, MouseEventKind::Up(MouseButton::Left), on_rename);
    assert!(release.actions.is_empty());
    assert!(matches!(
        menu_target(&state),
        ClientContextMenuTarget::Workspace { .. }
    ));

    // In the open menu a ctrl+click is a click.
    ctrl_mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        on_rename,
    );
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::Workspace { ref workspace_id },
            ..
        })) if workspace_id == "ws_1"
    ));
}

#[test]
fn ctrl_clicking_a_tab_opens_its_menu() {
    let mut state = state();
    let tab = state.hits.tabs[0].0;
    ctrl_mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        (tab.x + 1, tab.y),
    );
    assert!(matches!(
        menu_target(&state),
        ClientContextMenuTarget::Tab { tab_id, .. } if tab_id == "tab_1"
    ));
}

#[test]
fn ctrl_clicking_pane_content_still_looks_for_a_link() {
    let mut state = state();
    let inner = state.hits.panes[0].inner_rect;
    let press = ctrl_mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        (inner.x + 1, inner.y + 1),
    );
    assert!(state.overlay.is_none());
    assert!(press.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(request.method, crate::api::schema::Method::PaneLinkActivate(_))
    )));
}
