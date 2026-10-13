//! Fork: the red count of agents waiting for the user, on space rows, folded group headers and
//! the multi-machine sidebar.

use super::*;
use crate::client::endpoint::{ClientEndpointStatus, ProfileId, SavedSshEndpoint};

fn waiting_agent(workspace_id: &str, pane_id: &str, status: AgentStatus) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: workspace_id.into(),
        tab_id: format!("{workspace_id}_tab"),
        name: None,
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 2,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

/// `projected` with a finished agent nobody has looked at for each of `spaces`, in panes not on
/// screen (a space listed twice has two).
fn with_waiting(mut projected: ClientShellSnapshot, spaces: &[&str]) -> ClientShellSnapshot {
    projected.agents = spaces
        .iter()
        .enumerate()
        .map(|(index, space)| waiting_agent(space, &format!("waiting_{index}"), AgentStatus::Done))
        .collect();
    projected
}

/// `n` copies of `space`, for `with_waiting`.
fn times(space: &str, n: usize) -> impl Iterator<Item = &str> {
    std::iter::repeat_n(space, n)
}

fn render(state: &mut ClientShellState) -> Buffer {
    state
        .compose(106, 30)
        .expect("composed frame")
        .to_ratatui_buffer()
        .expect("buffer")
}

fn space_rect(state: &ClientShellState, workspace_id: &str) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .map(|hit| hit.rect)
        .expect("space row")
}

fn header_rect(state: &ClientShellState, group: &str) -> Rect {
    state
        .hits
        .group_headers
        .iter()
        .find(|hit| hit.group == group)
        .map(|hit| hit.rect)
        .expect("group header")
}

/// The red count on `row`'s first line: its number and the column right after it.
fn red_count(buffer: &Buffer, palette: &Palette, row: Rect) -> Option<(String, u16)> {
    let red = (row.x..row.right())
        .filter(|x| buffer[(*x, row.y)].bg == palette.red)
        .collect::<Vec<_>>();
    let (first, last) = (*red.first()?, *red.last()?);
    let text = (first..=last)
        .map(|x| buffer[(x, row.y)].symbol())
        .collect::<String>();
    Some((text.trim().to_owned(), last + 1))
}

fn space_count(state: &ClientShellState, buffer: &Buffer, workspace_id: &str) -> Option<String> {
    let row = space_rect(state, workspace_id);
    red_count(buffer, &state.config.palette, row).map(|(count, _)| count)
}

fn header_count(state: &ClientShellState, buffer: &Buffer, group: &str) -> Option<String> {
    let row = header_rect(state, group);
    red_count(buffer, &state.config.palette, row).map(|(count, _)| count)
}

fn count(n: usize) -> Option<String> {
    Some(n.to_string())
}

#[test]
fn a_space_shows_a_red_count_of_its_waiting_agents() {
    let mut state = grouped_state(snapshot());
    let mut asking = snapshot();
    asking.revision += 1;
    asking.agents = vec![waiting_agent("ws_1", "pane_9", AgentStatus::Blocked)];
    state.set_snapshot(Box::new(asking));
    state.set_pane_surface(PaneSurfaceFrame {
        projection_revision: 2,
        ..surface()
    });
    let buffer = render(&mut state);
    assert!(state.hits.machines.is_empty());
    let row = space_rect(&state, "ws_1");
    let palette = &state.config.palette;
    assert_eq!(
        red_count(&buffer, palette, row),
        Some(("1".to_owned(), row.right())),
        "at the row's right edge"
    );
    let digit = (row.x..row.right())
        .find(|x| buffer[(*x, row.y)].symbol() == "1")
        .expect("the count's digit");
    let cell = &buffer[(digit, row.y)];
    assert_eq!(cell.fg, crate::client::shell::panel_contrast_fg(palette));
    assert!(cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn a_new_window_counts_what_the_server_still_reports_unseen() {
    let new_window = |status| {
        let mut first = snapshot();
        first.agents = vec![waiting_agent("ws_1", "pane_9", status)];
        grouped_state(first)
    };
    for (status, expected, why) in [
        (
            AgentStatus::Done,
            count(1),
            "it finished while no window was looking",
        ),
        (AgentStatus::Blocked, count(1), "its question is still open"),
        (AgentStatus::Idle, None, "the server says it has been seen"),
        (AgentStatus::Working, None, "it is still working"),
    ] {
        let mut state = new_window(status);
        let buffer = render(&mut state);
        assert_eq!(space_count(&state, &buffer, "ws_1"), expected, "{why}");
    }

    // Both wait until their pane is on screen.
    for status in [AgentStatus::Done, AgentStatus::Blocked] {
        let mut state = new_window(status);
        let mut on_screen = surface();
        on_screen.surface_revision += 1;
        on_screen.panes[0].pane_id = "pane_9".into();
        state.set_pane_surface(on_screen);
        let buffer = render(&mut state);
        assert_eq!(space_count(&state, &buffer, "ws_1"), None, "{status:?}");
    }
}

#[test]
fn a_folded_worktree_parent_counts_itself_and_its_linked_worktrees_only() {
    // Two checkouts of the repository (`p`, `q`) and its linked worktree `c`.
    let mut projected = named_spaces(&["p", "q", "c", "plain"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    let waiting = times("ws_1", 1)
        .chain(times("ws_2", 2))
        .chain(times("ws_3", 4))
        .collect::<Vec<_>>();
    let mut state = grouped_state(with_waiting(focused_on(projected, "ws_4"), &waiting));
    let buffer = render(&mut state);
    for (space, expected) in [("ws_1", 1), ("ws_2", 2), ("ws_3", 4)] {
        assert_eq!(space_count(&state, &buffer, space), count(expected));
    }
    assert_eq!(space_count(&state, &buffer, "ws_4"), None);
    // Left of the ▾ on a parent row, at the edge on the child's.
    let parent = space_rect(&state, "ws_1");
    assert_eq!(
        red_count(&buffer, &state.config.palette, parent).map(|(_, end)| end),
        Some(parent.right() - 2)
    );
    let child = space_rect(&state, "ws_3");
    assert_eq!(
        red_count(&buffer, &state.config.palette, child).map(|(_, end)| end),
        Some(child.right())
    );

    state.collapsed_groups.insert("/repo/.git".into());
    let buffer = render(&mut state);
    assert_eq!(space_count(&state, &buffer, "ws_1"), count(5), "1 + c's 4");
    assert_eq!(
        space_count(&state, &buffer, "ws_2"),
        count(6),
        "2 + c's 4, not the other checkout's"
    );
    assert!(!has_hit(&state, "ws_3"));

    // The focused child shows under its folded parent with its own count, which the parent
    // keeps too, as it keeps the child's status.
    set_focus(&mut state, "ws_3");
    let buffer = render(&mut state);
    assert_eq!(space_count(&state, &buffer, "ws_3"), count(4));
    assert_eq!(space_count(&state, &buffer, "ws_1"), count(5));
}

#[test]
fn a_folded_group_header_shows_the_sum_of_its_members_counts() {
    let waiting = times("ws_1", 1)
        .chain(times("ws_2", 2))
        .chain(times("ws_3", 4))
        .collect::<Vec<_>>();
    let mut state = grouped_state(with_waiting(
        focused_on(named_spaces(&["A/one", "A/two", "plain"]), "ws_3"),
        &waiting,
    ));
    let buffer = render(&mut state);
    assert_eq!(
        header_count(&state, &buffer, "A"),
        None,
        "unfolded, the members show their own"
    );
    assert_eq!(space_count(&state, &buffer, "ws_1"), count(1));
    assert_eq!(space_count(&state, &buffer, "ws_2"), count(2));
    // A member's row spans the full width, so its count ends at the edge like any other.
    let member = space_rect(&state, "ws_2");
    assert_eq!(
        red_count(&buffer, &state.config.palette, member).map(|(_, end)| end),
        Some(member.right())
    );

    state.collapsed_groups.insert("group:A".into());
    let buffer = render(&mut state);
    let header = header_rect(&state, "A");
    assert_eq!(
        red_count(&buffer, &state.config.palette, header),
        Some(("3".to_owned(), header.right())),
        "at the header's right edge"
    );
    assert_eq!(space_count(&state, &buffer, "ws_3"), count(4));
    let text = (header.x..header.right())
        .map(|x| buffer[(x, header.y)].symbol())
        .collect::<String>();
    assert!(text.starts_with(" ▸ ") && text.contains(" A "), "{text:?}");

    // A focused member shows under the folded header with its own count; the header still
    // counts it once.
    set_focus(&mut state, "ws_1");
    let buffer = render(&mut state);
    assert_eq!(space_count(&state, &buffer, "ws_1"), count(1));
    assert_eq!(header_count(&state, &buffer, "A"), count(3));
}

#[test]
fn a_folded_group_counts_a_folded_worktree_block_inside_it_once() {
    // Group `Web`: checkouts `p` and `q` of the repository with its linked worktrees `c1` and
    // `c2` (focused), then `plain`.
    let mut projected = named_spaces(&["Web/p", "Web/q", "Web/c1", "c2", "plain"]);
    in_worktree(&mut projected, 1, false);
    in_worktree(&mut projected, 2, false);
    in_worktree(&mut projected, 3, true);
    in_worktree(&mut projected, 4, true);
    projected.workspaces[3].custom_label = false;
    projected.workspaces[3].branch = Some("worktree/c2".into());
    let waiting = times("ws_1", 1)
        .chain(times("ws_2", 2))
        .chain(times("ws_3", 4))
        .chain(times("ws_4", 8))
        .chain(times("ws_5", 16))
        .collect::<Vec<_>>();
    let mut state = grouped_state(with_waiting(focused_on(projected, "ws_4"), &waiting));
    state.collapsed_groups.insert("/repo/.git".into());
    state.collapsed_groups.insert("group:Web".into());
    let buffer = render(&mut state);
    // The folded group shows the focused child with its block's parent rows.
    assert_eq!(space_count(&state, &buffer, "ws_1"), count(1 + 4 + 8));
    assert_eq!(space_count(&state, &buffer, "ws_2"), count(2 + 4 + 8));
    assert_eq!(space_count(&state, &buffer, "ws_4"), count(8));
    assert!(!has_hit(&state, "ws_3"));
    assert_eq!(
        header_count(&state, &buffer, "Web"),
        count(1 + 2 + 4 + 8),
        "each space once"
    );
    assert_eq!(space_count(&state, &buffer, "ws_5"), count(16));

    state.collapsed_groups.remove("group:Web");
    let buffer = render(&mut state);
    assert_eq!(header_count(&state, &buffer, "Web"), None);
}

#[test]
fn counts_show_with_the_agents_panel_hidden() {
    let mut state = grouped_state(with_waiting(
        focused_on(named_spaces(&["A/one", "A/two", "plain"]), "ws_3"),
        &["ws_2", "ws_3", "ws_3"],
    ));
    state.config.agents.hidden = true;
    state.collapsed_groups.insert("group:A".into());
    let buffer = render(&mut state);
    assert_eq!(header_count(&state, &buffer, "A"), count(1));
    assert_eq!(space_count(&state, &buffer, "ws_3"), count(2));
}

/// Every cell of `row`'s first line has `background` but the count's, which stays red.
fn assert_lit_but_the_count(
    buffer: &Buffer,
    palette: &Palette,
    row: Rect,
    background: ratatui::style::Color,
    expected: &str,
) {
    let (count, end) = red_count(buffer, palette, row).expect("a red count");
    assert_eq!(count, expected);
    let start = end - u16::try_from(expected.len() + 2).expect("narrow count");
    for x in row.x..row.right() {
        let want = if (start..end).contains(&x) {
            palette.red
        } else {
            background
        };
        assert_eq!(buffer[(x, row.y)].bg, want, "column {x}");
    }
}

#[test]
fn hovered_and_dragged_rows_keep_their_red_counts() {
    let mut state = grouped_state(with_waiting(
        focused_on(named_spaces(&["A/one", "A/two", "plain", "other"]), "ws_4"),
        &["ws_2", "ws_3", "ws_3"],
    ));
    state.collapsed_groups.insert("group:A".into());
    render(&mut state);
    let palette = state.config.palette.clone();
    let assert_row = |buffer: &Buffer, row: Rect, background, expected: &str| {
        assert_lit_but_the_count(buffer, &palette, row, background, expected);
    };

    let plain = space_rect(&state, "ws_3");
    let header = header_rect(&state, "A");
    for (row, expected) in [(plain, "2"), (header, "1")] {
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: row.x + 4,
            row: row.y,
            modifiers: KeyModifiers::NONE,
        })]);
        let buffer = render(&mut state);
        assert_row(&buffer, row, palette.selection_bg, expected);
    }
    state.clear_row_hover();

    state.chrome_drag = Some(ClientChromeDrag::Workspace {
        source_workspace_id: "ws_3".into(),
        target: None,
    });
    let buffer = render(&mut state);
    assert_row(&buffer, plain, palette.surface1, "2");
    state.chrome_drag = None;

    let from = header_point(&state, "A");
    mouse(&mut state, MouseEventKind::Down(MouseButton::Left), from);
    mouse(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        (from.0 + 1, plain.y),
    );
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::SpaceGroups(_))
    ));
    let buffer = render(&mut state);
    assert_row(&buffer, header, palette.surface1, "1");
    mouse(&mut state, MouseEventKind::Up(MouseButton::Left), (0, 0));
}

#[test]
fn a_drop_target_header_keeps_its_red_count() {
    let mut state = grouped_state(with_waiting(
        focused_on(named_spaces(&["A/one", "A/two", "plain", "other"]), "ws_4"),
        &["ws_2"],
    ));
    state.collapsed_groups.insert("group:A".into());
    render(&mut state);
    let header = header_rect(&state, "A");
    let point = drag_space(&mut state, "ws_3", header.y);
    assert_eq!(
        group_target(&state),
        Some(&GroupDropTarget::Header("A".into()))
    );
    let buffer = render(&mut state);
    let palette = &state.config.palette;
    let highlight = if palette.selection_bg == ratatui::style::Color::Reset {
        palette.active_row_bg
    } else {
        palette.selection_bg
    };
    assert_lit_but_the_count(&buffer, palette, header, highlight, "1");
    release(&mut state, point);
}

#[test]
fn the_collapsed_rail_turns_a_waiting_spaces_number_red() {
    // The rail lists `one`, `two`, `plain` (the grouped order); `one` is focused.
    let mut state = grouped_state(with_waiting(
        named_spaces(&["A/one", "plain", "A/two"]),
        &["ws_1", "ws_3"],
    ));
    state.sidebar_collapsed = true;
    let buffer = render(&mut state);
    assert!(state.hits.machines.is_empty(), "the one-machine rail");
    let palette = &state.config.palette;
    let rows = ["ws_1", "ws_3", "ws_2"].map(|workspace_id| space_rect(&state, workspace_id));
    assert_eq!(
        rows.map(|row| row.y - rows[0].y),
        [0, 1, 2],
        "in the grouped order"
    );
    for (row, (number, waiting)) in rows.iter().zip([("1", true), ("2", true), ("3", false)]) {
        let cell = &buffer[(row.x, row.y)];
        assert_eq!(cell.symbol(), number);
        assert_eq!(cell.bg == palette.red, waiting, "space number {number}");
        assert_eq!(buffer[(row.x + 1, row.y)].bg == palette.red, waiting);
        if waiting {
            assert_eq!(cell.fg, crate::client::shell::panel_contrast_fg(palette));
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
    }
}

/// `row`'s first line as text.
fn line(buffer: &Buffer, row: Rect) -> String {
    (row.x..row.right())
        .map(|x| buffer[(x, row.y)].symbol())
        .collect()
}

/// `row`'s first line, cell by cell.
fn cells(buffer: &Buffer, row: Rect) -> Vec<ratatui::buffer::Cell> {
    (row.x..row.right())
        .map(|x| buffer[(x, row.y)].clone())
        .collect()
}

#[test]
fn a_cut_off_name_keeps_its_ellipsis_before_the_count() {
    let long = "a-space-name-far-too-long-for-the-sidebar";
    // `ws_3` is a worktree parent (with a ▾) of `ws_4`.
    let mut projected = named_spaces(&[long, long, long, "child"]);
    in_worktree(&mut projected, 3, false);
    in_worktree(&mut projected, 4, true);
    let projected = focused_on(projected, "ws_4");
    let mut before = grouped_state(projected.clone());
    let plain = render(&mut before);
    let waiting = times("ws_1", 1)
        .chain(times("ws_3", 12))
        .collect::<Vec<_>>();
    let mut state = grouped_state(with_waiting(projected, &waiting));
    let buffer = render(&mut state);

    let without_count = space_rect(&before, "ws_1");
    assert_eq!(
        plain[(without_count.right() - 3, without_count.y)].symbol(),
        "…",
        "cut off: {:?}",
        line(&plain, without_count)
    );
    for (workspace_id, count, end) in [("ws_1", "1", 0), ("ws_3", "12", 2)] {
        let row = space_rect(&state, workspace_id);
        assert_eq!(
            red_count(&buffer, &state.config.palette, row),
            Some((count.to_owned(), row.right() - end)),
            "left of the ▾ on the parent"
        );
        let start = row.right() - end - u16::try_from(count.len() + 2).expect("narrow count");
        let name = line(&buffer, Rect::new(row.x, row.y, start - row.x, 1));
        assert!(
            name.contains(" a-space-name") && name.ends_with('…'),
            "{workspace_id}: {:?}",
            line(&buffer, row)
        );
    }
    // Rows without a count are drawn as before.
    for workspace_id in ["ws_2", "ws_4"] {
        let row = space_rect(&state, workspace_id);
        assert_eq!(
            cells(&buffer, row),
            cells(&plain, space_rect(&before, workspace_id)),
            "{workspace_id}"
        );
    }
}

/// The window's own machine and the remote `Build`, both online, one space each; the remote's
/// first snapshot has a finished agent nobody has looked at for each of `remote_waiting`.
fn with_remote(
    local: ClientShellSnapshot,
    remote_waiting: &[&str],
) -> (ClientShellState, ClientEndpointId) {
    let profile = SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").expect("profile id"),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    let mut state = grouped_state(local);
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    let mut projected = snapshot();
    projected.boot_id = "remote-boot".into();
    projected.workspaces[0].label = "remote-workspace".into();
    let projected = with_waiting(projected, remote_waiting);
    state.set_endpoint_snapshot(&remote, Box::new(projected));
    (state, remote)
}

fn machine_rect(state: &ClientShellState, endpoint_id: &ClientEndpointId) -> Rect {
    state
        .hits
        .machines
        .iter()
        .find(|hit| &hit.endpoint_id == endpoint_id)
        .map(|hit| hit.rect)
        .expect("machine row")
}

fn machine_space_rect(state: &ClientShellState, endpoint_id: &ClientEndpointId) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| &hit.endpoint_id == endpoint_id)
        .map(|hit| hit.rect)
        .expect("space row")
}

#[test]
fn machines_and_their_spaces_show_the_counts_too() {
    let (mut state, remote) = with_remote(snapshot(), &[]);
    let mut projected = state
        .endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_id == remote)
        .and_then(|endpoint| endpoint.snapshot.clone())
        .expect("remote snapshot");
    projected.revision += 1;
    projected.agents = vec![
        waiting_agent("ws_1", "pane_1", AgentStatus::Blocked),
        waiting_agent("ws_1", "pane_2", AgentStatus::Working),
    ];
    state.set_endpoint_snapshot(&remote, projected);
    let palette = state.config.palette.clone();
    let count_on = |buffer: &Buffer, row| red_count(buffer, &palette, row).map(|(count, _)| count);

    let buffer = render(&mut state);
    assert_eq!(
        count_on(&buffer, machine_space_rect(&state, &remote)),
        count(1)
    );
    assert_eq!(
        count_on(&buffer, machine_rect(&state, &remote)),
        None,
        "an expanded machine leaves the count to its spaces"
    );
    assert_eq!(
        count_on(
            &buffer,
            machine_space_rect(&state, &ClientEndpointId::Local)
        ),
        None
    );

    state.collapsed_endpoints.insert(remote.clone());
    let buffer = render(&mut state);
    let machine = machine_rect(&state, &remote);
    assert_eq!(
        count_on(&buffer, machine),
        count(1),
        "a folded machine shows the count of its hidden spaces"
    );
    let text = (machine.x..machine.right())
        .map(|x| buffer[(x, machine.y)].symbol())
        .collect::<String>();
    assert!(
        text.trim_end().ends_with('●'),
        "the status badge stays rightmost: {text}"
    );
}

#[test]
fn the_narrow_multi_machine_sidebar_shows_red_space_numbers_and_folded_counts() {
    let (mut state, remote) = with_remote(with_waiting(snapshot(), &["ws_1"]), &["ws_1"]);
    state.sidebar_collapsed = true;
    let palette = state.config.palette.clone();
    let buffer = render(&mut state);
    let number_bg = |buffer: &Buffer, row: Rect| buffer[(row.x + 1, row.y)].bg;
    assert_eq!(
        number_bg(
            &buffer,
            machine_space_rect(&state, &ClientEndpointId::Local)
        ),
        palette.red,
        "a space with a waiting agent has a red number"
    );
    assert_eq!(
        number_bg(&buffer, machine_space_rect(&state, &remote)),
        palette.red,
        "a remote's space too"
    );

    state.collapsed_endpoints.insert(ClientEndpointId::Local);
    state.collapsed_endpoints.insert(remote.clone());
    let buffer = render(&mut state);
    let local = machine_rect(&state, &ClientEndpointId::Local);
    assert_eq!(
        red_count(&buffer, &palette, local),
        Some(("1".to_owned(), local.right()))
    );
    assert_eq!(
        red_count(&buffer, &palette, machine_rect(&state, &remote)),
        None,
        "a remote machine keeps its status badge"
    );
}
