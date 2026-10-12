use super::*;
use crate::client::shell::space_groups::drag::{GroupDragSource, GroupDropTarget};
use crate::client::shell::space_groups::split_name;

fn space(number: usize, label: &str, custom_label: bool) -> ClientShellWorkspace {
    let mut workspace = snapshot().workspaces[0].clone();
    workspace.workspace_id = format!("ws_{number}");
    workspace.number = number;
    workspace.label = label.into();
    workspace.custom_label = custom_label;
    workspace.branch = None;
    workspace.focused = number == 1;
    workspace
}

/// Spaces with user-set names in this server order; `ws_1` is focused.
fn named_spaces(labels: &[&str]) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces = labels
        .iter()
        .enumerate()
        .map(|(index, label)| space(index + 1, label, true))
        .collect();
    projected
}

fn grouped_state(projected: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

/// Puts `ws_<number>` in the one repository's worktree group: linked or a parent checkout.
fn in_worktree(projected: &mut ClientShellSnapshot, number: usize, linked: bool) {
    projected.workspaces[number - 1].worktree = Some(ClientShellWorktree {
        key: "/repo/.git".into(),
        label: "repo".into(),
        is_linked_worktree: linked,
    });
}

fn focused_on(mut projected: ClientShellSnapshot, workspace_id: &str) -> ClientShellSnapshot {
    projected.focused_workspace_id = Some(workspace_id.into());
    for workspace in &mut projected.workspaces {
        workspace.focused = workspace.workspace_id == workspace_id;
    }
    projected
}

/// Ten groups `G1`..`G10` with one member `space<n>` (`ws_<n>`) each: 20 sidebar rows.
fn ten_groups() -> ClientShellSnapshot {
    let labels = (1..=10)
        .map(|number| format!("G{number}/space{number}"))
        .collect::<Vec<_>>();
    named_spaces(&labels.iter().map(String::as_str).collect::<Vec<_>>())
}

/// The text of the sidebar's space list, one string per row, without the scrollbar and trailing
/// blanks.
fn body_lines(state: &mut ClientShellState, rows: usize) -> Vec<String> {
    body_lines_at(state, 30, rows)
}

fn body_lines_at(state: &mut ClientShellState, height: u16, rows: usize) -> Vec<String> {
    let frame = state.compose(106, height).expect("composed frame");
    let body = state.hits.workspace_body;
    frame_rows(&frame)
        .iter()
        .skip(usize::from(body.y))
        .take(rows)
        .map(|row| {
            row.chars()
                .skip(usize::from(body.x))
                .take(usize::from(body.width))
                .collect::<String>()
                .trim_end()
                .trim_end_matches('▕')
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn icon(state: &ClientShellState) -> &'static str {
    status_icon(AgentStatus::Idle, state.config.status_indicators)
}

fn set_focus(state: &mut ClientShellState, workspace_id: &str) {
    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.revision += 1;
    projected.focused_workspace_id = Some(workspace_id.into());
    for workspace in &mut projected.workspaces {
        workspace.focused = workspace.workspace_id == workspace_id;
    }
    let mut projected_surface = surface();
    projected_surface.projection_revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(projected_surface);
}

fn focus_request(state: &mut ClientShellState, action: crate::input::KeybindAction) -> String {
    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("{action:?} should focus a space");
    };
    let crate::api::schema::Method::WorkspaceFocus(target) = &request.method else {
        panic!("{action:?} should focus a space");
    };
    target.workspace_id.clone()
}

#[test]
fn group_names_split_user_set_names_at_the_first_slash() {
    for (label, custom_label, expected) in [
        ("Web/Miniteam", true, Some(("Web", "Miniteam"))),
        ("Web / Miniteam", true, Some(("Web", "Miniteam"))),
        ("Web/app/x", true, Some(("Web", "app/x"))),
        ("web/one", true, Some(("web", "one"))),
        ("Web/", true, None),
        ("/x", true, None),
        ("/", true, None),
        (" / ", true, None),
        ("plain", true, None),
        ("Web/Miniteam", false, None),
        ("/", false, None),
    ] {
        assert_eq!(
            split_name(&space(1, label, custom_label)),
            expected,
            "{label:?} custom={custom_label}"
        );
    }
}

#[test]
fn group_members_are_drawn_together_under_a_header_with_short_names() {
    let mut projected = named_spaces(&["A/one", "plain", "B/three", "A/two", "x/"]);
    projected.workspaces.push(space(6, "Auto/dir", false));
    let mut state = grouped_state(projected);
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 9),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
            " ▾ B".to_owned(),
            format!("   {icon} three"),
            format!(" {icon} x/"),
            format!(" {icon} Auto/dir"),
            String::new(),
        ]
    );
    let hits = state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.workspace_id.as_str(), hit.rect.y))
        .collect::<Vec<_>>();
    let body = state.hits.workspace_body.y;
    assert_eq!(
        hits,
        [
            ("ws_1", body + 1),
            ("ws_4", body + 2),
            ("ws_2", body + 3),
            ("ws_3", body + 5),
            ("ws_5", body + 6),
            ("ws_6", body + 7),
        ],
        "headers get no space hit"
    );
    assert!(state
        .hits
        .workspaces
        .iter()
        .all(|hit| hit.rect.x == state.hits.workspace_body.x));
}

#[test]
fn group_names_are_case_sensitive_and_shown_as_typed() {
    let mut state = grouped_state(named_spaces(&["Web/a", "web/b", "Web / c"]));
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 5),
        [
            " ▾ Web".to_owned(),
            format!("   {icon} a"),
            format!("   {icon} c"),
            " ▾ web".to_owned(),
            format!("   {icon} b"),
        ]
    );
}

#[test]
fn group_member_highlights_span_the_whole_row_and_never_the_header() {
    use ratatui::style::Color;

    fn row_backgrounds(
        state: &ClientShellState,
        buffer: &Buffer,
        workspace_id: &str,
    ) -> Vec<Color> {
        let rect = state
            .hits
            .workspaces
            .iter()
            .find(|hit| hit.workspace_id == workspace_id)
            .expect("visible space")
            .rect;
        (rect.x..rect.right())
            .map(|x| buffer[(x, rect.y)].bg)
            .collect()
    }

    for selection_bg in [Color::Reset, Color::Rgb(70, 63, 93)] {
        let mut state = grouped_state(named_spaces(&["A/one", "A/two", "plain"]));
        state.config.palette = Palette::terminal();
        state.config.palette.selection_bg = selection_bg;
        let palette = state.config.palette.clone();
        let buffer = state
            .compose(106, 30)
            .expect("composed frame")
            .to_ratatui_buffer()
            .expect("buffer");
        let focused = row_backgrounds(&state, &buffer, "ws_1");
        assert!(focused.iter().all(|bg| *bg == palette.active_row_bg));
        let body = state.hits.workspace_body;
        let header = Rect::new(body.x, body.y, focused.len() as u16, 1);
        assert!((header.x..header.right()).all(|x| buffer[(x, header.y)].bg == palette.sidebar_bg));
        let name = &buffer[(header.x + 3, header.y)];
        assert_eq!(name.symbol(), "A");
        assert_eq!(name.fg, palette.text);
        assert!(name.modifier.contains(Modifier::BOLD));

        state.handle_input_bytes(&[0x02]);
        state.handle_input_bytes(b"w");
        state.handle_input_bytes(b"\x1b[B");
        let buffer = state
            .compose(106, 30)
            .expect("composed frame")
            .to_ratatui_buffer()
            .expect("buffer");
        let (selected, focused) = if selection_bg == Color::Reset {
            (palette.active_row_bg, palette.sidebar_bg)
        } else {
            (selection_bg, palette.active_row_bg)
        };
        let selected_row = row_backgrounds(&state, &buffer, "ws_2");
        assert!(
            selected_row.iter().all(|bg| *bg == selected),
            "{selection_bg:?}: {selected_row:?}"
        );
        let focused_row = row_backgrounds(&state, &buffer, "ws_1");
        assert!(
            focused_row.iter().all(|bg| *bg == focused),
            "{selection_bg:?}: {focused_row:?}"
        );
    }
}

#[test]
fn space_navigation_follows_the_grouped_order_and_skips_headers() {
    use crate::input::KeybindAction;

    let mut state = grouped_state(named_spaces(&["A/one", "plain", "B/three", "A/two"]));
    state.compose(106, 30).expect("composed frame");
    let drawn = ["ws_1", "ws_4", "ws_2", "ws_3"];

    for (position, workspace_id) in drawn.iter().enumerate() {
        set_focus(&mut state, workspace_id);
        assert_eq!(
            focus_request(&mut state, KeybindAction::NextWorkspace),
            drawn[(position + 1) % drawn.len()]
        );
        assert_eq!(
            focus_request(&mut state, KeybindAction::PreviousWorkspace),
            drawn[(position + drawn.len() - 1) % drawn.len()]
        );
    }
    for (index, workspace_id) in drawn.iter().enumerate() {
        assert_eq!(
            focus_request(&mut state, KeybindAction::SwitchWorkspace(index)),
            *workspace_id
        );
    }
    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(KeybindAction::SwitchWorkspace(drawn.len())),
        &mut outcome,
    );
    assert!(
        !outcome.actions.iter().any(|action| matches!(
            action,
            ClientShellAction::Endpoint { request, .. }
                if matches!(request.method, crate::api::schema::Method::WorkspaceFocus(_))
        )),
        "no fifth space to switch to"
    );

    set_focus(&mut state, "ws_1");
    state.compose(106, 30).expect("composed frame");
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
    for workspace_id in &drawn[1..] {
        state.handle_input_bytes(b"\x1b[B");
        assert_eq!(
            state.navigate_workspace_id,
            state.navigation_target(&ClientEndpointId::Local, workspace_id)
        );
    }
    let digit = state.handle_input_bytes(b"3");
    assert!(matches!(
        &digit.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target)
                    if target.workspace_id == "ws_2"
            )
    ));
}

#[test]
fn revealing_a_space_counts_the_header_rows_above_it() {
    let last = "ws_10";

    // Focus arriving from the server scrolls the focused space into view.
    let mut state = grouped_state(focused_on(ten_groups(), last));
    state.compose(106, 20).expect("composed frame");
    // 20 rows, 10 of them headers: in a body this short, a scroll counted in spaces (9) would
    // leave the last space (row 19) below the view.
    assert!(state.hits.workspace_body.height <= 10);
    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == last));

    // Switching by key scrolls before the server answers.
    let mut state = grouped_state(ten_groups());
    state.compose(106, 20).expect("composed frame");
    assert!(!state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == last));
    assert_eq!(
        focus_request(&mut state, crate::input::KeybindAction::SwitchWorkspace(9)),
        last
    );
    state.compose(106, 20).expect("composed frame");
    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == last));
}

#[test]
fn worktree_block_moves_and_indents_as_one_unit_inside_its_group() {
    let mut projected = named_spaces(&[
        "plain",
        "Web/app",
        "Web/x",
        "Other/y",
        "feature",
        "Web/site",
        "Other/dup",
    ]);
    // A second checkout of the repository is a second parent row of the same block.
    for (index, linked) in [(1, false), (2, true), (3, true), (4, true), (6, false)] {
        projected.workspaces[index].worktree = Some(ClientShellWorktree {
            key: "/repo/.git".into(),
            label: "repo".into(),
            is_linked_worktree: linked,
        });
    }
    projected.workspaces[4].custom_label = false;
    projected.workspaces[4].branch = Some("worktree/feature".into());
    let mut state = grouped_state(projected);
    let icon = icon(&state);
    let lines = body_lines(&mut state, 9);
    assert_eq!(lines[0], format!(" {icon} plain"));
    assert_eq!(lines[1], " ▾ Web");
    for (line, name) in [(&lines[2], "app"), (&lines[3], "Other/dup")] {
        assert!(
            line.starts_with(&format!("   {icon} {name} ")) && line.ends_with('▾'),
            "{lines:?}"
        );
    }
    assert_eq!(
        lines[4..],
        [
            format!("     ├─ {icon} x"),
            format!("     ├─ {icon} Other/y"),
            format!("     └─ {icon} feature"),
            format!("   {icon} site"),
            String::new(),
        ]
    );
    let parent = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("parent row");
    let (toggle, _) = parent.group_toggle.as_ref().expect("worktree toggle");
    assert_eq!(toggle.x, parent.rect.right() - 1);
}

#[test]
fn checkouts_of_one_repository_without_linked_worktrees_group_by_their_own_names() {
    let mut projected = named_spaces(&["A/one", "B/two"]);
    for workspace in &mut projected.workspaces {
        workspace.worktree = Some(ClientShellWorktree {
            key: "/repo/.git".into(),
            label: "repo".into(),
            is_linked_worktree: false,
        });
    }
    let mut state = grouped_state(projected);
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 4),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            " ▾ B".to_owned(),
            format!("   {icon} two"),
        ]
    );
}

#[test]
fn row_gap_goes_before_top_level_rows_only() {
    let mut state = grouped_state(named_spaces(&["A/one", "plain", "B/three", "A/two"]));
    state.config.spaces.row_gap = 1;
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 9),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} two"),
            String::new(),
            format!(" {icon} plain"),
            String::new(),
            " ▾ B".to_owned(),
            format!("   {icon} three"),
            String::new(),
        ]
    );
}

/// `line` without the worktree ▾/▸ toggle at its right edge.
fn without_toggle(line: &str) -> String {
    line.trim_end_matches(['▾', '▸']).trim_end().to_owned()
}

#[test]
fn collapsed_rail_lists_and_numbers_spaces_in_the_grouped_order() {
    let mut state = grouped_state(named_spaces(&["A/one", "plain", "A/two"]));
    state.sidebar_collapsed = true;
    let frame = state.compose(106, 30).expect("collapsed sidebar");
    let rows = frame_rows(&frame);
    let rail = state
        .hits
        .workspaces
        .iter()
        .map(|hit| {
            let number = rows[usize::from(hit.rect.y)]
                .chars()
                .skip(usize::from(hit.rect.x))
                .take(2)
                .collect::<String>();
            (hit.workspace_id.clone(), number.trim_end().to_owned())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rail,
        [("ws_1", "1"), ("ws_3", "2"), ("ws_2", "3")]
            .map(|(id, number)| (id.into(), number.into()))
    );
    assert_eq!(
        focus_request(&mut state, crate::input::KeybindAction::SwitchWorkspace(1)),
        "ws_3",
        "the number keys pick the rail's numbers"
    );

    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    for workspace_id in ["ws_3", "ws_2"] {
        state.handle_input_bytes(b"\x1b[B");
        assert_eq!(
            state.navigate_workspace_id,
            state.navigation_target(&ClientEndpointId::Local, workspace_id)
        );
    }
}

#[test]
fn collapsed_rail_keeps_server_order_without_groups() {
    // The expanded sidebar draws the linked worktree `feature` under `repo`, before `plain`.
    let mut projected = named_spaces(&["repo", "plain", "feature"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(projected);
    state.compose(106, 30).expect("expanded sidebar");
    let expanded = state
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.workspace_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(expanded, ["ws_1", "ws_3", "ws_2"]);

    state.sidebar_collapsed = true;
    state.compose(106, 30).expect("collapsed sidebar");
    let rail = state
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.workspace_id.clone())
        .collect::<Vec<_>>();
    assert_eq!(rail, ["ws_1", "ws_2", "ws_3"]);
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_2")
    );
}

#[test]
fn revealing_a_groups_first_member_also_reveals_its_header() {
    let mut state = grouped_state(focused_on(ten_groups(), "ws_10"));
    let icon = icon(&state);
    body_lines_at(&mut state, 20, 1);
    assert!(
        !state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == "ws_3"),
        "scrolled to the bottom"
    );

    // By key, before the server answers: the space is out of view, above.
    assert_eq!(
        focus_request(&mut state, crate::input::KeybindAction::SwitchWorkspace(2)),
        "ws_3"
    );
    assert_eq!(
        body_lines_at(&mut state, 20, 2),
        [" ▾ G3".to_owned(), format!("   {icon} space3")]
    );

    // Focus from the server when the space is the top row and its header just above the view.
    state.workspace_scroll = 7;
    assert_eq!(
        body_lines_at(&mut state, 20, 1),
        [format!("   {icon} space4")]
    );
    set_focus(&mut state, "ws_4");
    assert_eq!(
        body_lines_at(&mut state, 20, 2),
        [" ▾ G4".to_owned(), format!("   {icon} space4")]
    );
}

#[test]
fn long_group_names_end_in_an_ellipsis() {
    let name = "Customer-Projects-With-A-Very-Long-Group-Name";
    let mut state = grouped_state(named_spaces(&[&format!("{name}/x")]));
    let header = body_lines(&mut state, 1).remove(0);
    let width = usize::from(state.hits.workspace_body.width);
    assert!(header.starts_with(" ▾ Customer"), "{header:?}");
    assert!(header.ends_with('…'), "{header:?}");
    assert_eq!(header.chars().count(), width - 2, "{header:?}");
}

#[test]
fn grouping_moves_a_worktree_block_past_other_spaces_as_one_unit() {
    let mut projected = named_spaces(&["Web/a", "plain", "Web/p", "Other/c", "feature"]);
    in_worktree(&mut projected, 3, false);
    in_worktree(&mut projected, 4, true);
    in_worktree(&mut projected, 5, true);
    projected.workspaces[4].custom_label = false;
    projected.workspaces[4].branch = Some("worktree/feature".into());
    let mut state = grouped_state(projected);
    let icon = icon(&state);
    let lines = body_lines(&mut state, 7);
    assert!(lines[2].ends_with('▾'), "{lines:?}");
    assert_eq!(
        lines
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            " ▾ Web".to_owned(),
            format!("   {icon} a"),
            format!("   {icon} p"),
            format!("     ├─ {icon} Other/c"),
            format!("     └─ {icon} feature"),
            format!(" {icon} plain"),
            String::new(),
        ]
    );
}

#[test]
fn an_orphan_linked_worktree_groups_by_its_own_name() {
    let mut projected = named_spaces(&["plain", "Web/a", "x", "Web/orphan"]);
    in_worktree(&mut projected, 4, true);
    let mut state = grouped_state(projected);
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 6),
        [
            format!(" {icon} plain"),
            " ▾ Web".to_owned(),
            format!("   {icon} a"),
            format!("   {icon} orphan"),
            format!(" {icon} x"),
            String::new(),
        ]
    );
    let orphan = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_4")
        .expect("orphan row");
    assert!(!orphan.indented && orphan.group_toggle.is_none());
}

#[test]
fn a_folded_worktree_block_stays_together_inside_its_group() {
    let mut projected = named_spaces(&["Web/p", "plain", "Web/c1", "c2", "Web/z"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 3, true);
    in_worktree(&mut projected, 4, true);
    projected.workspaces[3].custom_label = false;
    projected.workspaces[3].branch = Some("worktree/c2".into());
    let mut state = grouped_state(focused_on(projected, "ws_4"));
    state.collapsed_groups.insert("/repo/.git".into());
    let icon = icon(&state);
    let lines = body_lines(&mut state, 6);
    assert!(lines[1].ends_with('▸'), "{lines:?}");
    assert_eq!(
        lines
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            " ▾ Web".to_owned(),
            format!("   {icon} p"),
            format!("     └─ {icon} c2"),
            format!("   {icon} z"),
            format!(" {icon} plain"),
            String::new(),
        ]
    );
}

#[test]
fn a_dragged_group_member_is_highlighted_across_the_whole_row() {
    let mut state = grouped_state(named_spaces(&["A/one", "A/two", "plain"]));
    state.chrome_drag = Some(ClientChromeDrag::Workspace {
        source_workspace_id: "ws_2".into(),
        target: None,
    });
    let buffer = state
        .compose(106, 30)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    let dragged = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("dragged row")
        .rect;
    for x in dragged.x..dragged.right() {
        assert_eq!(
            buffer[(x, dragged.y)].bg,
            state.config.palette.surface1,
            "column {x}"
        );
    }
}

#[test]
fn row_gap_skips_duplicate_parent_rows_inside_a_group() {
    let mut projected = named_spaces(&["Web/p", "Web/q", "Web/c", "plain"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(projected);
    state.config.spaces.row_gap = 1;
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 7)
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            " ▾ Web".to_owned(),
            format!("   {icon} p"),
            format!("   {icon} q"),
            format!("     └─ {icon} c"),
            String::new(),
            format!(" {icon} plain"),
            String::new(),
        ]
    );
}

fn mouse(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    point: (u16, u16),
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column: point.0,
        row: point.1,
        modifiers: KeyModifiers::empty(),
    })])
}

/// A left press and release at `point`; the actions both sent.
fn click(state: &mut ClientShellState, point: (u16, u16)) -> Vec<ClientShellAction> {
    let mut actions = mouse(state, MouseEventKind::Down(MouseButton::Left), point).actions;
    actions.extend(mouse(state, MouseEventKind::Up(MouseButton::Left), point).actions);
    actions
}

fn click_header(state: &mut ClientShellState, group: &str) -> Vec<ClientShellAction> {
    let point = header_point(state, group);
    click(state, point)
}

/// A cell on `group`'s header in the last frame, right of its chevron.
fn header_point(state: &ClientShellState, group: &str) -> (u16, u16) {
    let hit = state
        .hits
        .group_headers
        .iter()
        .find(|hit| hit.group == group)
        .expect("group header hit");
    (hit.rect.x + 4, hit.rect.y)
}

fn space_point(state: &ClientShellState, workspace_id: &str) -> (u16, u16) {
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("space hit");
    (hit.rect.x + 2, hit.rect.y)
}

fn has_hit(state: &ClientShellState, workspace_id: &str) -> bool {
    state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == workspace_id)
}

fn navigate(state: &mut ClientShellState) {
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn clicking_a_group_header_folds_and_unfolds_it() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    let icon = icon(&state);
    state.compose(106, 30).expect("composed frame");
    let header = header_point(&state, "A");

    let press = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header);
    assert!(press.actions.is_empty());
    assert!(
        state.collapsed_groups.is_empty(),
        "a press alone doesn't fold"
    );
    assert!(state.workspace_press.is_none() && state.chrome_drag.is_none());
    let release = mouse(&mut state, MouseEventKind::Up(MouseButton::Left), header);
    assert!(release.actions.is_empty() && release.repaint);
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:A".to_owned()])
    );
    assert_eq!(
        body_lines(&mut state, 3),
        [
            format!(" ▸ {icon} A"),
            format!(" {icon} plain"),
            String::new()
        ]
    );
    assert!(!has_hit(&state, "ws_1") && !has_hit(&state, "ws_2"));

    assert!(click_header(&mut state, "A").is_empty());
    assert!(state.collapsed_groups.is_empty());
    assert_eq!(
        body_lines(&mut state, 4),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
        ]
    );
}

#[test]
fn a_header_press_that_drags_does_not_fold() {
    let mut state = grouped_state(named_spaces(&["A/one", "A/two", "plain"]));
    state.compose(106, 30).expect("composed frame");
    let header = header_point(&state, "A");
    let plain = space_point(&state, "ws_3");

    // Dragged onto a space and released there: no fold, no space press, no space drag (the
    // group's drag drops right after the group, which changes nothing).
    let mut actions = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header).actions;
    actions.extend(mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), plain).actions);
    assert!(matches!(
        state.chrome_drag.as_ref(),
        Some(ClientChromeDrag::SpaceGroups(drag)) if drag.source == GroupDragSource::Header("A".into())
    ));
    assert!(state.workspace_press.is_none());
    actions.extend(mouse(&mut state, MouseEventKind::Up(MouseButton::Left), plain).actions);
    assert!(
        actions.is_empty(),
        "the release doesn't focus the space under it"
    );
    assert!(state.collapsed_groups.is_empty());

    // Dragged away and back onto the header.
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header);
    mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        (header.0, header.1 + 1),
    );
    mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), header);
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), header);
    assert!(state.collapsed_groups.is_empty());

    click(&mut state, header);
    assert!(state.collapsed_groups.contains("group:A"));
}

#[test]
fn a_folded_header_shows_its_most_urgent_member_status() {
    use crate::config::StatusIndicatorStyle;

    let mut projected = named_spaces(&["A/one", "A/two", "A/three", "plain"]);
    for (index, status) in [
        (0, AgentStatus::Done),
        (1, AgentStatus::Blocked),
        (2, AgentStatus::Idle),
    ] {
        projected.workspaces[index].agent_status = status;
    }
    let mut state = grouped_state(focused_on(projected, "ws_4"));
    state.config.status_indicators = StatusIndicatorStyle::Symbols;
    state.collapsed_groups.insert("group:A".into());
    let buffer = state
        .compose(106, 30)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    let palette = state.config.palette.clone();
    let body = state.hits.workspace_body;
    let icon = &buffer[(body.x + 3, body.y)];
    assert_eq!(icon.symbol(), "×");
    assert_eq!(icon.fg, status_color(AgentStatus::Blocked, &palette));
    let name = &buffer[(body.x + 5, body.y)];
    assert_eq!(name.symbol(), "A");
    assert_eq!(name.fg, palette.text);
    assert!(name.modifier.contains(Modifier::BOLD));
    assert_eq!(body_lines(&mut state, 1), [" ▸ × A"]);

    // A blocked worktree child hidden by its block's own fold still counts.
    let mut projected = named_spaces(&["Web/p", "Web/c", "Web/z", "plain"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, true);
    for (index, status) in [
        (0, AgentStatus::Idle),
        (1, AgentStatus::Blocked),
        (2, AgentStatus::Done),
    ] {
        projected.workspaces[index].agent_status = status;
    }
    let mut state = grouped_state(focused_on(projected, "ws_4"));
    state.config.status_indicators = StatusIndicatorStyle::Symbols;
    state.collapsed_groups.insert("/repo/.git".into());
    state.collapsed_groups.insert("group:Web".into());
    assert_eq!(body_lines(&mut state, 1), [" ▸ × Web"]);
    state.collapsed_groups.remove("/repo/.git");
    assert_eq!(body_lines(&mut state, 1), [" ▸ × Web"]);
    state.collapsed_groups.insert("/repo/.git".into());
    state.collapsed_groups.remove("group:Web");
    assert_eq!(body_lines(&mut state, 1), [" ▾ Web"], "unfolded: no icon");
}

#[test]
fn a_folded_group_still_shows_its_focused_member_and_the_navigate_preview() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "A/three", "plain"]),
        "ws_2",
    ));
    state.collapsed_groups.insert("group:A".into());
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 4),
        [
            format!(" ▸ {icon} A"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
            String::new(),
        ]
    );

    navigate(&mut state);
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(
        body_lines(&mut state, 4),
        [
            format!(" ▸ {icon} A"),
            format!("   {icon} two"),
            format!("   {icon} three"),
            format!(" {icon} plain"),
        ]
    );
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(
        body_lines(&mut state, 4),
        [
            format!(" ▸ {icon} A"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
            String::new(),
        ]
    );

    // A worktree child shows under its block's parent row, as the block's last child.
    let mut projected = named_spaces(&["Web/p", "Web/c1", "Web/c2", "plain"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, true);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(focused_on(projected, "ws_2"));
    state.collapsed_groups.insert("group:Web".into());
    let lines = body_lines(&mut state, 5);
    assert!(lines[1].ends_with('▾'), "{lines:?}");
    assert_eq!(
        lines
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            format!(" ▸ {icon} Web"),
            format!("   {icon} p"),
            format!("     └─ {icon} c1"),
            format!(" {icon} plain"),
            String::new(),
        ]
    );
}

#[test]
fn navigation_visits_the_members_of_a_folded_group() {
    use crate::input::KeybindAction;

    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "plain", "A/two"]),
        "ws_2",
    ));
    state.collapsed_groups.insert("group:A".into());
    let icon = icon(&state);
    state.compose(106, 30).expect("composed frame");

    assert_eq!(
        focus_request(&mut state, KeybindAction::NextWorkspace),
        "ws_1"
    );
    set_focus(&mut state, "ws_1");
    assert_eq!(
        body_lines(&mut state, 3),
        [
            format!(" ▸ {icon} A"),
            format!("   {icon} one"),
            format!(" {icon} plain"),
        ]
    );
    assert_eq!(
        focus_request(&mut state, KeybindAction::NextWorkspace),
        "ws_3"
    );
    set_focus(&mut state, "ws_3");
    assert_eq!(
        body_lines(&mut state, 3),
        [
            format!(" ▸ {icon} A"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
        ]
    );
    assert_eq!(
        focus_request(&mut state, KeybindAction::PreviousWorkspace),
        "ws_1"
    );
    for (index, workspace_id) in ["ws_1", "ws_3", "ws_2"].into_iter().enumerate() {
        assert_eq!(
            focus_request(&mut state, KeybindAction::SwitchWorkspace(index)),
            workspace_id
        );
    }

    set_focus(&mut state, "ws_2");
    state.compose(106, 30).expect("composed frame");
    navigate(&mut state);
    for (workspace_id, short) in [("ws_1", "one"), ("ws_3", "two")] {
        state.handle_input_bytes(b"\x1b[B");
        assert_eq!(
            state.navigate_workspace_id,
            state.navigation_target(&ClientEndpointId::Local, workspace_id)
        );
        assert_eq!(
            body_lines(&mut state, 3),
            [
                format!(" ▸ {icon} A"),
                format!("   {icon} {short}"),
                format!(" {icon} plain"),
            ]
        );
    }
}

/// Plain spaces `p1`.., the folded group `A` (`a1`..), then plain spaces `q1`..; focused on
/// `focus`, composed once at 106x20 (a 7-row sidebar body).
fn around_folded_group(
    before: usize,
    members: usize,
    after: usize,
    focus: &str,
) -> ClientShellState {
    let labels = (1..=before)
        .map(|number| format!("p{number}"))
        .chain((1..=members).map(|number| format!("A/a{number}")))
        .chain((1..=after).map(|number| format!("q{number}")))
        .collect::<Vec<_>>();
    let projected = named_spaces(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    let mut state = grouped_state(focused_on(projected, focus));
    state.collapsed_groups.insert("group:A".into());
    state.compose(106, 20).expect("composed frame");
    assert_eq!(state.hits.workspace_body.height, 7);
    state
}

/// Whether `group`'s header is drawn on the row right above `workspace_id`'s row.
fn header_right_above(state: &ClientShellState, group: &str, workspace_id: &str) -> bool {
    let Some(space) = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
    else {
        return false;
    };
    state
        .hits
        .group_headers
        .iter()
        .any(|hit| hit.group == group && hit.rect.y + 1 == space.rect.y)
}

fn preview_is(state: &ClientShellState, workspace_id: &str) -> bool {
    state.navigate_workspace_id == state.navigation_target(&ClientEndpointId::Local, workspace_id)
}

#[test]
fn revealing_a_space_counts_the_rows_of_a_folded_group_above_it() {
    // `p1`..`p10` (`ws_1`..), `a1`..`a10` (`ws_11`..) folded, `q1`..`q20` (`ws_21`..).
    let mut state = around_folded_group(10, 10, 20, "ws_1");

    // Navigate mode: every preview is in view, the header with the group's first drawn member.
    navigate(&mut state);
    for number in 2..=40 {
        let workspace_id = format!("ws_{number}");
        state.handle_input_bytes(b"\x1b[B");
        assert!(preview_is(&state, &workspace_id));
        state.compose(106, 20).expect("composed frame");
        assert!(has_hit(&state, &workspace_id), "{workspace_id} hidden");
        if (11..=20).contains(&number) {
            assert!(
                header_right_above(&state, "A", &workspace_id),
                "{workspace_id}"
            );
        }
    }

    // Switching by key scrolls before the server answers.
    let mut state = around_folded_group(10, 10, 20, "ws_1");
    assert_eq!(
        focus_request(&mut state, crate::input::KeybindAction::SwitchWorkspace(29)),
        "ws_30"
    );
    state.compose(106, 20).expect("composed frame");
    assert!(has_hit(&state, "ws_30"));
}

#[test]
fn a_preview_in_a_folded_group_at_the_end_of_the_list_is_drawn() {
    // Six spaces and the header fill the body exactly; ↑ wraps to the group's last member.
    let mut state = around_folded_group(6, 2, 0, "ws_1");
    navigate(&mut state);
    state.handle_input_bytes(b"\x1b[A");
    assert!(preview_is(&state, "ws_8"));
    state.compose(106, 20).expect("composed frame");
    assert!(header_right_above(&state, "A", "ws_8"));

    // ↓ from the row above the group into it, then through its members.
    let mut state = around_folded_group(30, 3, 0, "ws_1");
    navigate(&mut state);
    for number in 2..=33 {
        let workspace_id = format!("ws_{number}");
        state.handle_input_bytes(b"\x1b[B");
        assert!(preview_is(&state, &workspace_id));
        state.compose(106, 20).expect("composed frame");
        assert!(has_hit(&state, &workspace_id), "{workspace_id} hidden");
    }
    assert!(header_right_above(&state, "A", "ws_33"));

    // The focused member is drawn last; ↓ previews the next member below it.
    let mut state = around_folded_group(30, 3, 0, "ws_31");
    navigate(&mut state);
    state.handle_input_bytes(b"\x1b[B");
    assert!(preview_is(&state, "ws_32"));
    state.compose(106, 20).expect("composed frame");
    assert!(header_right_above(&state, "A", "ws_31"));
    assert!(has_hit(&state, "ws_32"));
}

#[test]
fn moving_to_a_hidden_member_that_fits_in_view_does_not_scroll() {
    use crate::input::KeybindAction;

    // `p1`..`p20`, `a1`..`a3` (`ws_21`..`ws_23`) folded, `q1`..`q20`; `a1` is focused.
    let scrolled_to = |scroll: usize| {
        let mut state = around_folded_group(20, 3, 20, "ws_21");
        state.workspace_scroll = scroll;
        state.compose(106, 20).expect("composed frame");
        state
    };
    let mut state = scrolled_to(19);
    let icon = icon(&state);
    let rows = |names: &[&str]| {
        names
            .iter()
            .map(|name| match *name {
                "A" => format!(" ▸ {icon} A"),
                member if member.starts_with('a') => format!("   {icon} {member}"),
                plain => format!(" {icon} {plain}"),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        body_lines_at(&mut state, 20, 7),
        rows(&["p20", "A", "a1", "q1", "q2", "q3", "q4"])
    );

    // Navigate ↓ to `a2`: drawn under `a1` in place.
    navigate(&mut state);
    state.handle_input_bytes(b"\x1b[B");
    assert!(preview_is(&state, "ws_22"));
    assert_eq!(
        body_lines_at(&mut state, 20, 7),
        rows(&["p20", "A", "a1", "a2", "q1", "q2", "q3"])
    );
    assert_eq!(state.workspace_scroll, 19);

    // Next space to `a2`: nothing scrolls before or after the server answers.
    let mut state = scrolled_to(19);
    assert_eq!(
        focus_request(&mut state, KeybindAction::NextWorkspace),
        "ws_22"
    );
    assert_eq!(
        body_lines_at(&mut state, 20, 7),
        rows(&["p20", "A", "a1", "q1", "q2", "q3", "q4"])
    );
    set_focus(&mut state, "ws_22");
    assert_eq!(
        body_lines_at(&mut state, 20, 7),
        rows(&["p20", "A", "a2", "q1", "q2", "q3", "q4"])
    );

    // Out of view: the least scroll that shows it, under `a1`.
    let mut state = scrolled_to(0);
    assert_eq!(
        focus_request(&mut state, KeybindAction::NextWorkspace),
        "ws_22"
    );
    assert_eq!(state.workspace_scroll, 16);
    set_focus(&mut state, "ws_22");
    assert_eq!(
        body_lines_at(&mut state, 20, 7),
        rows(&["p17", "p18", "p19", "p20", "A", "a2", "q1"])
    );
}

#[test]
fn group_headers_get_no_hits_without_mouse_capture() {
    let mut state = grouped_state(named_spaces(&["A/one", "plain"]));
    state.config.mouse_capture = false;
    state.compose(106, 30).expect("composed frame");
    assert!(state.hits.group_headers.is_empty());
    let body = state.hits.workspace_body;
    click(&mut state, (body.x + 4, body.y));
    assert!(state.collapsed_groups.is_empty());
    assert_eq!(body_lines(&mut state, 1), [" ▾ A"]);
    let right = mouse(
        &mut state,
        MouseEventKind::Down(MouseButton::Right),
        (body.x + 4, body.y),
    );
    assert!(right.actions.is_empty());
    assert!(state.overlay.is_none(), "no header menu");
}

/// A new window preferences file holding `contents`.
fn preferences_file(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "herdr-space-group-folds-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::write(&path, contents).expect("write preferences");
    path
}

/// A window that loads and saves its preferences at `path`.
fn with_preferences(path: &std::path::Path, projected: ClientShellSnapshot) -> ClientShellState {
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.into());
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

#[test]
fn group_folds_are_saved_with_the_window_preferences() {
    let path = preferences_file(r#"{"collapsed_groups":["/repo","group:A"]}"#);
    let open = |path: &std::path::Path| {
        with_preferences(path, focused_on(named_spaces(&["A/one", "plain"]), "ws_2"))
    };

    let mut state = open(&path);
    let icon = icon(&state);
    assert_eq!(body_lines(&mut state, 1), [format!(" ▸ {icon} A")]);
    click_header(&mut state, "A");
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(
        stored.contains("/repo") && !stored.contains("group:A"),
        "{stored}"
    );
    click_header(&mut state, "A");
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(stored.contains("\"group:A\""), "{stored}");

    let mut reopened = open(&path);
    assert_eq!(body_lines(&mut reopened, 1), [format!(" ▸ {icon} A")]);
    std::fs::remove_file(path).expect("remove preferences");
}

#[test]
fn worktree_folds_inside_a_group_stay_independent_of_the_group_fold() {
    let mut projected = named_spaces(&["Web/p", "Web/c1", "Web/c2", "plain"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, true);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(focused_on(projected, "ws_4"));
    let icon = icon(&state);
    state.compose(106, 30).expect("composed frame");
    let toggle = |state: &ClientShellState| {
        let hit = state
            .hits
            .workspaces
            .iter()
            .find(|hit| hit.workspace_id == "ws_1")
            .expect("parent row");
        let (rect, _) = hit.group_toggle.as_ref().expect("worktree toggle");
        (rect.x, rect.y)
    };

    let point = toggle(&state);
    click(&mut state, point);
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["/repo/.git".to_owned()])
    );
    let lines = body_lines(&mut state, 3);
    assert!(lines[1].ends_with('▸'), "{lines:?}");
    assert_eq!(lines[2], format!(" {icon} plain"));

    click_header(&mut state, "Web");
    assert_eq!(
        body_lines(&mut state, 2),
        [format!(" ▸ {icon} Web"), format!(" {icon} plain")]
    );
    click_header(&mut state, "Web");
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["/repo/.git".to_owned()])
    );
    let lines = body_lines(&mut state, 3);
    assert!(lines[1].ends_with('▸'), "{lines:?}");
    assert_eq!(without_toggle(&lines[1]), format!("   {icon} p"));

    let point = toggle(&state);
    click(&mut state, point);
    assert!(state.collapsed_groups.is_empty());
    assert_eq!(
        body_lines(&mut state, 5)
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            " ▾ Web".to_owned(),
            format!("   {icon} p"),
            format!("     ├─ {icon} c1"),
            format!("     └─ {icon} c2"),
            format!(" {icon} plain"),
        ]
    );
}

fn right_click(state: &mut ClientShellState, point: (u16, u16)) -> ClientShellInput {
    mouse(state, MouseEventKind::Down(MouseButton::Right), point)
}

fn key(state: &mut ClientShellState, code: KeyCode) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        code,
        KeyModifiers::NONE,
    ))])
}

/// Right-clicks `group`'s header in a new frame.
fn open_header_menu(state: &mut ClientShellState, group: &str) {
    state.compose(106, 30).expect("composed frame");
    let point = header_point(state, group);
    let outcome = right_click(state, point);
    assert!(outcome.actions.is_empty() && outcome.repaint);
}

/// The open context menu's target and item labels.
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

/// Clicks the open menu's item `label` where a new frame draws it.
fn choose(state: &mut ClientShellState, label: &str) -> ClientShellInput {
    state.compose(106, 30).expect("menu frame");
    let index = open_menu(state)
        .1
        .iter()
        .position(|item| *item == label)
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

/// Opens the rename box of `group` from its header menu.
fn open_group_rename(state: &mut ClientShellState, group: &str) {
    open_header_menu(state, group);
    assert!(choose(state, "Rename group...").actions.is_empty());
}

/// Replaces the rename box's text with `text` and presses Enter; the renames sent, as
/// `(workspace id, new name)`.
fn submit_rename(state: &mut ClientShellState, text: &str) -> Vec<(String, String)> {
    state.handle_input_bytes(&[0x15]);
    if !text.is_empty() {
        state.handle_input_bytes(text.as_bytes());
    }
    let save = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none(), "Enter closes the rename box");
    save.actions
        .iter()
        .map(|action| {
            let ClientShellAction::Endpoint { request, .. } = action else {
                panic!("not an endpoint request: {action:?}");
            };
            let crate::api::schema::Method::WorkspaceRename(params) = &request.method else {
                panic!("not a space rename: {:?}", request.method);
            };
            (params.workspace_id.clone(), params.label.clone())
        })
        .collect()
}

fn renamed(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(workspace_id, label)| ((*workspace_id).to_owned(), (*label).to_owned()))
        .collect()
}

/// The server's answer to `renames`: the next snapshot, with the new names.
fn apply_renames(state: &mut ClientShellState, renames: &[(String, String)]) {
    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.revision += 1;
    for (workspace_id, label) in renames {
        let workspace = projected
            .workspaces
            .iter_mut()
            .find(|workspace| &workspace.workspace_id == workspace_id)
            .expect("renamed space");
        workspace.label = label.clone();
    }
    let mut projected_surface = surface();
    projected_surface.projection_revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(projected_surface);
}

#[test]
fn right_clicking_a_group_header_opens_its_menu() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    let icon = icon(&state);
    open_header_menu(&mut state, "A");
    let (target, labels) = open_menu(&state);
    assert!(matches!(
        target,
        ClientContextMenuTarget::SpaceGroup { name, collapsed: false } if name == "A"
    ));
    assert_eq!(labels, ["Collapse", "Rename group..."]);

    // Drawn at the pointer, like the space menu.
    let pointer = header_point(&state, "A");
    let frame = state.compose(106, 30).expect("menu frame");
    let rows = state.hits.context_menu_rows.clone();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].0.x, rows[0].0.y), (pointer.0 + 1, pointer.1 + 1));
    let lines = frame_rows(&frame);
    for ((row, _), label) in rows.iter().zip(["Collapse", "Rename group..."]) {
        let line = lines[usize::from(row.y)]
            .chars()
            .skip(usize::from(row.x))
            .take(usize::from(row.width))
            .collect::<String>();
        assert_eq!(line.trim_end(), label);
    }

    // Highlighted and chosen with the keys like any context menu.
    key(&mut state, KeyCode::Down);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("menu still open");
    };
    assert_eq!(menu.highlighted, 1);
    key(&mut state, KeyCode::Up);
    let collapse = key(&mut state, KeyCode::Enter);
    assert!(collapse.actions.is_empty() && collapse.repaint);
    assert!(state.overlay.is_none());
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:A".to_owned()])
    );
    assert_eq!(
        body_lines(&mut state, 2),
        [format!(" ▸ {icon} A"), format!(" {icon} plain")]
    );

    open_header_menu(&mut state, "A");
    let (target, labels) = open_menu(&state);
    assert!(matches!(
        target,
        ClientContextMenuTarget::SpaceGroup { name, collapsed: true } if name == "A"
    ));
    assert_eq!(labels, ["Expand", "Rename group..."]);
    key(&mut state, KeyCode::Esc);
    assert!(state.overlay.is_none());
    assert!(
        state.collapsed_groups.contains("group:A"),
        "Esc does nothing"
    );

    open_header_menu(&mut state, "A");
    assert!(choose(&mut state, "Expand").actions.is_empty());
    assert!(state.overlay.is_none() && state.collapsed_groups.is_empty());
    assert_eq!(
        body_lines(&mut state, 4),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
        ]
    );

    // Not from a sidebar that was just collapsed (its header hits are from the last frame).
    state.sidebar_collapsed = true;
    assert!(right_click(&mut state, pointer).actions.is_empty());
    assert!(state.overlay.is_none());
}

#[test]
fn the_header_menu_folds_and_saves_like_a_click() {
    let path = preferences_file(r#"{"collapsed_groups":["/repo"]}"#);
    let mut state = with_preferences(&path, focused_on(named_spaces(&["A/one", "plain"]), "ws_2"));
    let icon = icon(&state);

    open_header_menu(&mut state, "A");
    choose(&mut state, "Collapse");
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(
        stored.contains("/repo") && stored.contains("\"group:A\""),
        "{stored}"
    );
    let mut reopened =
        with_preferences(&path, focused_on(named_spaces(&["A/one", "plain"]), "ws_2"));
    assert_eq!(body_lines(&mut reopened, 1), [format!(" ▸ {icon} A")]);

    open_header_menu(&mut state, "A");
    choose(&mut state, "Expand");
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(
        stored.contains("/repo") && !stored.contains("group:A"),
        "{stored}"
    );
    std::fs::remove_file(path).expect("remove preferences");
}

#[test]
fn right_clicking_a_group_member_still_opens_the_space_menu() {
    let mut state = grouped_state(named_spaces(&["A/one", "A/two", "plain"]));
    state.compose(106, 30).expect("composed frame");
    let point = space_point(&state, "ws_2");
    assert!(right_click(&mut state, point).actions.is_empty());
    let (target, labels) = open_menu(&state);
    assert!(matches!(
        target,
        ClientContextMenuTarget::Workspace { workspace_id, .. } if workspace_id == "ws_2"
    ));
    assert_eq!(labels, ["Rename", "Move to group", "Close"]);

    choose(&mut state, "Rename");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename workspace",
            ref input,
            target: ClientRenameTarget::Workspace { ref workspace_id },
        })) if input.as_str() == "A/two" && workspace_id == "ws_2"
    ));
}

#[test]
fn rename_group_renames_every_member_by_its_own_name() {
    let mut projected = named_spaces(&[
        "Web/one",
        "plain",
        "Web / two",
        "Other/x",
        "Web/app/x",
        "web/y",
        "Web/p",
        "c1",
        "Other/c2",
        "Webby/z",
    ]);
    projected.workspaces.push(space(11, "Web/auto", false));
    // `Web/p` heads a worktree block whose linked children are named otherwise.
    in_worktree(&mut projected, 7, false);
    in_worktree(&mut projected, 8, true);
    in_worktree(&mut projected, 9, true);
    let mut state = grouped_state(projected);
    let icon = icon(&state);
    let web_rows = [
        " ▾ Web".to_owned(),
        format!("   {icon} one"),
        format!("   {icon} two"),
        format!("   {icon} app/x"),
        format!("   {icon} p"),
        format!("     ├─ {icon} c1"),
        format!("     └─ {icon} Other/c2"),
    ];
    let lines = body_lines(&mut state, 7)
        .iter()
        .map(|line| without_toggle(line))
        .collect::<Vec<_>>();
    assert_eq!(lines, web_rows, "the children are drawn in Web");

    open_group_rename(&mut state, "Web");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename group",
            ref input,
            target: ClientRenameTarget::SpaceGroup { ref name },
        })) if input.as_str() == "Web" && name == "Web"
    ));
    let renames = submit_rename(&mut state, "  Sites  ");
    assert_eq!(
        renames,
        renamed(&[
            ("ws_1", "Sites/one"),
            ("ws_3", "Sites/two"),
            ("ws_5", "Sites/app/x"),
            ("ws_7", "Sites/p"),
        ])
    );
    assert!(state.collapsed_groups.is_empty());

    apply_renames(&mut state, &renames);
    let lines = body_lines(&mut state, 7)
        .iter()
        .map(|line| without_toggle(line))
        .collect::<Vec<_>>();
    let mut sites_rows = web_rows;
    sites_rows[0] = " ▾ Sites".to_owned();
    assert_eq!(lines, sites_rows, "the same rows under the new name");
}

#[test]
fn renaming_a_folded_group_moves_its_fold() {
    let path = preferences_file(r#"{"collapsed_groups":["/repo","group:A"]}"#);
    let mut state = with_preferences(
        &path,
        focused_on(named_spaces(&["A/one", "A/two", "plain"]), "ws_3"),
    );
    let icon = icon(&state);
    assert_eq!(body_lines(&mut state, 1), [format!(" ▸ {icon} A")]);

    open_group_rename(&mut state, "A");
    let renames = submit_rename(&mut state, "B");
    assert_eq!(renames, renamed(&[("ws_1", "B/one"), ("ws_2", "B/two")]));
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["/repo".to_owned(), "group:B".to_owned()])
    );
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(
        stored.contains("/repo") && stored.contains("\"group:B\"") && !stored.contains("group:A"),
        "{stored}"
    );

    apply_renames(&mut state, &renames);
    assert_eq!(
        body_lines(&mut state, 3),
        [
            format!(" ▸ {icon} B"),
            format!(" {icon} plain"),
            String::new()
        ]
    );
    std::fs::remove_file(path).expect("remove preferences");
}

#[test]
fn renaming_a_group_onto_another_merges_them() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "plain", "B/two", "A/three"]),
        "ws_2",
    ));
    let icon = icon(&state);
    state.collapsed_groups.insert("group:B".into());

    open_group_rename(&mut state, "A");
    let renames = submit_rename(&mut state, "B");
    assert_eq!(renames, renamed(&[("ws_1", "B/one"), ("ws_4", "B/three")]));
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:B".to_owned()])
    );

    apply_renames(&mut state, &renames);
    assert_eq!(
        body_lines(&mut state, 3),
        [
            format!(" ▸ {icon} B"),
            format!(" {icon} plain"),
            String::new()
        ],
        "one group, at its first member's place, folded as B was"
    );
    click_header(&mut state, "B");
    assert_eq!(
        body_lines(&mut state, 5),
        [
            " ▾ B".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} two"),
            format!("   {icon} three"),
            format!(" {icon} plain"),
        ]
    );
}

#[test]
fn rename_group_sends_nothing_for_an_empty_slashed_or_unchanged_name() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    state.collapsed_groups.insert("group:A".into());
    for text in ["", "   ", "a/b", "/", "B/", "A", "  A "] {
        open_group_rename(&mut state, "A");
        assert_eq!(submit_rename(&mut state, text), [], "{text:?}");
        assert_eq!(
            state.collapsed_groups,
            HashSet::from(["group:A".to_owned()]),
            "{text:?}"
        );
    }
}

#[test]
fn group_rename_plans_and_fold_moves_need_no_window() {
    use crate::client::shell::space_groups::{group_renames, header_menu_items, move_fold};

    let labels = |collapsed| {
        header_menu_items(collapsed)
            .into_iter()
            .map(|item| (item.label.into_owned(), item.action))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(false),
        [
            ("Collapse".to_owned(), ClientContextMenuAction::ToggleGroup),
            (
                "Rename group...".to_owned(),
                ClientContextMenuAction::Rename
            ),
        ]
    );
    assert_eq!(
        labels(true)[0],
        ("Expand".to_owned(), ClientContextMenuAction::ToggleGroup)
    );

    let projected = named_spaces(&["A/one", "A / two", "AB/x", "plain"]);
    assert_eq!(
        group_renames(&projected, "A", "C"),
        renamed(&[("ws_1", "C/one"), ("ws_2", "C/two")])
    );
    assert_eq!(group_renames(&projected, "Z", "C"), []);
    for new in ["", "A", "C/D"] {
        assert_eq!(group_renames(&projected, "A", new), [], "{new:?}");
    }

    let mut folds = HashSet::from(["/repo".to_owned()]);
    assert!(!move_fold(&mut folds, "A", "C"));
    assert_eq!(folds, HashSet::from(["/repo".to_owned()]));
    folds.insert("group:A".into());
    assert!(move_fold(&mut folds, "A", "C"));
    assert_eq!(
        folds,
        HashSet::from(["/repo".to_owned(), "group:C".to_owned()])
    );
    folds.insert("group:D".into());
    assert!(move_fold(&mut folds, "C", "D"));
    assert_eq!(
        folds,
        HashSet::from(["/repo".to_owned(), "group:D".to_owned()])
    );
}

#[test]
fn a_refused_group_rename_keeps_the_fold() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    state.collapsed_groups.insert("group:A".into());

    // The machine drops while the box is open: the renames are refused.
    open_group_rename(&mut state, "A");
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Reconnecting);
    assert_eq!(submit_rename(&mut state, "B"), []);
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:A".to_owned()])
    );

    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
    open_group_rename(&mut state, "A");
    assert_eq!(
        submit_rename(&mut state, "B"),
        renamed(&[("ws_1", "B/one"), ("ws_2", "B/two")])
    );
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:B".to_owned()])
    );
}

#[test]
fn folding_a_group_that_went_away_while_its_menu_was_open_does_nothing() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    let away = renamed(&[("ws_1", "Z/one"), ("ws_2", "Z/two")]);

    open_header_menu(&mut state, "A");
    apply_renames(&mut state, &away);
    assert!(choose(&mut state, "Collapse").actions.is_empty());
    assert!(state.overlay.is_none());
    assert!(state.collapsed_groups.is_empty(), "no stale group:A");

    // Folded, then gone: Expand leaves the set alone too.
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    state.collapsed_groups.insert("group:A".into());
    open_header_menu(&mut state, "A");
    apply_renames(&mut state, &away);
    choose(&mut state, "Expand");
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:A".to_owned()])
    );
}

#[test]
fn a_held_left_press_does_not_fold_the_group_under_its_menu() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    state.compose(106, 30).expect("composed frame");
    let header = header_point(&state, "A");

    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header);
    right_click(&mut state, header);
    let release = mouse(&mut state, MouseEventKind::Up(MouseButton::Left), header);
    assert!(release.actions.is_empty());
    assert!(state.collapsed_groups.is_empty());
    assert_eq!(open_menu(&state).1, ["Collapse", "Rename group..."]);

    choose(&mut state, "Collapse");
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:A".to_owned()])
    );
}

#[test]
fn rename_group_renames_members_drawn_elsewhere_or_hidden() {
    let mut projected = named_spaces(&["Web/one", "Other/p", "Web/c", "plain", "Web/two"]);
    // `Web/c` is a linked child of `Other/p`'s block, so it is drawn under Other.
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(focused_on(projected, "ws_4"));
    let icon = icon(&state);
    state.collapsed_groups = HashSet::from(["group:Web".to_owned(), "/repo/.git".to_owned()]);
    let lines = body_lines(&mut state, 4)
        .iter()
        .map(|line| without_toggle(line))
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            format!(" ▸ {icon} Web"),
            " ▾ Other".to_owned(),
            format!("   {icon} p"),
            format!(" {icon} plain"),
        ],
        "every Web member hidden by a fold"
    );

    open_group_rename(&mut state, "Web");
    let renames = submit_rename(&mut state, "Sites");
    assert_eq!(
        renames,
        renamed(&[
            ("ws_1", "Sites/one"),
            ("ws_3", "Sites/c"),
            ("ws_5", "Sites/two"),
        ])
    );
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:Sites".to_owned(), "/repo/.git".to_owned()])
    );

    apply_renames(&mut state, &renames);
    state.collapsed_groups.remove("/repo/.git");
    let lines = body_lines(&mut state, 5)
        .iter()
        .map(|line| without_toggle(line))
        .collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            format!(" ▸ {icon} Sites"),
            " ▾ Other".to_owned(),
            format!("   {icon} p"),
            format!("     └─ {icon} Sites/c"),
            format!(" {icon} plain"),
        ]
    );
}

#[test]
fn cancelling_the_group_rename_box_or_menu_sends_nothing() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_3",
    ));
    state.collapsed_groups.insert("group:A".into());

    for way in ["Esc", "cancel button", "click outside"] {
        open_group_rename(&mut state, "A");
        let mut actions = state.handle_input_bytes(&[0x15]).actions;
        actions.extend(state.handle_input_bytes(b"B").actions);
        state.compose(106, 30).expect("rename box frame");
        let cancel = state.hits.overlay_cancel;
        actions.extend(
            match way {
                "Esc" => key(&mut state, KeyCode::Esc),
                "cancel button" => click_input(&mut state, (cancel.x + 1, cancel.y)),
                _ => click_input(&mut state, (0, 0)),
            }
            .actions,
        );
        assert!(state.overlay.is_none(), "{way}");
        assert!(actions.is_empty(), "{way}");
        assert_eq!(
            state.collapsed_groups,
            HashSet::from(["group:A".to_owned()]),
            "{way}"
        );
    }

    // A click outside the header menu closes it without reaching the space under it.
    open_header_menu(&mut state, "A");
    let plain = space_point(&state, "ws_3");
    assert!(click(&mut state, plain).is_empty());
    assert!(state.overlay.is_none());
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:A".to_owned()])
    );
}

/// A left press at `point` (a click on a popup acts on the press).
fn click_input(state: &mut ClientShellState, point: (u16, u16)) -> ClientShellInput {
    mouse(state, MouseEventKind::Down(MouseButton::Left), point)
}

#[test]
fn the_header_menu_on_a_scrolled_sidebar_opens_for_the_group_drawn_there() {
    let mut state = grouped_state(ten_groups());
    let icon = icon(&state);
    state.compose(106, 20).expect("composed frame");
    state.workspace_scroll = 6;
    assert_eq!(
        body_lines_at(&mut state, 20, 2),
        [" ▾ G4".to_owned(), format!("   {icon} space4")]
    );
    let body = state.hits.workspace_body;
    assert!(state.hits.group_headers.iter().all(|hit| hit.group != "G1"));

    assert!(right_click(&mut state, (body.x + 4, body.y))
        .actions
        .is_empty());
    assert!(matches!(
        open_menu(&state).0,
        ClientContextMenuTarget::SpaceGroup { name, .. } if name == "G4"
    ));
    key(&mut state, KeyCode::Esc);

    right_click(&mut state, (body.x + 4, body.y + 1));
    assert!(matches!(
        open_menu(&state).0,
        ClientContextMenuTarget::Workspace { workspace_id, .. } if workspace_id == "ws_4"
    ));
}

/// The space renames and moves `actions` sent, as `rename <id> <name>` and `move <ids> before
/// <id|end>`.
fn sent(actions: &[ClientShellAction]) -> Vec<String> {
    sent_methods(actions).iter().map(described).collect()
}

fn sent_methods(actions: &[ClientShellAction]) -> Vec<crate::api::schema::Method> {
    actions
        .iter()
        .map(|action| {
            let ClientShellAction::Endpoint { request, .. } = action else {
                panic!("not an endpoint request: {action:?}");
            };
            request.method.clone()
        })
        .collect()
}

fn described(method: &crate::api::schema::Method) -> String {
    use crate::api::schema::Method;

    match method {
        Method::WorkspaceRename(params) => {
            format!("rename {} {}", params.workspace_id, params.label)
        }
        Method::WorkspaceMoveBlock(params) => format!(
            "move {} before {}",
            params.workspace_ids.join(","),
            params.before_workspace_id.as_deref().unwrap_or("end")
        ),
        other => panic!("not a space rename or move: {other:?}"),
    }
}

/// Right-clicks `workspace_id`'s row in a new frame and chooses "Move to group".
fn open_move_menu(state: &mut ClientShellState, workspace_id: &str) {
    state.compose(106, 30).expect("composed frame");
    let point = space_point(state, workspace_id);
    assert!(right_click(state, point).actions.is_empty());
    assert!(choose(state, "Move to group").actions.is_empty());
    assert!(matches!(
        open_menu(state).0,
        ClientContextMenuTarget::MoveToGroup { workspace_id: id, .. } if id == workspace_id
    ));
}

/// Chooses `item` of `workspace_id`'s "Move to group" menu; what that sent.
fn move_to(state: &mut ClientShellState, workspace_id: &str, item: &str) -> Vec<String> {
    open_move_menu(state, workspace_id);
    let outcome = choose(state, item);
    assert!(state.overlay.is_none(), "{item} closes the menu");
    sent(&outcome.actions)
}

/// The labels of `workspace_id`'s space menu.
fn space_menu(state: &mut ClientShellState, workspace_id: &str) -> Vec<String> {
    state.compose(106, 30).expect("composed frame");
    let point = space_point(state, workspace_id);
    right_click(state, point);
    let (target, labels) = open_menu(state);
    assert!(matches!(
        target,
        ClientContextMenuTarget::Workspace { workspace_id: id, .. } if id == workspace_id
    ));
    key(state, KeyCode::Esc);
    labels
}

/// The server's answer to the renames and moves sent: the next snapshot.
fn apply_sent(state: &mut ClientShellState, methods: &[crate::api::schema::Method]) {
    use crate::api::schema::Method;

    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.revision += 1;
    for method in methods {
        match method {
            Method::WorkspaceRename(params) => {
                let workspace = projected
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.workspace_id == params.workspace_id)
                    .expect("renamed space");
                workspace.label = params.label.clone();
                workspace.custom_label = true;
            }
            Method::WorkspaceMoveBlock(params) => {
                let (mut moved, mut rest): (Vec<_>, Vec<_>) = projected
                    .workspaces
                    .drain(..)
                    .partition(|workspace| params.workspace_ids.contains(&workspace.workspace_id));
                moved.sort_by_key(|workspace| {
                    params
                        .workspace_ids
                        .iter()
                        .position(|id| *id == workspace.workspace_id)
                });
                let at = params
                    .before_workspace_id
                    .as_ref()
                    .and_then(|before| {
                        rest.iter()
                            .position(|workspace| &workspace.workspace_id == before)
                    })
                    .unwrap_or(rest.len());
                rest.splice(at..at, moved);
                projected.workspaces = rest;
            }
            other => panic!("not a space rename or move: {other:?}"),
        }
    }
    let mut projected_surface = surface();
    projected_surface.projection_revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(projected_surface);
}

/// The space ids in drawn order (the navigation list) and in server order.
fn drawn_and_server_order(state: &ClientShellState) -> (Vec<String>, Vec<String>) {
    let snapshot = state.snapshot.as_deref().expect("snapshot");
    let id = |index: usize| snapshot.workspaces[index].workspace_id.clone();
    let drawn = state
        .navigation_workspace_entries(snapshot)
        .iter()
        .map(|entry| id(entry.index))
        .collect();
    let server = (0..snapshot.workspaces.len()).map(id).collect();
    (drawn, server)
}

#[test]
fn the_space_menu_offers_move_to_group_after_rename() {
    let mut projected = named_spaces(&["A/one", "plain", "Web/p", "c1", "orphan"]);
    in_worktree(&mut projected, 3, false);
    in_worktree(&mut projected, 4, true);
    projected.workspaces[4].worktree = Some(ClientShellWorktree {
        key: "/other/.git".into(),
        label: "other".into(),
        is_linked_worktree: true,
    });
    let mut state = grouped_state(projected);

    for workspace_id in ["ws_1", "ws_2"] {
        assert_eq!(
            space_menu(&mut state, workspace_id),
            ["Rename", "Move to group", "Close"],
            "{workspace_id}"
        );
    }
    let parent = space_menu(&mut state, "ws_3");
    assert_eq!(parent[..2], ["Rename", "Move to group"]);
    assert!(parent.iter().any(|label| label == "Collapse"));
    // A linked worktree child goes where its parent goes; an orphan one moves on its own.
    let child = space_menu(&mut state, "ws_4");
    assert_eq!(child[0], "Rename");
    assert!(!child.iter().any(|label| label == "Move to group"));
    assert_eq!(
        space_menu(&mut state, "ws_5"),
        [
            "Rename",
            "Move to group",
            "Close",
            "Delete worktree checkout..."
        ]
    );
}

#[test]
fn move_to_group_lists_the_other_groups_in_drawn_order() {
    let mut projected = named_spaces(&["B/one", "plain", "A/two", "C/three", "B/four", "x"]);
    projected.workspaces[5].custom_label = false;
    let mut state = grouped_state(projected);

    // Drawn B, plain, A, C, x: the menu follows the drawing, not the names.
    state.compose(106, 30).expect("composed frame");
    let point = space_point(&state, "ws_2");
    right_click(&mut state, point);
    let first = state.compose(106, 30).expect("first menu frame");
    let first_row = state.hits.context_menu_rows[0].0;
    assert!(frame_rows(&first)
        .iter()
        .any(|line| line.contains("Move to group")));
    assert!(choose(&mut state, "Move to group").actions.is_empty());
    assert_eq!(open_menu(&state).1, ["B", "A", "C", "New group..."]);
    assert!(matches!(
        open_menu(&state).0,
        ClientContextMenuTarget::MoveToGroup { workspace_id, groups, in_group: false }
            if workspace_id == "ws_2" && groups == &["B", "A", "C"]
    ));

    // Drawn where the first menu was, its labels (group names) on its rows.
    let frame = state.compose(106, 30).expect("second menu frame");
    let rows = state.hits.context_menu_rows.clone();
    assert_eq!((rows[0].0.x, rows[0].0.y), (first_row.x, first_row.y));
    let lines = frame_rows(&frame);
    for ((row, _), label) in rows.iter().zip(["B", "A", "C", "New group..."]) {
        let line = lines[usize::from(row.y)]
            .chars()
            .skip(usize::from(row.x))
            .take(usize::from(row.width))
            .collect::<String>();
        assert_eq!(line.trim_end(), label);
    }
    key(&mut state, KeyCode::Esc);
    assert!(state.overlay.is_none());

    // A member: its own group is left out and "No group" is offered.
    open_move_menu(&mut state, "ws_3");
    assert_eq!(open_menu(&state).1, ["B", "C", "New group...", "No group"]);
    key(&mut state, KeyCode::Esc);
    open_move_menu(&mut state, "ws_5");
    assert_eq!(open_menu(&state).1, ["A", "C", "New group...", "No group"]);
    key(&mut state, KeyCode::Esc);
    open_move_menu(&mut state, "ws_6");
    assert_eq!(open_menu(&state).1, ["B", "A", "C", "New group..."]);
    key(&mut state, KeyCode::Esc);

    // Without groups, only "New group...".
    let mut state = grouped_state(named_spaces(&["one", "two"]));
    open_move_menu(&mut state, "ws_2");
    assert_eq!(open_menu(&state).1, ["New group..."]);
}

#[test]
fn picking_a_group_renames_the_space_and_moves_it_after_the_groups_last_member() {
    let mut projected = named_spaces(&["A/one", "plain", "B/two", "A/three", "other", "C/x"]);
    projected.workspaces[4].label = "auto".into();
    projected.workspaces[4].custom_label = false;
    let mut state = grouped_state(projected);

    // A in the middle of the server order, its spaces not adjacent: first `three` is gathered
    // behind `one`, then the space goes after it.
    assert_eq!(
        move_to(&mut state, "ws_2", "A"),
        [
            "rename ws_2 A/plain",
            "move ws_4 before ws_3",
            "move ws_2 before ws_3"
        ]
    );
    // C at the end.
    assert_eq!(
        move_to(&mut state, "ws_2", "C"),
        ["rename ws_2 C/plain", "move ws_2 before end"]
    );
    // Right after A's last member, but A is gathered first, so the space follows it. An unnamed
    // space keeps its automatic label.
    assert_eq!(
        move_to(&mut state, "ws_5", "A"),
        [
            "rename ws_5 A/auto",
            "move ws_4 before ws_2",
            "move ws_5 before ws_2"
        ]
    );
    // From another group: its short name.
    assert_eq!(
        move_to(&mut state, "ws_6", "B"),
        ["rename ws_6 B/x", "move ws_6 before ws_4"]
    );
}

#[test]
fn a_worktree_parent_moves_to_a_group_with_its_whole_block() {
    let mut projected = named_spaces(&["A/one", "Web/p", "plain", "c1", "A/two", "c2"]);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 4, true);
    in_worktree(&mut projected, 6, true);
    let mut state = grouped_state(projected);
    assert_eq!(
        move_to(&mut state, "ws_2", "A"),
        [
            "rename ws_2 A/p",
            "move ws_5 before ws_3",
            "move ws_2,ws_4,ws_6 before ws_3"
        ]
    );

    // A group whose last space is a linked child of its worktree block: after that child (gathered
    // behind its parent first).
    let mut projected = named_spaces(&["G/p", "x", "plain", "c1", "y"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 4, true);
    let mut state = grouped_state(projected);
    assert_eq!(
        move_to(&mut state, "ws_2", "G"),
        [
            "rename ws_2 G/x",
            "move ws_4 before ws_3",
            "move ws_2 before ws_3"
        ]
    );
}

#[test]
fn joining_groups_keeps_the_server_order_the_same_as_the_drawing() {
    let mut projected = named_spaces(&["A/one", "A/two", "plain", "B/three", "Web/p", "c1"]);
    in_worktree(&mut projected, 5, false);
    in_worktree(&mut projected, 6, true);
    let mut state = grouped_state(projected);
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, server);

    for (workspace_id, group) in [("ws_5", "A"), ("ws_3", "B"), ("ws_2", "B"), ("ws_4", "A")] {
        open_move_menu(&mut state, workspace_id);
        let methods = sent_methods(&choose(&mut state, group).actions);
        assert!(!methods.is_empty());
        apply_sent(&mut state, &methods);
        let (drawn, server) = drawn_and_server_order(&state);
        assert_eq!(drawn, server, "{workspace_id} to {group}");
    }
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 9)
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} p"),
            format!("     └─ {icon} c1"),
            format!("   {icon} three"),
            " ▾ B".to_owned(),
            format!("   {icon} plain"),
            format!("   {icon} two"),
            String::new(),
        ]
    );
}

#[test]
fn no_group_renames_the_space_to_its_short_name_and_leaves_it_in_place() {
    let mut state = grouped_state(named_spaces(&["A/one", "plain", "A / two"]));
    assert_eq!(move_to(&mut state, "ws_3", "No group"), ["rename ws_3 two"]);
    // The group's first space leaves: the group's other spaces are gathered behind it, so the
    // group stays drawn right below it instead of jumping past `plain`.
    open_move_menu(&mut state, "ws_1");
    let outcome = choose(&mut state, "No group");
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_1 one", "move ws_3 before ws_2"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_1", "ws_3", "ws_2"]);
    assert_eq!(drawn, server);
}

#[test]
fn new_group_opens_the_rename_box_with_the_cursor_before_the_short_name() {
    let mut projected = named_spaces(&["A/one", "B/two", "plain", "x"]);
    projected.workspaces[3].label = "auto".into();
    projected.workspaces[3].custom_label = false;
    let mut state = grouped_state(projected);

    for (workspace_id, prefill) in [("ws_1", "/one"), ("ws_3", "/plain"), ("ws_4", "/auto")] {
        open_move_menu(&mut state, workspace_id);
        assert!(choose(&mut state, "New group...").actions.is_empty());
        assert!(
            matches!(
                state.overlay,
                Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename workspace",
                    ref input,
                    target: ClientRenameTarget::SpaceNewGroup { workspace_id: ref id },
                })) if input.as_str() == prefill && id == workspace_id
            ),
            "{workspace_id}"
        );
        key(&mut state, KeyCode::Esc);
    }

    // Typed at the start; a new group sends only the rename.
    open_move_menu(&mut state, "ws_1");
    choose(&mut state, "New group...");
    assert!(state.handle_input_bytes(b"X").actions.is_empty());
    let save = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none());
    assert_eq!(sent(&save.actions), ["rename ws_1 X/one"]);

    // An existing group's name moves it there, as the rename box does.
    open_move_menu(&mut state, "ws_3");
    choose(&mut state, "New group...");
    state.handle_input_bytes(b"A");
    let save = state.handle_input_bytes(b"\r");
    assert_eq!(
        sent(&save.actions),
        ["rename ws_3 A/plain", "move ws_3 before ws_2"]
    );
}

/// Opens `workspace_id`'s rename box from its menu, replaces the text with `text` and presses
/// Enter; what that sent.
fn rename_space(state: &mut ClientShellState, workspace_id: &str, text: &str) -> Vec<String> {
    state.compose(106, 30).expect("composed frame");
    let point = space_point(state, workspace_id);
    right_click(state, point);
    choose(state, "Rename");
    state.handle_input_bytes(&[0x15]);
    state.handle_input_bytes(text.as_bytes());
    let save = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none(), "Enter closes the rename box");
    sent(&save.actions)
}

#[test]
fn renaming_a_space_into_an_existing_group_moves_it_after_the_groups_last_member() {
    let mut projected = named_spaces(&["A/one", "plain", "B/two", "A/three", "other", "Web/p"]);
    in_worktree(&mut projected, 6, false);
    let mut child = space(7, "c1", true);
    child.worktree = Some(ClientShellWorktree {
        key: "/repo/.git".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    projected.workspaces.push(child);
    let mut state = grouped_state(projected);

    // A's spaces aren't adjacent: `three` is gathered behind `one` first.
    assert_eq!(
        rename_space(&mut state, "ws_2", " A / x "),
        [
            "rename ws_2 A / x",
            "move ws_4 before ws_3",
            "move ws_2 before ws_3"
        ]
    );
    // A new group, the same group, leaving one, not grouped: only the rename.
    for (workspace_id, text) in [
        ("ws_2", "Z/x"),
        ("ws_3", "B/renamed"),
        ("ws_3", "two"),
        ("ws_2", "A/"),
    ] {
        assert_eq!(
            rename_space(&mut state, workspace_id, text),
            [format!("rename {workspace_id} {text}")],
            "{workspace_id} {text:?}"
        );
    }
    // Leaving A as its first space (into another case's group): A's other space is gathered
    // behind it, so A doesn't jump.
    assert_eq!(
        rename_space(&mut state, "ws_1", "a/one"),
        ["rename ws_1 a/one", "move ws_4 before ws_2"]
    );
    // Or for no group: the same.
    assert_eq!(
        rename_space(&mut state, "ws_1", "one"),
        ["rename ws_1 one", "move ws_4 before ws_2"]
    );
    // From another group, and a worktree parent with its block.
    assert_eq!(
        rename_space(&mut state, "ws_3", "A/two"),
        [
            "rename ws_3 A/two",
            "move ws_4 before ws_2",
            "move ws_3 before ws_2"
        ]
    );
    assert_eq!(
        rename_space(&mut state, "ws_6", "B/p"),
        ["rename ws_6 B/p", "move ws_6,ws_7 before ws_4"]
    );
    // A linked child drawn under its parent stays where its parent is.
    assert_eq!(
        rename_space(&mut state, "ws_7", "A/c1"),
        ["rename ws_7 A/c1"]
    );
}

#[test]
fn the_move_to_group_menu_works_with_the_keys() {
    let mut state = grouped_state(named_spaces(&["A/one", "B/two", "other", "plain"]));
    state.compose(106, 30).expect("composed frame");
    let point = space_point(&state, "ws_4");
    right_click(&mut state, point);
    key(&mut state, KeyCode::Down);
    assert!(key(&mut state, KeyCode::Enter).actions.is_empty());
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("the second menu is open");
    };
    assert!(matches!(
        menu.target,
        ClientContextMenuTarget::MoveToGroup { .. }
    ));
    assert_eq!(menu.highlighted, 0);
    assert_eq!(open_menu(&state).1, ["A", "B", "New group..."]);

    key(&mut state, KeyCode::Down);
    key(&mut state, KeyCode::Down);
    key(&mut state, KeyCode::Down);
    key(&mut state, KeyCode::Up);
    let pick = key(&mut state, KeyCode::Enter);
    assert!(state.overlay.is_none());
    assert_eq!(
        sent(&pick.actions),
        ["rename ws_4 B/plain", "move ws_4 before ws_3"]
    );

    // Esc closes it without sending anything.
    open_move_menu(&mut state, "ws_4");
    let esc = key(&mut state, KeyCode::Esc);
    assert!(state.overlay.is_none() && esc.actions.is_empty());
}

#[test]
fn a_refused_rename_sends_no_move() {
    let mut state = grouped_state(named_spaces(&["A/one", "plain", "other"]));

    // The machine drops while the menu is open: nothing goes.
    open_move_menu(&mut state, "ws_3");
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Reconnecting);
    assert_eq!(sent(&choose(&mut state, "A").actions), Vec::<String>::new());
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);

    // A server that refuses the rename gets no move either, though it could move the space.
    state.set_endpoint_methods(Some(vec!["workspace.move_block".into()]));
    assert_eq!(move_to(&mut state, "ws_3", "A"), Vec::<String>::new());
    state.set_endpoint_methods(None);
    assert_eq!(
        move_to(&mut state, "ws_3", "A"),
        ["rename ws_3 A/other", "move ws_3 before ws_2"]
    );
}

#[test]
fn move_to_group_plans_need_no_window() {
    use crate::client::shell::space_groups::{join_group_methods, move_menu_items, rename_moves};

    let plan = |projected: &ClientShellSnapshot, workspace_id: &str, group: &str| {
        join_group_methods(projected, workspace_id, group)
            .iter()
            .map(described)
            .collect::<Vec<_>>()
    };
    // A's spaces aren't adjacent in server order: joining A gathers them behind `one` first
    // (`three` before `two`), so A's last space is the one drawn last.
    let projected = named_spaces(&["A/one", "plain", "B/two", "A/three", "other"]);
    assert_eq!(
        plan(&projected, "ws_2", "A"),
        [
            "rename ws_2 A/plain",
            "move ws_4 before ws_3",
            "move ws_2 before ws_3"
        ]
    );
    assert_eq!(
        plan(&projected, "ws_2", "B"),
        ["rename ws_2 B/plain", "move ws_2 before ws_4"]
    );
    assert_eq!(
        plan(&projected, "ws_5", "A"),
        [
            "rename ws_5 A/other",
            "move ws_4 before ws_2",
            "move ws_5 before ws_2"
        ]
    );
    // A group nothing is in yet: only the rename. A space that is gone: nothing.
    assert_eq!(plan(&projected, "ws_2", "New"), ["rename ws_2 New/plain"]);
    assert_eq!(plan(&projected, "ws_9", "A"), Vec::<String>::new());

    let moved = |workspace_id: &str, label: &str| {
        rename_moves(&projected, workspace_id, label)
            .iter()
            .map(described)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        moved("ws_2", "A/x"),
        ["move ws_4 before ws_3", "move ws_2 before ws_3"]
    );
    for label in ["x", "New/x", "a/x", "A/", "/x"] {
        assert_eq!(moved("ws_2", label), Vec::<String>::new(), "{label:?}");
    }
    assert_eq!(moved("ws_4", "A/y"), Vec::<String>::new());

    // A second parent checkout goes where its block's head goes: never moved on its own.
    let mut projected = named_spaces(&["A/one", "Web/p", "q", "c1"]);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, false);
    in_worktree(&mut projected, 4, true);
    assert_eq!(plan(&projected, "ws_3", "A"), ["rename ws_3 A/q"]);
    assert!(rename_moves(&projected, "ws_3", "A/q").is_empty());
    // The block's head right after the group already: no move.
    assert_eq!(plan(&projected, "ws_2", "A"), ["rename ws_2 A/p"]);
    // A linked child under its parent: renamed, never moved on its own.
    assert_eq!(plan(&projected, "ws_4", "A"), ["rename ws_4 A/c1"]);

    let items = |groups: &[String], in_group| {
        move_menu_items(groups, in_group)
            .into_iter()
            .map(|item| (item.label.into_owned(), item.action))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        items(&["B".to_owned(), "A".to_owned()], true),
        [
            ("B".to_owned(), ClientContextMenuAction::JoinGroup(0)),
            ("A".to_owned(), ClientContextMenuAction::JoinGroup(1)),
            ("New group...".to_owned(), ClientContextMenuAction::NewGroup),
            ("No group".to_owned(), ClientContextMenuAction::LeaveGroup),
        ]
    );
    assert_eq!(
        items(&[], false),
        [("New group...".to_owned(), ClientContextMenuAction::NewGroup)]
    );
}

/// Opens the "New group..." box of `workspace_id`, types `keys` and presses Enter; what that sent.
fn new_group(state: &mut ClientShellState, workspace_id: &str, keys: &[u8]) -> Vec<String> {
    open_move_menu(state, workspace_id);
    assert!(choose(state, "New group...").actions.is_empty());
    if !keys.is_empty() {
        assert!(state.handle_input_bytes(keys).actions.is_empty());
    }
    let save = state.handle_input_bytes(b"\r");
    assert!(state.overlay.is_none(), "Enter closes the rename box");
    sent(&save.actions)
}

#[test]
fn new_group_sends_nothing_while_the_group_side_is_empty() {
    let mut state = grouped_state(named_spaces(&["A/one", "B/two", "plain"]));
    // Enter right away (`/one`), a space before the slash, only a slash (Ctrl+K clears the line
    // from the cursor at the start).
    for keys in [&b""[..], b" ", b"\x0b/", b"\x0b / x"] {
        assert_eq!(
            new_group(&mut state, "ws_1", keys),
            Vec::<String>::new(),
            "{keys:?}"
        );
    }
    // A name without a slash is a plain rename, an existing group's name moves the space.
    assert_eq!(
        new_group(&mut state, "ws_1", b"\x0bsolo"),
        ["rename ws_1 solo"]
    );
    assert_eq!(
        new_group(&mut state, "ws_3", b"A"),
        ["rename ws_3 A/plain", "move ws_3 before ws_2"]
    );
    // The space's plain rename box still sends a name that starts with a slash.
    assert_eq!(rename_space(&mut state, "ws_1", "/x"), ["rename ws_1 /x"]);
}

#[test]
fn a_refused_rename_box_rename_sends_no_move() {
    let mut state = grouped_state(named_spaces(&["A/one", "plain", "other"]));
    state.set_endpoint_methods(Some(vec!["workspace.move_block".into()]));
    assert_eq!(
        rename_space(&mut state, "ws_3", "A/x"),
        Vec::<String>::new()
    );
    assert_eq!(new_group(&mut state, "ws_3", b"A"), Vec::<String>::new());
    state.set_endpoint_methods(None);

    state.compose(106, 30).expect("composed frame");
    let point = space_point(&state, "ws_3");
    right_click(&mut state, point);
    choose(&mut state, "Rename");
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Reconnecting);
    state.handle_input_bytes(&[0x15]);
    state.handle_input_bytes(b"A/x");
    assert!(state.handle_input_bytes(b"\r").actions.is_empty());
    state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);

    assert_eq!(
        rename_space(&mut state, "ws_3", "A/x"),
        ["rename ws_3 A/x", "move ws_3 before ws_2"]
    );
}

#[test]
fn no_group_lets_the_user_name_a_space_whose_short_name_has_a_slash() {
    let mut state = grouped_state(named_spaces(&["Web/app/x", "Web/two", "app/y", "plain"]));
    open_move_menu(&mut state, "ws_1");
    assert!(choose(&mut state, "No group").actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename workspace",
            ref input,
            target: ClientRenameTarget::Workspace { ref workspace_id },
        })) if input.as_str() == "app/x" && workspace_id == "ws_1"
    ));
    // The cursor is at the end; the box saves as the space's rename box does.
    assert!(state.handle_input_bytes(b"z").actions.is_empty());
    let save = state.handle_input_bytes(b"\r");
    assert_eq!(
        sent(&save.actions),
        ["rename ws_1 app/xz", "move ws_1 before ws_4"]
    );

    open_move_menu(&mut state, "ws_1");
    choose(&mut state, "No group");
    state.handle_input_bytes(&[0x15]);
    state.handle_input_bytes(b"x");
    assert_eq!(
        sent(&state.handle_input_bytes(b"\r").actions),
        ["rename ws_1 x"]
    );
    // A short name without a slash is renamed right away.
    assert_eq!(move_to(&mut state, "ws_2", "No group"), ["rename ws_2 two"]);
}

#[test]
fn a_second_parent_checkout_goes_where_its_block_head_goes() {
    let mut projected = named_spaces(&["Web/p", "q", "c1", "Web/other", "A/one"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(projected);

    // Only the block's head offers "Move to group".
    assert_eq!(
        space_menu(&mut state, "ws_1")[..2],
        ["Rename", "Move to group"]
    );
    for workspace_id in ["ws_2", "ws_3"] {
        let labels = space_menu(&mut state, workspace_id);
        assert!(
            !labels.iter().any(|label| label == "Move to group"),
            "{workspace_id}: {labels:?}"
        );
    }
    // The head takes the whole block.
    assert_eq!(
        move_to(&mut state, "ws_1", "A"),
        ["rename ws_1 A/p", "move ws_1,ws_2,ws_3 before end"]
    );

    // Renamed into A in the rename box, it stays in its block under Web: nothing moves.
    let (drawn_before, _) = drawn_and_server_order(&state);
    assert_eq!(rename_space(&mut state, "ws_2", "A/q"), ["rename ws_2 A/q"]);
    apply_renames(&mut state, &renamed(&[("ws_2", "A/q")]));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, drawn_before);
    assert_eq!(drawn, server);
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 7)
            .iter()
            .map(|line| without_toggle(line))
            .collect::<Vec<_>>(),
        [
            " ▾ Web".to_owned(),
            format!("   {icon} p"),
            format!("   {icon} A/q"),
            format!("     └─ {icon} c1"),
            format!("   {icon} other"),
            " ▾ A".to_owned(),
            format!("   {icon} one"),
        ]
    );
}

/// The first line of the space list in a new frame; the other rows' lines count from it.
fn list_top(state: &mut ClientShellState) -> u16 {
    state.compose(106, 30).expect("composed frame");
    state.hits.workspace_body.y
}

/// Presses `from` and drags to `to`; the drag's outcome.
fn drag_to(state: &mut ClientShellState, from: (u16, u16), to: (u16, u16)) -> ClientShellInput {
    mouse(state, MouseEventKind::Down(MouseButton::Left), from);
    mouse(state, MouseEventKind::Drag(MouseButton::Left), to)
}

/// Drags `workspace_id`'s row (in a new frame) to line `row`, a column to the right.
fn drag_space(state: &mut ClientShellState, workspace_id: &str, row: u16) -> (u16, u16) {
    state.compose(106, 30).expect("composed frame");
    let from = space_point(state, workspace_id);
    let to = (from.0 + 1, row);
    drag_to(state, from, to);
    to
}

/// Drags `group`'s header (in a new frame) to line `row`, a column to the right.
fn drag_header(state: &mut ClientShellState, group: &str, row: u16) -> (u16, u16) {
    state.compose(106, 30).expect("composed frame");
    let from = header_point(state, group);
    let to = (from.0 + 1, row);
    drag_to(state, from, to);
    to
}

fn release(state: &mut ClientShellState, point: (u16, u16)) -> ClientShellInput {
    mouse(state, MouseEventKind::Up(MouseButton::Left), point)
}

/// Drags `workspace_id` to line `row` and drops it there; what that sent.
fn drop_space(state: &mut ClientShellState, workspace_id: &str, row: u16) -> Vec<String> {
    let point = drag_space(state, workspace_id, row);
    sent(&release(state, point).actions)
}

/// Drags `group`'s header to line `row` and drops it there; what that sent.
fn drop_header(state: &mut ClientShellState, group: &str, row: u16) -> Vec<String> {
    let point = drag_header(state, group, row);
    let sent = sent(&release(state, point).actions);
    assert!(state.group_header_press.is_none());
    sent
}

/// The target of the group drag under way.
fn group_target(state: &ClientShellState) -> Option<&GroupDropTarget> {
    let Some(ClientChromeDrag::SpaceGroups(drag)) = state.chrome_drag.as_ref() else {
        panic!("a group drag is under way");
    };
    drag.target.as_ref()
}

fn gap(group: Option<&str>, before: Option<&str>, row: u16, beside: bool) -> GroupDropTarget {
    GroupDropTarget::Gap {
        group: group.map(str::to_owned),
        before: before.map(str::to_owned),
        row,
        beside,
    }
}

/// The sidebar's space list text on line `row` of a new frame.
fn list_line(state: &mut ClientShellState, row: u16) -> String {
    let frame = state.compose(106, 30).expect("composed frame");
    let body = state.hits.workspace_body;
    frame_rows(&frame)[usize::from(row)]
        .chars()
        .skip(usize::from(body.x))
        .take(usize::from(body.width))
        .collect()
}

/// `A/one, A/two, plain, B/three, B/four, other`, drawn one line each from `top`:
/// ` ▾ A` top, one +1, two +2, plain +3, ` ▾ B` +4, three +5, four +6, other +7.
fn two_groups() -> (ClientShellState, u16) {
    let mut state = grouped_state(named_spaces(&[
        "A/one", "A/two", "plain", "B/three", "B/four", "other",
    ]));
    let top = list_top(&mut state);
    (state, top)
}

#[test]
fn dropping_a_space_on_a_header_joins_the_group_at_its_end() {
    let (mut state, top) = two_groups();
    drag_space(&mut state, "ws_3", top + 4);
    assert_eq!(
        group_target(&state),
        Some(&GroupDropTarget::Header("B".into()))
    );
    assert_eq!(
        drop_space(&mut state, "ws_3", top + 4),
        ["rename ws_3 B/plain", "move ws_3 before ws_6"]
    );
    assert!(state.chrome_drag.is_none());

    // A folded group's header too.
    click_header(&mut state, "B");
    assert!(state.collapsed_groups.contains("group:B"));
    assert_eq!(header_point(&state, "B").1, top + 4);
    assert_eq!(
        drop_space(&mut state, "ws_3", top + 4),
        ["rename ws_3 B/plain", "move ws_3 before ws_6"]
    );
    // And from another group.
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 4),
        ["rename ws_1 B/one", "move ws_1 before ws_6"]
    );
}

#[test]
fn a_space_on_its_own_groups_header_goes_to_the_groups_first_place() {
    let (mut state, top) = two_groups();
    // On its own header a space goes to its group's first place, which for the first member is
    // no change.
    drag_space(&mut state, "ws_1", top);
    assert_eq!(
        group_target(&state),
        Some(&gap(Some("A"), Some("ws_1"), top, true))
    );
    assert_eq!(drop_space(&mut state, "ws_1", top), Vec::<String>::new());
    // For the second member it is a reorder to the top of the group.
    assert_eq!(
        drop_space(&mut state, "ws_2", top),
        ["move ws_2 before ws_1"]
    );
}

#[test]
fn dropping_a_space_between_a_groups_members_joins_it_there() {
    let (mut state, top) = two_groups();
    drag_space(&mut state, "ws_3", top + 5);
    assert_eq!(
        group_target(&state),
        Some(&gap(Some("B"), Some("ws_5"), top + 5, false))
    );
    let point = (space_point(&state, "ws_3").0 + 1, top + 5);
    let outcome = release(&mut state, point);
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_3 B/plain", "move ws_3 before ws_5"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_4", "ws_3", "ws_5", "ws_6"]);
    assert_eq!(drawn, server);
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 8),
        [
            " ▾ A".to_owned(),
            format!("   {icon} one"),
            format!("   {icon} two"),
            " ▾ B".to_owned(),
            format!("   {icon} three"),
            format!("   {icon} plain"),
            format!("   {icon} four"),
            format!(" {icon} other"),
        ]
    );
}

#[test]
fn dropping_a_space_between_a_header_and_its_first_member_joins_as_first() {
    // Two-line rows (a branch line), so the first member's top line is nearer that gap.
    let mut projected = named_spaces(&["plain", "A/one", "A/two", "B/three", "B/four"]);
    for workspace in &mut projected.workspaces {
        workspace.branch = Some("main".into());
    }
    let mut state = grouped_state(projected);
    state.compose(106, 30).expect("composed frame");
    let header = header_point(&state, "B").1;
    assert_eq!(row_of(&state, "ws_4"), header + 1);
    drag_space(&mut state, "ws_1", header + 1);
    assert_eq!(
        group_target(&state),
        Some(&gap(Some("B"), Some("ws_4"), header, false))
    );
    let point = (space_point(&state, "ws_1").0 + 1, header + 1);
    let outcome = release(&mut state, point);
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_1 B/plain", "move ws_1 before ws_4"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_2", "ws_3", "ws_1", "ws_4", "ws_5"]);
    assert_eq!(drawn, server);
}

fn row_of(state: &ClientShellState, workspace_id: &str) -> u16 {
    space_point(state, workspace_id).1
}

#[test]
fn dropping_a_member_inside_its_own_group_reorders_it_without_a_rename() {
    let mut state = grouped_state(named_spaces(&["A/one", "A/two", "A/three", "plain"]));
    let top = list_top(&mut state);
    // Between two and three.
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 2),
        ["move ws_1 before ws_3"]
    );
    // Last in the group is the top-level gap after it: that leaves the group.
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 3),
        ["rename ws_1 one", "move ws_1 before ws_4"]
    );
}

#[test]
fn dropping_a_grouped_space_between_top_level_rows_leaves_its_group() {
    let (mut state, top) = two_groups();
    // Before the group's own header.
    assert_eq!(
        drop_space(&mut state, "ws_2", top - 1),
        ["rename ws_2 two", "move ws_2 before ws_1"]
    );
    // Right after the group's last member: the server order stays, only the name changes.
    assert_eq!(drop_space(&mut state, "ws_2", top + 2), ["rename ws_2 two"]);
    // Right after another group (before an ungrouped row), and before another group's header.
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 6),
        ["rename ws_1 one", "move ws_1 before ws_6"]
    );
    let point = drag_space(&mut state, "ws_5", top + 3);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_4"), top + 3, false))
    );
    let outcome = release(&mut state, point);
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_5 four", "move ws_5 before ws_4"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_1", "ws_2", "ws_3", "ws_5", "ws_4", "ws_6"]);
    assert_eq!(drawn, server);
}

#[test]
fn dropping_a_space_at_the_end_puts_it_last_at_the_top_level() {
    let (mut state, top) = two_groups();
    drag_space(&mut state, "ws_1", top + 8);
    assert_eq!(group_target(&state), Some(&gap(None, None, top + 8, false)));
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 8),
        ["rename ws_1 one", "move ws_1 before end"]
    );
    // An ungrouped space only moves.
    assert_eq!(
        drop_space(&mut state, "ws_3", top + 8),
        ["move ws_3 before end"]
    );
    // Already last: nothing.
    assert_eq!(
        drop_space(&mut state, "ws_6", top + 8),
        Vec::<String>::new()
    );
}

#[test]
fn an_ungrouped_space_between_top_level_rows_only_moves() {
    let (mut state, top) = two_groups();
    assert_eq!(
        drop_space(&mut state, "ws_6", top + 2),
        ["move ws_6 before ws_3"]
    );
    assert_eq!(
        drop_space(&mut state, "ws_6", top - 1),
        ["move ws_6 before ws_1"]
    );
    assert_eq!(
        drop_space(&mut state, "ws_3", top + 6),
        ["move ws_3 before ws_6"]
    );
}

#[test]
fn a_leaving_space_whose_short_name_has_a_slash_moves_and_asks_for_its_name() {
    let mut state = grouped_state(named_spaces(&["Web/app/x", "Web/two", "plain"]));
    let top = list_top(&mut state);
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 4),
        ["move ws_1 before end"]
    );
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename workspace",
            ref input,
            target: ClientRenameTarget::Workspace { ref workspace_id },
        })) if input.as_str() == "app/x" && workspace_id == "ws_1"
    ));
    key(&mut state, KeyCode::Esc);
    // Joining another group keeps the whole short name.
    let mut state = grouped_state(named_spaces(&["Web/app/x", "B/two"]));
    let top = list_top(&mut state);
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 2),
        ["rename ws_1 B/app/x", "move ws_1 before end"]
    );
    assert!(state.overlay.is_none());
}

/// `A/one, A/two, p, c, plain`: `p` a worktree parent, `c` its linked child. Drawn ` ▾ A` top,
/// one +1, two +2, p +3, c +4, plain +5.
fn group_and_worktree() -> (ClientShellState, u16) {
    let mut projected = named_spaces(&["A/one", "A/two", "p", "c", "plain"]);
    in_worktree(&mut projected, 3, false);
    in_worktree(&mut projected, 4, true);
    let mut state = grouped_state(projected);
    let top = list_top(&mut state);
    (state, top)
}

#[test]
fn a_worktree_parent_dragged_into_a_group_carries_its_children() {
    let (mut state, top) = group_and_worktree();
    assert_eq!(row_of(&state, "ws_4"), top + 4);
    let point = drag_space(&mut state, "ws_3", top + 1);
    let outcome = release(&mut state, point);
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_3 A/p", "move ws_3,ws_4 before ws_2"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_1", "ws_3", "ws_4", "ws_2", "ws_5"]);
    assert_eq!(drawn, server);

    // No gap inside the block: on the parent's line the nearest gaps are around the block.
    let (mut state, top) = group_and_worktree();
    drag_space(&mut state, "ws_5", top + 3);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_3"), top + 2, false))
    );
}

#[test]
fn linked_children_and_second_parents_are_not_dragged() {
    let (mut state, top) = group_and_worktree();
    let point = drag_space(&mut state, "ws_4", top);
    assert!(state.chrome_drag.is_none());
    // The release is a click on the child, as upstream: it focuses it.
    let release = release(&mut state, point);
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_4"
            )
    ));

    // A second parent checkout of the block: its head places it.
    let mut projected = named_spaces(&["p", "q", "c", "A/one"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(projected);
    let top = list_top(&mut state);
    assert_eq!(header_point(&state, "A").1, top + 3);
    drag_space(&mut state, "ws_2", top + 3);
    assert!(state.chrome_drag.is_none());
    // The head moves with the whole block.
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 3),
        ["rename ws_1 A/p", "move ws_1,ws_2,ws_3 before end"]
    );
}

#[test]
fn a_before_inside_the_dragged_block_gives_way_to_the_next_space() {
    // The child comes first in server order; the block is drawn at its place.
    let mut projected = named_spaces(&["c", "A/one", "Web/p"]);
    in_worktree(&mut projected, 1, true);
    in_worktree(&mut projected, 3, false);
    let mut state = grouped_state(projected);
    let top = list_top(&mut state);
    assert_eq!(header_point(&state, "Web").1, top);
    // Before its own header: the group's first space is the child, which moves with it.
    let point = drag_space(&mut state, "ws_3", top - 1);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_1"), top - 1, false))
    );
    let outcome = release(&mut state, point);
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_3 p", "move ws_3,ws_1 before ws_2"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_3", "ws_1", "ws_2"]);
    assert_eq!(drawn, server);
}

/// `A/one, plain, A/p, c, B/x`: `p` a worktree parent, `c` its linked child. Drawn ` ▾ A` top,
/// one +1, p +2, c +3, plain +4, ` ▾ B` +5, x +6.
fn group_with_worktree() -> (ClientShellState, u16) {
    let mut projected = named_spaces(&["A/one", "plain", "A/p", "c", "B/x"]);
    in_worktree(&mut projected, 3, false);
    in_worktree(&mut projected, 4, true);
    let mut state = grouped_state(projected);
    let top = list_top(&mut state);
    (state, top)
}

#[test]
fn dragging_a_header_moves_its_whole_group() {
    let (mut state, top) = group_with_worktree();
    assert_eq!(header_point(&state, "B").1, top + 5);
    let point = drag_header(&mut state, "A", top + 7);
    assert_eq!(group_target(&state), Some(&gap(None, None, top + 7, false)));
    let outcome = release(&mut state, point);
    assert_eq!(sent(&outcome.actions), ["move ws_1,ws_3,ws_4 before end"]);
    assert!(
        state.collapsed_groups.is_empty(),
        "the release doesn't fold"
    );
    assert!(state.group_header_press.is_none() && state.chrome_drag.is_none());
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_2", "ws_5", "ws_1", "ws_3", "ws_4"]);
    assert_eq!(drawn, server);

    // Between top-level rows: before B.
    let (mut state, top) = group_with_worktree();
    assert_eq!(
        drop_header(&mut state, "A", top + 4),
        ["move ws_1,ws_3,ws_4 before ws_5"]
    );
    // B to the top.
    assert_eq!(
        drop_header(&mut state, "B", top - 1),
        ["move ws_5 before ws_1"]
    );
    assert!(state.collapsed_groups.is_empty());
}

#[test]
fn a_header_drops_nowhere_on_a_header_or_inside_a_group() {
    let (mut state, top) = group_with_worktree();
    // B's header, inside A, A's own header.
    for row in [top + 5, top + 1, top] {
        let point = drag_header(&mut state, "A", row);
        assert_eq!(group_target(&state), None, "line {row}");
        assert!(release(&mut state, point).actions.is_empty());
        assert!(state.collapsed_groups.is_empty());
    }
    // A folded group's header too, and its drag starts on the header's own line.
    click_header(&mut state, "B");
    let header = header_point(&state, "A");
    drag_to(&mut state, header, (header.0 + 1, header.1));
    assert_eq!(group_target(&state), None);
    mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        (header.0, top + 5),
    );
    assert_eq!(group_target(&state), None);
    assert!(release(&mut state, (header.0, top + 5)).actions.is_empty());
    assert_eq!(
        state.collapsed_groups,
        HashSet::from(["group:B".to_owned()])
    );

    // Over the middle member of a three-member group.
    let mut state = grouped_state(named_spaces(&["A/a", "B/one", "B/two", "B/three", "plain"]));
    let top = list_top(&mut state);
    assert_eq!(row_of(&state, "ws_3"), top + 4);
    let point = drag_header(&mut state, "A", top + 4);
    assert_eq!(group_target(&state), None);
    assert!(release(&mut state, point).actions.is_empty());
}

#[test]
fn drops_that_change_nothing_send_nothing() {
    let (mut state, top) = two_groups();
    // Right before and right after a member, in its group.
    for (workspace_id, row) in [("ws_1", top), ("ws_1", top + 1), ("ws_2", top + 1)] {
        assert_eq!(
            drop_space(&mut state, workspace_id, row),
            Vec::<String>::new(),
            "{workspace_id} at line {row}"
        );
    }
    // Right before and right after a top-level space.
    for row in [top + 2, top + 3] {
        assert_eq!(drop_space(&mut state, "ws_3", row), Vec::<String>::new());
    }
    // A header right before or right after its group.
    for row in [top - 1, top + 2] {
        assert_eq!(drop_header(&mut state, "A", row), Vec::<String>::new());
    }
    assert_eq!(drop_header(&mut state, "B", top + 6), Vec::<String>::new());
}

#[test]
fn the_drop_target_is_drawn_inside_a_group_across_the_list_or_on_a_header() {
    let (mut state, top) = two_groups();
    // Inside B: from the member indent.
    drag_space(&mut state, "ws_3", top + 5);
    let line = list_line(&mut state, top + 5);
    assert!(line.starts_with("  ──"), "{line:?}");
    assert!(!line.contains("three"));
    release(&mut state, (0, 0));

    // At the top level: the whole width, as upstream.
    drag_space(&mut state, "ws_1", top + 6);
    let line = list_line(&mut state, top + 6);
    assert!(line.starts_with("────"), "{line:?}");
    release(&mut state, (0, 0));

    // On a header: no line, the header highlighted, the dragged space as upstream.
    drag_space(&mut state, "ws_3", top + 4);
    let buffer = state
        .compose(106, 30)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    let palette = &state.config.palette;
    let highlight = if palette.selection_bg == ratatui::style::Color::Reset {
        palette.active_row_bg
    } else {
        palette.selection_bg
    };
    let header = state
        .hits
        .group_headers
        .iter()
        .find(|hit| hit.group == "B")
        .expect("header B")
        .rect;
    for x in header.x..header.right() {
        assert_eq!(buffer[(x, header.y)].bg, highlight, "column {x}");
    }
    let plain = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_3")
        .expect("dragged row")
        .rect;
    assert_eq!(buffer[(plain.x, plain.y)].bg, palette.surface1);
    let body = state.hits.workspace_body;
    for y in body.y.saturating_sub(1)..body.bottom() {
        for x in body.x..body.right() {
            assert_ne!(buffer[(x, y)].symbol(), "─", "cell {x},{y}");
        }
    }
    release(&mut state, (0, 0));
}

#[test]
fn a_dragged_header_gets_the_dragged_background() {
    let (mut state, top) = two_groups();
    drag_header(&mut state, "A", top + 3);
    let buffer = state
        .compose(106, 30)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer");
    let header = state
        .hits
        .group_headers
        .iter()
        .find(|hit| hit.group == "A")
        .expect("header A")
        .rect;
    for x in header.x..header.right() {
        assert_eq!(
            buffer[(x, header.y)].bg,
            state.config.palette.surface1,
            "column {x}"
        );
    }
    // Its line is the top-level one, before B.
    assert!(list_line(&mut state, top + 3).starts_with("────"));
}

#[test]
fn a_refused_drop_rename_sends_no_move() {
    let (mut state, top) = two_groups();
    state.set_endpoint_methods(Some(vec!["workspace.move_block".into()]));
    assert_eq!(
        drop_space(&mut state, "ws_3", top + 5),
        Vec::<String>::new()
    );
    // A move alone still goes.
    assert_eq!(
        drop_space(&mut state, "ws_6", top + 2),
        ["move ws_6 before ws_3"]
    );
}

#[test]
fn without_groups_the_drag_is_upstreams() {
    let mut state = grouped_state(named_spaces(&["one", "two", "three"]));
    let top = list_top(&mut state);
    drag_space(&mut state, "ws_1", top + 3);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::Workspace {
            ref source_workspace_id,
            target: Some((None, row)),
        }) if source_workspace_id == "ws_1" && row == top + 3
    ));
    let release = release(&mut state, (0, top + 3));
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceMove(params)
                    if params.workspace_id == "ws_1" && params.insert_index == 3
            )
    ));

    drag_space(&mut state, "ws_3", top);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::Workspace {
            target: Some((Some(ref before), row)),
            ..
        }) if before == "ws_2" && row == top
    ));
    let frame = state.compose(106, 30).expect("composed frame");
    let body = state.hits.workspace_body;
    assert!(frame_rows(&frame)[usize::from(top)]
        .chars()
        .skip(usize::from(body.x))
        .take(usize::from(body.width))
        .all(|symbol| symbol == '─'));
}

#[test]
fn a_folded_group_offers_its_header_but_no_gaps_between_members() {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/one", "A/two", "plain"]),
        "ws_2",
    ));
    let top = list_top(&mut state);
    click_header(&mut state, "A");
    let icon = icon(&state);
    assert_eq!(
        body_lines(&mut state, 3),
        [
            format!(" ▸ {icon} A"),
            format!("   {icon} two"),
            format!(" {icon} plain"),
        ]
    );
    // The shown member back on its own header: the group's first place (a reorder, no rename).
    drag_space(&mut state, "ws_2", top);
    assert_eq!(
        group_target(&state),
        Some(&gap(Some("A"), Some("ws_1"), top, false))
    );
    assert_eq!(
        sent(&release(&mut state, (0, 0)).actions),
        ["move ws_2 before ws_1"]
    );
    // Leaving the group is the line above the header.
    drag_space(&mut state, "ws_2", top - 1);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_1"), top - 1, false))
    );
    release(&mut state, (0, 0));
    // Another space on the folded header joins it.
    drag_space(&mut state, "ws_3", top);
    assert_eq!(
        group_target(&state),
        Some(&GroupDropTarget::Header("A".into()))
    );
    assert_eq!(
        sent(&release(&mut state, (0, 0)).actions),
        ["rename ws_3 A/plain"]
    );
}

#[test]
fn a_gap_before_a_worktree_block_goes_before_its_first_space_in_server_order() {
    // The child comes first in server order: the block is drawn at its place, and a space dropped
    // above the block's parent row goes before the child.
    let mut projected = named_spaces(&["c", "A/one", "p", "other"]);
    in_worktree(&mut projected, 1, true);
    in_worktree(&mut projected, 3, false);
    let mut state = grouped_state(projected);
    let top = list_top(&mut state);
    assert_eq!(row_of(&state, "ws_3"), top);
    let point = drag_space(&mut state, "ws_4", top - 1);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_1"), top - 1, false))
    );
    let outcome = release(&mut state, point);
    assert_eq!(sent(&outcome.actions), ["move ws_4 before ws_1"]);
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, _) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_4", "ws_3", "ws_1", "ws_2"]);
}

#[test]
fn a_grouped_space_dropped_between_two_ungrouped_rows_leaves_its_group_there() {
    let mut state = grouped_state(named_spaces(&["A/one", "A/two", "x", "y"]));
    let top = list_top(&mut state);
    let point = drag_space(&mut state, "ws_1", top + 3);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_4"), top + 3, false))
    );
    let outcome = release(&mut state, point);
    assert_eq!(
        sent(&outcome.actions),
        ["rename ws_1 one", "move ws_1 before ws_4"]
    );
    apply_sent(&mut state, &sent_methods(&outcome.actions));
    let (drawn, server) = drawn_and_server_order(&state);
    assert_eq!(drawn, ["ws_2", "ws_3", "ws_1", "ws_4"]);
    assert_eq!(drawn, server);
}

/// Drags `workspace_id` to line `row` and drops it; checks what it sent, then applies it and checks
/// the drawn order (which the server order then matches).
fn drop_and_apply(
    state: &mut ClientShellState,
    workspace_id: &str,
    row: u16,
    expected: &[&str],
    drawn: &[&str],
) {
    let point = drag_space(state, workspace_id, row);
    let outcome = release(state, point);
    assert_eq!(sent(&outcome.actions), expected);
    apply_sent(state, &sent_methods(&outcome.actions));
    let (now, server) = drawn_and_server_order(state);
    assert_eq!(now, drawn);
    assert_eq!(now, server);
}

#[test]
fn moving_the_first_space_of_a_scattered_group_gathers_the_group_where_it_is_drawn() {
    // `B/b1, x, B/a1, y` is drawn ` ▾ B` b1 a1, x, y: B sits at b1's place.
    let mut state = grouped_state(named_spaces(&["B/b1", "x", "B/a1", "y"]));
    let top = list_top(&mut state);
    // Right after itself: nothing, not even the gathering.
    assert_eq!(
        drop_space(&mut state, "ws_1", top + 1),
        Vec::<String>::new()
    );
    // b1 to just below the group: B stays where it is, b1 right below it.
    drop_and_apply(
        &mut state,
        "ws_1",
        top + 2,
        &[
            "rename ws_1 b1",
            "move ws_3 before ws_2",
            "move ws_1 before ws_2",
        ],
        &["ws_3", "ws_1", "ws_2", "ws_4"],
    );

    // A reorder inside the group.
    let mut state = grouped_state(named_spaces(&["B/b1", "x", "B/a1", "B/a2"]));
    let top = list_top(&mut state);
    drop_and_apply(
        &mut state,
        "ws_1",
        top + 2,
        &["move ws_3,ws_4 before ws_2", "move ws_1 before ws_4"],
        &["ws_3", "ws_1", "ws_4", "ws_2"],
    );

    // Into another group: by its header, and by "Move to group".
    let layout = ["B/b1", "x", "B/a1", "A/z"];
    let mut state = grouped_state(named_spaces(&layout));
    let top = list_top(&mut state);
    assert_eq!(header_point(&state, "A").1, top + 4);
    let joined = [
        "rename ws_1 A/b1",
        "move ws_3 before ws_2",
        "move ws_1 before end",
    ];
    drop_and_apply(
        &mut state,
        "ws_1",
        top + 4,
        &joined,
        &["ws_3", "ws_2", "ws_4", "ws_1"],
    );
    let mut state = grouped_state(named_spaces(&layout));
    assert_eq!(move_to(&mut state, "ws_1", "A"), joined);
}

#[test]
fn joining_a_scattered_group_gathers_it_first() {
    // `A/t1, x, A/t2, plain` is drawn ` ▾ A` t1 t2, x, plain.
    let mut state = grouped_state(named_spaces(&["A/t1", "x", "A/t2", "plain"]));
    let top = list_top(&mut state);
    drop_and_apply(
        &mut state,
        "ws_4",
        top + 1,
        &[
            "rename ws_4 A/plain",
            "move ws_3 before ws_2",
            "move ws_4 before ws_3",
        ],
        &["ws_1", "ws_4", "ws_3", "ws_2"],
    );
}

/// `A/a, B/b, C/c, D/d, plain` with `plain` focused and every group folded: drawn ` ▸ A` top,
/// ` ▸ B` +1, ` ▸ C` +2, ` ▸ D` +3, plain +4.
fn folded_groups() -> (ClientShellState, u16) {
    let mut state = grouped_state(focused_on(
        named_spaces(&["A/a", "B/b", "C/c", "D/d", "plain"]),
        "ws_5",
    ));
    for group in ["A", "B", "C", "D"] {
        state.compose(106, 30).expect("composed frame");
        click_header(&mut state, group);
    }
    let top = list_top(&mut state);
    assert_eq!(header_point(&state, "D").1, top + 3);
    (state, top)
}

#[test]
fn a_header_drops_between_folded_groups_on_their_header_lines() {
    let (mut state, top) = folded_groups();
    // On A's line: the gap after A, before B.
    drag_header(&mut state, "D", top);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_2"), top, false))
    );
    assert_eq!(drop_header(&mut state, "D", top), ["move ws_4 before ws_2"]);
    assert_eq!(
        drop_header(&mut state, "D", top + 1),
        ["move ws_4 before ws_3"]
    );
    // On C's line the gap right before D: nothing changes. On its own header: nowhere.
    assert_eq!(drop_header(&mut state, "D", top + 2), Vec::<String>::new());
    drag_header(&mut state, "D", top + 3);
    assert_eq!(group_target(&state), None);
    release(&mut state, (0, 0));
    // A space on a folded header joins it (the header keeps its line).
    drag_space(&mut state, "ws_5", top + 1);
    assert_eq!(
        group_target(&state),
        Some(&GroupDropTarget::Header("B".into()))
    );
    release(&mut state, (0, 0));
}

#[test]
fn a_drop_whose_next_space_has_gone_sends_nothing() {
    let (mut state, top) = two_groups();
    drag_space(&mut state, "ws_3", top + 5);
    assert_eq!(
        group_target(&state),
        Some(&gap(Some("B"), Some("ws_5"), top + 5, false))
    );
    // `four` closes before the release.
    let mut projected = (**state.snapshot.as_ref().expect("snapshot")).clone();
    projected.revision += 1;
    projected
        .workspaces
        .retain(|workspace| workspace.workspace_id != "ws_5");
    let mut projected_surface = surface();
    projected_surface.projection_revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(projected_surface);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::SpaceGroups(_))
    ));
    assert!(release(&mut state, (0, 0)).actions.is_empty());
}

#[test]
fn a_scrolled_list_offers_the_gap_before_the_next_row_below_the_last_row_shown() {
    // Two-line spaces under a one-line header leave a spare line at the bottom of an even body.
    let mut labels = vec!["G/g".to_owned()];
    labels.extend((1..=30).map(|number| format!("s{number}")));
    let mut projected = named_spaces(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    for workspace in &mut projected.workspaces {
        workspace.branch = Some("main".into());
    }
    let mut state = grouped_state(projected);
    // The first frame height with a spare line and a few spaces shown (`ws_2` not the last).
    let (height, number, spare) = (20..60)
        .find_map(|height| {
            state.compose(106, height).expect("composed frame");
            let last = state.hits.workspaces.last()?;
            let number = last
                .workspace_id
                .strip_prefix("ws_")
                .and_then(|number| number.parse::<usize>().ok())?;
            (number >= 4 && last.rect.bottom() < state.hits.new_workspace.y).then_some((
                height,
                number,
                last.rect.bottom(),
            ))
        })
        .expect("a frame with a spare line");
    assert!(number < 31, "rows are hidden below");
    let next = format!("ws_{}", number + 1);
    state.compose(106, height).expect("composed frame");
    let from = space_point(&state, "ws_2");
    drag_to(&mut state, from, (from.0 + 1, spare));
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some(&next), spare, false))
    );
    assert_eq!(
        sent(&release(&mut state, (0, spare)).actions),
        [format!("move ws_2 before {next}")]
    );
}

#[test]
fn a_slash_name_that_draws_no_header_keeps_upstreams_drag() {
    // A linked child named `feature/x` under an ungrouped parent: no header is drawn.
    let mut projected = named_spaces(&["p", "q", "feature/x", "other"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    let mut state = grouped_state(projected);
    let top = list_top(&mut state);
    assert!(state.hits.group_headers.is_empty());
    drag_space(&mut state, "ws_4", top - 1);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::Workspace { .. })
    ));
    release(&mut state, (0, 0));
    // A second parent checkout drags as upstream lets it.
    drag_space(&mut state, "ws_2", top + 3);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::Workspace { ref source_workspace_id, .. }) if source_workspace_id == "ws_2"
    ));
}

#[test]
fn a_folded_groups_shown_member_has_no_gap_before_it() {
    // Two-line rows: on the shown member's top line the nearest gaps would tie, one before the
    // member (inside the folded group, not offered) and one after it.
    let mut projected = focused_on(named_spaces(&["A/one", "A/two", "plain"]), "ws_2");
    for workspace in &mut projected.workspaces {
        workspace.branch = Some("main".into());
    }
    let mut state = grouped_state(projected);
    state.compose(106, 30).expect("composed frame");
    click_header(&mut state, "A");
    let top = list_top(&mut state);
    assert_eq!(row_of(&state, "ws_2"), top + 1);
    drag_space(&mut state, "ws_3", top + 1);
    assert_eq!(
        group_target(&state),
        Some(&gap(None, Some("ws_3"), top + 2, true))
    );
    assert!(release(&mut state, (0, 0)).actions.is_empty());
}
