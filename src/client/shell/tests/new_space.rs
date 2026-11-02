//! A new space goes next to the space it was made from, and in a group the name box starts with
//! `<group>/` (`space_groups::new_space`).

use super::*;
use crate::api::schema::{Method, ResponseResult};
use crate::client::shell::space_groups::new_space::new_space_label;

fn prompting_state(projected: ClientShellSnapshot) -> ClientShellState {
    let mut config = Config::default();
    config.ui.prompt_new_workspace_name = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

/// Presses the new space key: the box opens when the window asks for names, else the create goes.
fn new_space_key(state: &mut ClientShellState) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
        &mut outcome,
    );
    outcome
}

/// The new space box's text and whether typing replaces it.
fn new_space_box(state: &ClientShellState) -> (String, bool) {
    let Some(ClientShellOverlay::Rename(ClientRenameOverlay {
        input,
        target: ClientRenameTarget::NewWorkspace { .. },
        ..
    })) = state.overlay.as_ref()
    else {
        panic!("the new space box is not open: {:?}", state.overlay);
    };
    let mut typed = input.clone();
    typed.insert("!");
    (input.as_str().to_owned(), typed.as_str() == "!")
}

/// The one `workspace.create` in `outcome`: its request id and name.
fn create_request(outcome: &ClientShellInput) -> (String, Option<String>) {
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("not one request: {:?}", outcome.actions);
    };
    let Method::WorkspaceCreate(params) = &request.method else {
        panic!("not a space create: {:?}", request.method);
    };
    (request.id.clone(), params.label.clone())
}

/// The server's answer to `request_id`: space `workspace_id` was made. What the window sends then.
fn answer_created(
    state: &mut ClientShellState,
    request_id: &str,
    workspace_id: &str,
) -> Vec<ClientShellAction> {
    let root_pane = serde_json::from_value(serde_json::json!({
        "pane_id": format!("{workspace_id}:p1"),
        "terminal_id": "term_new",
        "workspace_id": workspace_id,
        "tab_id": format!("{workspace_id}:t1"),
        "focused": true,
        "agent_status": "idle",
        "revision": 0,
    }))
    .expect("pane info");
    let created = ResponseResult::WorkspaceCreated {
        workspace: crate::api::schema::WorkspaceInfo {
            workspace_id: workspace_id.into(),
            number: 0,
            label: "repo".into(),
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: format!("{workspace_id}:t1"),
            agent_status: AgentStatus::Idle,
            tokens: Default::default(),
            worktree: None,
        },
        tab: crate::api::schema::TabInfo {
            tab_id: format!("{workspace_id}:t1"),
            workspace_id: workspace_id.into(),
            number: 1,
            label: "1".into(),
            focused: true,
            pane_count: 1,
            agent_status: AgentStatus::Idle,
        },
        root_pane,
    };
    let (_, actions) = state.handle_endpoint_result("boot-1", request_id, Ok(created));
    actions
}

/// The server's next snapshot with the new space `label` (None: unnamed) added at the end, as the
/// server adds it, then the moves sent.
fn add_and_apply(state: &mut ClientShellState, label: Option<&str>, moves: &[ClientShellAction]) {
    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.revision += 1;
    let number = projected.workspaces.len() + 1;
    projected
        .workspaces
        .push(space(number, label.unwrap_or("repo"), label.is_some()));
    state.set_snapshot(Box::new(projected));
    apply_sent(state, &sent_methods(moves));
}

/// Makes a space from `focused` through the name box with `text`; the name it gets, what the answer
/// sends and the drawn order once the server has done both.
fn make_space(
    projected: ClientShellSnapshot,
    focused: &str,
    text: &str,
) -> (Option<String>, Vec<String>, Vec<String>) {
    let mut state = prompting_state(focused_on(projected, focused));
    assert!(new_space_key(&mut state).actions.is_empty());
    let (request_id, label) = create_request(&save_rename_box(&mut state, text));
    let new_id = format!(
        "ws_{}",
        state
            .snapshot
            .as_deref()
            .expect("snapshot")
            .workspaces
            .len()
            + 1
    );
    let moves = answer_created(&mut state, &request_id, &new_id);
    let sent = sent(&moves);
    add_and_apply(&mut state, label.as_deref(), &moves);
    let (drawn, _) = drawn_and_server_order(&state);
    (label, sent, drawn)
}

#[test]
fn in_a_group_the_name_box_starts_with_the_groups_name() {
    let mut state = prompting_state(named_spaces(&["Code/herdr", "Code/web", "x"]));
    new_space_key(&mut state);
    assert_eq!(new_space_box(&state), ("Code/".to_owned(), false));

    // A linked worktree is drawn in its parent's group.
    let mut projected = named_spaces(&["Code/repo", "x", "child"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 3, true);
    let mut state = prompting_state(focused_on(projected, "ws_3"));
    new_space_key(&mut state);
    assert_eq!(new_space_box(&state).0, "Code/");

    // Outside a group: upstream's folder name, replaced on typing.
    let mut state = prompting_state(focused_on(named_spaces(&["Code/herdr", "x"]), "ws_2"));
    new_space_key(&mut state);
    assert_eq!(new_space_box(&state), ("repo".to_owned(), true));
    let mut state = prompting_state(named_spaces(&["a/", "x"]));
    new_space_key(&mut state);
    assert_eq!(new_space_box(&state).0, "repo");
}

#[test]
fn a_new_space_in_the_same_group_goes_right_after_the_space_it_was_made_from() {
    let (label, sent, drawn) = make_space(
        named_spaces(&["Code/herdr", "Code/web", "x"]),
        "ws_1",
        "Code/test",
    );
    assert_eq!(label.as_deref(), Some("Code/test"));
    assert_eq!(sent, ["move ws_4 before ws_2"]);
    assert_eq!(drawn, ["ws_1", "ws_4", "ws_2", "ws_3"]);

    // Typed after the prefilled `Code/`.
    let mut state = prompting_state(named_spaces(&["Code/herdr", "Code/web"]));
    new_space_key(&mut state);
    let save = state.handle_input_bytes(b"test\r");
    assert_eq!(create_request(&save).1.as_deref(), Some("Code/test"));

    // The group's last space: right after it, where the server adds it anyway.
    let (_, sent, drawn) = make_space(
        named_spaces(&["Code/herdr", "Code/web"]),
        "ws_2",
        "Code/test",
    );
    assert!(sent.is_empty());
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_3"]);

    // A scattered group: still drawn right after it.
    let (_, sent, drawn) = make_space(named_spaces(&["G/a", "x", "G/b"]), "ws_1", "G/new");
    assert_eq!(sent, ["move ws_4 before ws_2"]);
    assert_eq!(drawn, ["ws_1", "ws_4", "ws_3", "ws_2"]);
}

#[test]
fn saving_the_bare_group_name_names_the_space_after_its_folder_in_the_group() {
    let mut state = prompting_state(named_spaces(&["Code/herdr", "Code/web"]));
    new_space_key(&mut state);
    let save = state.handle_input_bytes(b"\r");
    assert_eq!(create_request(&save).1.as_deref(), Some("Code/repo"));

    assert_eq!(
        new_space_label("Code/", "repo").as_deref(),
        Some("Code/repo")
    );
    assert_eq!(
        new_space_label("Code /", "repo").as_deref(),
        Some("Code/repo")
    );
    assert_eq!(new_space_label("Code/x", "repo").as_deref(), Some("Code/x"));
    assert_eq!(new_space_label("a/b/", "repo").as_deref(), Some("a/b/"));
    // Upstream's rule otherwise.
    assert_eq!(new_space_label("", "repo"), None);
    assert_eq!(new_space_label("repo", "repo"), None);
    assert_eq!(new_space_label("other", "repo").as_deref(), Some("other"));
    assert_eq!(new_space_label("/", "repo").as_deref(), Some("/"));
}

#[test]
fn a_new_space_in_another_group_goes_after_that_groups_last_space() {
    let (_, sent, drawn) = make_space(named_spaces(&["A/a", "x", "B/b", "y"]), "ws_1", "B/new");
    assert_eq!(sent, ["move ws_5 before ws_4"]);
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_3", "ws_5", "ws_4"]);
}

#[test]
fn a_new_space_leaving_the_group_goes_right_after_it() {
    // The top level.
    let (_, sent, drawn) = make_space(named_spaces(&["A/a", "A/b", "x"]), "ws_1", "plain");
    assert_eq!(sent, ["move ws_4 before ws_3"]);
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_4", "ws_3"]);

    // A new group.
    let (_, sent, drawn) = make_space(named_spaces(&["A/a", "A/b", "x"]), "ws_2", "New/n");
    assert_eq!(sent, ["move ws_4 before ws_3"]);
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_4", "ws_3"]);

    // An emptied box: unnamed, so at the top level.
    let (label, sent, drawn) = make_space(named_spaces(&["A/a", "A/b", "x"]), "ws_1", "");
    assert_eq!(label, None);
    assert_eq!(sent, ["move ws_4 before ws_3"]);
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_4", "ws_3"]);
}

#[test]
fn a_new_top_level_space_goes_right_after_the_space_and_its_worktrees() {
    let (_, sent, drawn) = make_space(named_spaces(&["x", "A/a", "y"]), "ws_1", "plain");
    assert_eq!(sent, ["move ws_4 before ws_2"]);
    assert_eq!(drawn, ["ws_1", "ws_4", "ws_2", "ws_3"]);

    // The parent of a linked worktree drawn under it from further down the server order.
    let mut projected = named_spaces(&["p", "x", "child"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 3, true);
    let (_, sent, drawn) = make_space(projected, "ws_1", "plain");
    assert_eq!(sent, ["move ws_4 before ws_2"]);
    assert_eq!(drawn, ["ws_1", "ws_3", "ws_4", "ws_2"]);

    // The last space: the server's end, nothing to move.
    let (_, sent, drawn) = make_space(named_spaces(&["x", "y"]), "ws_2", "plain");
    assert!(sent.is_empty());
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_3"]);
}

#[test]
fn without_the_name_box_a_new_space_still_goes_next_to_its_space() {
    let mut state = grouped_state(named_spaces(&["x", "A/a", "A/b", "y"]));
    let (request_id, label) = create_request(&new_space_key(&mut state));
    assert_eq!(label, None);
    assert_eq!(
        sent(&answer_created(&mut state, &request_id, "ws_5")),
        ["move ws_5 before ws_2"]
    );

    // From a group, unnamed: right after the group.
    let mut state = grouped_state(focused_on(named_spaces(&["x", "A/a", "A/b", "y"]), "ws_2"));
    let (request_id, _) = create_request(&new_space_key(&mut state));
    assert_eq!(
        sent(&answer_created(&mut state, &request_id, "ws_5")),
        ["move ws_5 before ws_4"]
    );
}

#[test]
fn a_new_space_moves_nowhere_when_its_place_has_gone_or_the_create_failed() {
    let mut state = grouped_state(named_spaces(&["x", "y"]));
    let (request_id, _) = create_request(&new_space_key(&mut state));
    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.workspaces.remove(1);
    state.set_snapshot(Box::new(projected));
    assert!(answer_created(&mut state, &request_id, "ws_3").is_empty());

    let mut state = grouped_state(named_spaces(&["x", "y"]));
    let (request_id, _) = create_request(&new_space_key(&mut state));
    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("workspace_create_failed".into()),
            message: "no".into(),
        }),
    );
    assert!(repaint);
    assert!(actions.is_empty());
}

/// The server's next snapshot: `change` applied to the current one.
fn next_snapshot(state: &mut ClientShellState, change: impl FnOnce(&mut ClientShellSnapshot)) {
    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.revision += 1;
    change(&mut projected);
    let mut projected_surface = surface();
    projected_surface.projection_revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(projected_surface);
}

/// The server adds space 21 at the end and focuses it.
fn created_at_the_end(projected: &mut ClientShellSnapshot) {
    projected.workspaces.push(space(21, "s1", false));
    *projected = focused_on(projected.clone(), "ws_21");
}

#[test]
fn a_new_space_moved_up_a_long_list_is_revealed_where_it_lands() {
    // The answer comes after the snapshot with the new space (the sidebar scrolls down to it
    // first), or before it.
    for answer_first in [false, true] {
        let labels = (1..=20).map(|n| format!("s{n}")).collect::<Vec<_>>();
        let labels = labels.iter().map(String::as_str).collect::<Vec<_>>();
        let mut state = grouped_state(named_spaces(&labels));
        state.compose(106, 20).expect("frame");
        assert!(!has_hit(&state, "ws_20"), "the list overflows the sidebar");
        let (request_id, _) = create_request(&new_space_key(&mut state));
        let moves = if answer_first {
            let moves = answer_created(&mut state, &request_id, "ws_21");
            next_snapshot(&mut state, created_at_the_end);
            moves
        } else {
            next_snapshot(&mut state, created_at_the_end);
            answer_created(&mut state, &request_id, "ws_21")
        };
        state.compose(106, 20).expect("frame");
        assert!(has_hit(&state, "ws_21"), "revealed at the end first");
        assert!(!has_hit(&state, "ws_2"));
        assert_eq!(sent(&moves), ["move ws_21 before ws_2"]);

        // A frame between the answer and the move's snapshot doesn't use the reveal up.
        state.compose(106, 20).expect("frame");
        apply_sent(&mut state, &sent_methods(&moves));
        state.compose(106, 20).expect("frame");
        let (drawn, _) = drawn_and_server_order(&state);
        assert_eq!(&drawn[..3], ["ws_1", "ws_21", "ws_2"]);
        assert!(has_hit(&state, "ws_21"), "answer first: {answer_first}");
        assert!(state.placed_new_space.is_none());
    }
}

#[test]
fn a_new_space_whose_place_went_is_not_waited_for() {
    let mut state = grouped_state(named_spaces(&["x", "y", "z"]));
    let (request_id, _) = create_request(&new_space_key(&mut state));
    next_snapshot(&mut state, |projected| {
        projected.workspaces.push(space(4, "new", false));
    });
    assert_eq!(
        sent(&answer_created(&mut state, &request_id, "ws_4")),
        ["move ws_4 before ws_2"]
    );
    assert!(state.placed_new_space.is_some());
    next_snapshot(&mut state, |projected| {
        projected
            .workspaces
            .retain(|workspace| workspace.workspace_id != "ws_2");
    });
    assert!(state.placed_new_space.is_none());
}
