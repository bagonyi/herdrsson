use super::*;

#[test]
fn tab_scroll_arrows_take_the_colour_of_hidden_agents_needing_attention() {
    let mut projected = snapshot();
    projected.tabs.extend((2..=8).map(|number| ClientShellTab {
        tab_id: format!("tab_{number}"),
        workspace_id: "ws_1".into(),
        number,
        label: number.to_string(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    }));
    let arrow_fg = |state: &mut ClientShellState, snapshot: &ClientShellSnapshot| {
        state.set_snapshot(Box::new(snapshot.clone()));
        state.set_pane_surface(surface());
        let frame = state.compose(80, 20).expect("overflow tab bar");
        let fg = |rect: Rect| frame.cells[(rect.y * frame.width + rect.x + 1) as usize].fg;
        (
            fg(state.hits.tab_scroll_left),
            fg(state.hits.tab_scroll_right),
        )
    };
    let color = crate::protocol::color_to_u32;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let palette = state.config.palette.clone();

    projected.tabs[7].agent_status = AgentStatus::Done;
    assert_eq!(
        arrow_fg(&mut state, &projected),
        (color(palette.overlay0), color(palette.teal)),
        "a finished agent on a hidden tab to the right"
    );

    projected.tabs[6].agent_status = AgentStatus::Blocked;
    assert_eq!(
        arrow_fg(&mut state, &projected).1,
        color(palette.red),
        "a blocked agent outranks a finished one"
    );

    projected.tabs[0].agent_status = AgentStatus::Done;
    projected.tabs[6].agent_status = AgentStatus::Working;
    projected.focused_tab_id = Some("tab_8".into());
    for tab in &mut projected.tabs {
        tab.focused = tab.tab_id == "tab_8";
    }
    assert_eq!(
        arrow_fg(&mut state, &projected),
        (color(palette.teal), color(palette.overlay0)),
        "a finished agent on a hidden tab to the left; working agents stay uncoloured"
    );
}

#[test]
fn tab_scroll_arrow_counts_a_tab_cut_short_at_the_edge_as_hidden() {
    let mut projected = snapshot();
    projected.tabs.extend((2..=12).map(|number| ClientShellTab {
        tab_id: format!("tab_{number}"),
        workspace_id: "ws_1".into(),
        number,
        label: number.to_string(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    }));
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let palette = state.config.palette.clone();
    state.set_snapshot(Box::new(projected.clone()));
    state.set_pane_surface(surface());
    // Find a width that leaves the last drawn tab a single column, under the "…".
    let (width, (last, tab_id)) = (60..140)
        .find_map(|width| {
            state.compose(width, 20).expect("overflow tab bar");
            let last = state.hits.tabs.last().cloned()?;
            (last.0.width == 1).then_some((width, last))
        })
        .expect("a width that cuts the last drawn tab to one column");

    let index = projected
        .tabs
        .iter()
        .position(|tab| tab.tab_id == tab_id)
        .expect("drawn tab");
    projected.tabs[index].agent_status = AgentStatus::Done;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(width, 20).expect("finished tab cut short");
    let cell = |x: u16| &frame.cells[(last.y * frame.width + x) as usize];
    assert_eq!(cell(last.x).symbol, "…", "nothing of {tab_id} is on screen");
    let right = state.hits.tab_scroll_right;
    assert_eq!(
        cell(right.x + 1).fg,
        crate::protocol::color_to_u32(palette.teal),
        "{tab_id} is finished and cut short, so '>' points to it"
    );
}

#[test]
fn hidden_agents_panel_gives_the_sidebar_to_spaces() {
    let sidebar_text = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("expanded sidebar");
        frame
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>()
    };
    let mut config = Config::default();
    let mut shown = ClientShellState::new(ClientShellConfig::from_config(&config));
    shown.set_snapshot(Box::new(snapshot()));
    shown.set_pane_surface(surface());
    assert!(sidebar_text(&mut shown).contains(" agents"));
    assert!(shown.hits.workspace_body.bottom() + 1 < shown.hits.sidebar_toggle.y);

    config.ui.sidebar.agents.hidden = true;
    let mut hidden = ClientShellState::new(ClientShellConfig::from_config(&config));
    hidden.set_snapshot(Box::new(snapshot()));
    hidden.set_pane_surface(surface());
    let text = sidebar_text(&mut hidden);
    assert!(text.contains(" spaces"));
    assert!(!text.contains(" agents"));
    assert_eq!(hidden.hits.sidebar_section_divider, Rect::default());
    assert_eq!(
        hidden.hits.workspace_body.bottom() + 1,
        hidden.hits.sidebar_toggle.y,
        "the spaces list ends at its footer, right above the sidebar toggle"
    );
}

#[test]
fn hidden_agents_panel_leaves_space_groups_the_full_height() {
    // Ten groups with one space each: 20 rows, more than the spaces list holds beside the
    // agents panel.
    let grouped = || {
        let mut projected = snapshot();
        projected.workspaces = (1..=10)
            .map(|number| {
                let mut workspace = projected.workspaces[0].clone();
                workspace.workspace_id = format!("ws_{number}");
                workspace.number = number;
                workspace.label = format!("G{number}/space{number}");
                workspace.custom_label = true;
                workspace.branch = None;
                workspace.focused = number == 1;
                workspace
            })
            .collect();
        projected
    };
    let mut config = Config::default();
    let mut shown = ClientShellState::new(ClientShellConfig::from_config(&config));
    shown.set_snapshot(Box::new(grouped()));
    shown.set_pane_surface(surface());
    shown.compose(106, 30).expect("frame");
    assert!(shown.hits.group_headers.len() < 10);

    config.ui.sidebar.agents.hidden = true;
    let mut hidden = ClientShellState::new(ClientShellConfig::from_config(&config));
    hidden.set_snapshot(Box::new(grouped()));
    hidden.set_pane_surface(surface());
    hidden.compose(106, 30).expect("frame");
    assert_eq!(hidden.hits.group_headers.len(), 10);
    assert_eq!(hidden.hits.workspaces.len(), 10);
    let body = hidden.hits.workspace_body;
    assert!(hidden
        .hits
        .group_headers
        .iter()
        .all(|hit| hit.rect.y >= body.y && hit.rect.y < body.bottom()));

    let header = hidden
        .hits
        .group_headers
        .iter()
        .find(|hit| hit.group == "G10")
        .expect("last group's header")
        .rect;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        hidden.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: header.x + 4,
            row: header.y,
            modifiers: KeyModifiers::empty(),
        })]);
    }
    assert!(hidden.collapsed_groups.contains("group:G10"));
}

#[test]
fn agents_header_shows_how_many_agents_it_lists() {
    let agent = |pane_id: &str| ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    };
    let header = |projected: &ClientShellSnapshot| {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(projected.clone()));
        state.set_pane_surface(surface());
        let frame = state.compose(106, 30).expect("expanded sidebar");
        frame
            .cells
            .chunks(frame.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .find(|row| row.starts_with(" agents"))
            .expect("agents header")
    };

    let mut projected = snapshot();
    assert!(header(&projected).starts_with(" agents · 0 "));
    projected.agents = vec![agent("pane_a"), agent("pane_b"), agent("pane_c")];
    assert!(header(&projected).starts_with(" agents · 3 "));

    projected.agent_view_label = Some("recent".into());
    projected.agent_order = vec!["pane_c".into(), "pane_a".into()];
    assert!(
        header(&projected).starts_with(" agents · 2 "),
        "a plugin's agent view counts the agents it lists"
    );
}
