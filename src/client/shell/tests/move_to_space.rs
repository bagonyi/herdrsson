use super::*;

/// Spaces `Web/one` (`ws_1`, focused, with `tab_1`), `plain` and `Web/two` in this server order:
/// the sidebar draws `Web/two` before `plain`.
fn three_spaces() -> ClientShellState {
    let mut projected = snapshot();
    projected.workspaces = ["Web/one", "plain", "Web/two"]
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let mut workspace = snapshot().workspaces[0].clone();
            workspace.workspace_id = format!("ws_{}", index + 1);
            workspace.number = index + 1;
            workspace.label = (*label).into();
            workspace.custom_label = true;
            workspace.branch = None;
            workspace.focused = index == 0;
            workspace
        })
        .collect();
    state_with(projected)
}

fn state_with(projected: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

fn mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    point: (u16, u16),
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind,
        column: point.0,
        row: point.1,
        modifiers: KeyModifiers::empty(),
    })])
}

fn open_menu(state: &ClientShellState) -> (&ClientContextMenuTarget, Vec<String>) {
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("a context menu is open");
    };
    let labels = menu
        .items()
        .into_iter()
        .map(|item| item.label.into_owned())
        .collect();
    (&menu.target, labels)
}

/// Right-clicks `tab_1` in the tab bar of a new frame; the labels of the menu that opens.
fn open_tab_menu(state: &mut ClientShellState) -> Vec<String> {
    state.compose(106, 30).expect("composed frame");
    let tab = state.hits.tabs[0].0;
    let outcome = mouse(
        state,
        MouseEventKind::Down(MouseButton::Right),
        (tab.x + 1, tab.y),
    );
    assert!(outcome.actions.is_empty());
    let (target, labels) = open_menu(state);
    assert!(matches!(
        target,
        ClientContextMenuTarget::Tab { tab_id, .. } if tab_id == "tab_1"
    ));
    labels
}

/// Clicks the open menu's item `label` where a new frame draws it.
fn choose(state: &mut ClientShellState, label: &str) -> ClientShellInput {
    state.compose(106, 30).expect("menu frame");
    let index = open_menu(state)
        .1
        .iter()
        .position(|item| item == label)
        .expect("menu item");
    let (row, _) = state
        .hits
        .context_menu_rows
        .iter()
        .find(|(_, row)| *row == index)
        .copied()
        .expect("menu row hit");
    mouse(
        state,
        MouseEventKind::Down(MouseButton::Left),
        (row.x + 1, row.y),
    )
}

#[test]
fn tab_menu_offers_move_to_space_after_rename() {
    let mut state = three_spaces();

    assert_eq!(
        open_tab_menu(&mut state),
        ["New tab", "Rename", "Move to space", "Close"]
    );
}

#[test]
fn tab_menu_leaves_move_to_space_out_without_another_space() {
    let mut state = state_with(snapshot());

    assert_eq!(open_tab_menu(&mut state), ["New tab", "Rename", "Close"]);
}

#[test]
fn move_to_space_lists_the_other_spaces_in_sidebar_order() {
    let mut state = three_spaces();
    open_tab_menu(&mut state);

    let opened = choose(&mut state, "Move to space");

    // Opening the list sends nothing, not even the tab's focus.
    assert!(opened.actions.is_empty());
    let (target, labels) = open_menu(&state);
    assert!(matches!(
        target,
        ClientContextMenuTarget::MoveToSpace { tab_id, .. } if tab_id == "tab_1"
    ));
    assert_eq!(labels, ["Web/two", "plain"]);
}

#[test]
fn choosing_a_space_moves_the_tab_there_and_follows_it() {
    let mut state = three_spaces();
    open_tab_menu(&mut state);
    choose(&mut state, "Move to space");

    let outcome = choose(&mut state, "plain");

    assert!(state.overlay.is_none());
    let methods = outcome
        .actions
        .iter()
        .map(|action| {
            let ClientShellAction::Endpoint { request, .. } = action else {
                panic!("not an endpoint request: {action:?}");
            };
            request.method.clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        [crate::api::schema::Method::TabMoveToWorkspace(
            crate::api::schema::TabMoveToWorkspaceParams {
                tab_id: "tab_1".into(),
                workspace_id: "ws_2".into(),
                focus: true,
            }
        )]
    );
}

#[test]
fn a_server_without_tab_moves_explains_instead_of_moving() {
    let mut state = three_spaces();
    state.set_endpoint_methods(Some(vec!["tab.move".into(), "tab.focus".into()]));
    open_tab_menu(&mut state);
    choose(&mut state, "Move to space");

    let outcome = choose(&mut state, "Web/two");

    assert!(outcome.actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("unsupported action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Unsupported);
    assert_eq!(notice.key.code, "tab.move_to_workspace");
}
