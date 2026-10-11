use super::*;
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

    // Right-click on a header does nothing yet.
    let header = header_point(&state, "A");
    let right = mouse(&mut state, MouseEventKind::Down(MouseButton::Right), header);
    assert!(right.actions.is_empty() && state.overlay.is_none());
}

#[test]
fn a_header_press_that_drags_does_not_fold() {
    let mut state = grouped_state(named_spaces(&["A/one", "A/two", "plain"]));
    state.compose(106, 30).expect("composed frame");
    let header = header_point(&state, "A");
    let plain = space_point(&state, "ws_3");

    // Dragged onto a space and released there: no fold, no space press, no space drag.
    let mut actions = mouse(&mut state, MouseEventKind::Down(MouseButton::Left), header).actions;
    actions.extend(mouse(&mut state, MouseEventKind::Drag(MouseButton::Left), plain).actions);
    assert!(state.chrome_drag.is_none() && state.workspace_press.is_none());
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
}

#[test]
fn group_folds_are_saved_with_the_window_preferences() {
    let path = std::env::temp_dir().join(format!(
        "herdr-space-group-folds-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    std::fs::write(&path, r#"{"collapsed_groups":["/repo","group:A"]}"#)
        .expect("write preferences");
    let open = |path: &std::path::Path| {
        let config =
            ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.into());
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(focused_on(
            named_spaces(&["A/one", "plain"]),
            "ws_2",
        )));
        state.set_pane_surface(surface());
        state
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
