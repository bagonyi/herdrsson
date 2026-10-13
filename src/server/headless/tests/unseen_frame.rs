use super::*;

use crate::terminal::TerminalRuntime;
use crate::workspace::Workspace;

type Render = std::sync::mpsc::Receiver<Vec<u8>>;

fn frame_symbol(surface: &crate::protocol::PaneSurfaceFrame, x: u16, y: u16) -> &str {
    let index = usize::from(y) * usize::from(surface.frame.width) + usize::from(x);
    &surface.frame.cells[index].symbol
}

/// Whether the full render's pane at `index` has the green frame drawn over its top-left corner.
fn framed(surface: &crate::protocol::PaneSurfaceFrame, index: usize) -> bool {
    let rect = surface.panes[index].rect;
    frame_symbol(surface, rect.x, rect.y) == "┌"
}

fn set_unseen(server: &mut HeadlessServer, pane_id: crate::layout::PaneId) {
    server.app.state.test_finish_unseen(0, pane_id);
}

fn seen(server: &HeadlessServer, pane_id: crate::layout::PaneId) -> bool {
    server.app.state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .seen
}

fn set_focus(server: &mut HeadlessServer, focused: bool) {
    server.clients.get_mut(&7).unwrap().outer_terminal_focus = Some(focused);
}

fn show_workspace(server: &mut HeadlessServer, workspace: Workspace, render: &Render) {
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.reconcile_client_shell_locations();
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    let _ = recv_pane_surface(render, "baseline");
}

fn report_test_agent_state(
    server: &mut HeadlessServer,
    public_pane_id: &str,
    seq: u64,
    state: api::schema::PaneAgentState,
) {
    completion_guard_api_report(
        server,
        api::schema::Method::PaneReportAgent(api::schema::PaneReportAgentParams {
            pane_id: public_pane_id.into(),
            source: "test-agent".into(),
            agent: "test-agent".into(),
            state,
            message: None,
            seq: Some(seq),
            agent_session_id: None,
            agent_session_path: None,
            resume_argv: None,
        }),
    );
}

/// A window attaching to a server whose current tab has a finished agent frames it while the
/// window is in the background. Without a seen delay, the window coming to the front counts it
/// as seen on the tick of that loop pass, and the frame goes with the full render the tick asks
/// for.
fn attaching_window_frames_its_tab_until_it_comes_to_the_front(
    server: &mut HeadlessServer,
) -> (Render, Render) {
    let pane_id = install_shared_view_test_runtime(server);
    set_unseen(server, pane_id);
    let (control, render) = connect_matching_test_shell(server, 7);
    assert_eq!(server.foreground_client_id, Some(7));
    // Nothing to do on the tick: the frame needs no clock while the agent waits.
    assert!(!server.handle_scheduled_tasks_headless(Instant::now(), false));
    assert!(!seen(server, pane_id));
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    assert!(framed(&recv_pane_surface(&render, "background"), 0));

    assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
        client_id: 7,
        focused: true,
    }));
    assert!(server.handle_scheduled_tasks_headless(Instant::now(), false));
    assert!(seen(server, pane_id));
    assert!(!server.app.state.pane_is_unseen(0, pane_id));
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    assert!(!framed(&recv_pane_surface(&render, "front"), 0));

    // The cases below start with the window unfocused.
    assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
        client_id: 7,
        focused: false,
    }));
    while render.try_recv().is_ok() {}
    (control, render)
}

/// A lone pane's frame is drawn in full renders for as long as its agent is unseen, such as while
/// the window is in the background, keeps its own output off the retained path, and goes once the
/// agent counts as seen.
fn lone_pane_frame_lasts_until_its_agent_is_seen(server: &mut HeadlessServer, render: &Render) {
    let pane_id = install_shared_view_test_runtime(server);
    server.render_and_stream();
    let baseline = recv_pane_surface(render, "baseline");
    assert!(!framed(&baseline, 0));

    set_unseen(server, pane_id);
    let now = Instant::now();
    // Static while it waits: no tick asks for a render, and none wakes the loop for it.
    for later in [now, now + Duration::from_secs(10)] {
        assert!(!server.handle_scheduled_tasks_headless(later, false));
    }
    assert_eq!(server.app.state.next_seen_wait_deadline(), None);
    assert!(!seen(server, pane_id));
    server.render_and_stream();
    assert!(framed(&recv_pane_surface(render, "framed"), 0));

    // The framed pane's own output would paint over the frame, so it isn't patched in.
    write_shared_test_pane(server, pane_id, b"more");
    assert!(!server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    server.render_and_stream();
    let _ = recv_pane_surface(render, "framed output");

    // The window comes back: the agent is seen, its frame goes, and output is patched in again.
    set_focus(server, true);
    assert!(server.handle_scheduled_tasks_headless(now, false));
    set_focus(server, false);
    assert!(seen(server, pane_id));
    server.render_and_stream();
    assert!(!framed(&recv_pane_surface(render, "unframed"), 0));
    write_shared_test_pane(server, pane_id, b"again");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
}

/// The agent finishing while the window is in the background frames its pane, and its working
/// again takes the frame away. Both come as API requests, which render in full.
fn agent_working_again_takes_its_frame_away(server: &mut HeadlessServer, render: &Render) {
    set_focus(server, false);
    server.sync_foreground_client_state();
    let pane_id = install_shared_view_test_runtime(server);
    server.app.state.ensure_test_terminals();
    let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();
    server.render_and_stream();
    assert!(!framed(&recv_pane_surface(render, "baseline"), 0));

    for (seq, state) in [
        (1, api::schema::PaneAgentState::Working),
        (2, api::schema::PaneAgentState::Idle),
    ] {
        report_test_agent_state(server, &public_pane_id, seq, state);
    }
    assert!(server.app.state.pane_is_unseen(0, pane_id));
    server.render_and_stream();
    assert!(framed(&recv_pane_surface(render, "finished"), 0));

    report_test_agent_state(
        server,
        &public_pane_id,
        3,
        api::schema::PaneAgentState::Working,
    );
    assert!(!server.app.state.pane_is_unseen(0, pane_id));
    server.render_and_stream();
    assert!(!framed(&recv_pane_surface(render, "working"), 0));
}

/// Typing in the window gets the server to count its tab as looked at, on the tick of that loop
/// pass. Typing doesn't render on its own, so the tick asks for the full render that takes the
/// frame away.
fn typing_counts_a_finished_agent_as_seen_and_asks_for_a_full_render(server: &mut HeadlessServer) {
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    set_unseen(server, pane_id);
    set_focus(server, true);

    server.sync_foreground_client_state();
    assert!(server.handle_scheduled_tasks_headless(Instant::now(), false));

    assert!(seen(server, pane_id));
    // Nothing newly seen: no render asked for.
    server.sync_foreground_client_state();
    assert!(!server.handle_scheduled_tasks_headless(Instant::now(), false));
    set_focus(server, false);
    server.sync_foreground_client_state();
}

/// Closing the tab a window shows brings the finished agent's tab behind it on screen: with the
/// window focused, that counts as looking at it.
fn closing_the_tab_in_front_counts_as_seeing_the_tab_behind(server: &mut HeadlessServer) {
    let now = Instant::now();
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let front = server.app.state.workspaces[0].test_add_tab(None);
    server.app.state.workspaces[0].switch_tab(front);
    let front_id = server.app.public_tab_id(0, front).unwrap();
    assert!(server.focus_shell_client_on_tab(7, &front_id));
    set_focus(server, true);
    set_unseen(server, pane_id);
    server.handle_scheduled_tasks_headless(now, false);
    assert!(!seen(server, pane_id));

    // Closing a tab reconciles the windows' locations, so the window now shows the tab behind.
    assert!(server.app.state.workspaces[0].close_tab(front));
    server.reconcile_client_shell_locations();
    assert!(server.handle_scheduled_tasks_headless(now, false));
    assert!(seen(server, pane_id));
    assert!(!server.app.state.pane_is_unseen(0, pane_id));
}

/// The window's own tab is what it shows, even when the server's default tab is another one.
fn focused_window_sees_only_its_own_tab(server: &mut HeadlessServer, render: &Render) {
    let mut workspace = Workspace::test_new("two-tabs");
    let shown_tab = workspace.test_add_tab(None);
    let shown = workspace.tabs[shown_tab].root_pane;
    let default = workspace.tabs[0].root_pane;
    show_workspace(server, workspace, render);
    let shown_tab_id = server.app.public_tab_id(0, shown_tab).unwrap();
    assert!(server.focus_shell_client_on_tab(7, &shown_tab_id));
    assert_eq!(server.app.state.workspaces[0].active_tab_index(), 0);
    set_unseen(server, shown);
    set_unseen(server, default);

    server.handle_scheduled_tasks_headless(Instant::now(), false);

    assert!(seen(server, shown));
    assert!(!seen(server, default));
    assert!(server.app.state.pane_is_unseen(0, default));
}

/// A bordered pane's frame is on its border, which its output never reaches, so its output
/// still takes the retained path.
fn bordered_pane_output_keeps_the_retained_path(server: &mut HeadlessServer, render: &Render) {
    let mut workspace = Workspace::test_new("split");
    let left = workspace.tabs[0].root_pane;
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.insert_test_runtime(left, TerminalRuntime::test_with_screen_bytes(40, 21, b"L"));
    workspace.insert_test_runtime(right, TerminalRuntime::test_with_screen_bytes(40, 21, b"R"));
    show_workspace(server, workspace, render);
    set_unseen(server, right);
    assert!(server.app.state.pane_is_unseen(0, right));
    server.render_and_stream();
    let _ = recv_pane_surface(render, "framed split");

    write_shared_test_pane(server, right, b"more");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([right])));
}

/// With a seen delay, the window's tab counts as seen only once it has been on screen that long,
/// its agent framed meanwhile. Moving on sooner leaves its agent unseen, and framed whenever it
/// shows; coming back starts the wait over.
fn shown_tab_counts_as_seen_after_the_delay(server: &mut HeadlessServer, render: &Render) {
    let delay = std::time::Duration::from_secs(1);
    server.app.state.seen_delay = delay;
    set_focus(server, true);
    let mut workspace = Workspace::test_new("delay");
    let finished_tab = workspace.test_add_tab(None);
    let finished = workspace.tabs[finished_tab].root_pane;
    show_workspace(server, workspace, render);
    let first_tab_id = server.app.public_tab_id(0, 0).unwrap();
    let finished_tab_id = server.app.public_tab_id(0, finished_tab).unwrap();
    set_unseen(server, finished);

    let start = Instant::now();
    assert!(server.focus_shell_client_on_tab(7, &finished_tab_id));
    assert!(!server.handle_scheduled_tasks_headless(start, false));
    assert!(!seen(server, finished));
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    assert!(framed(&recv_pane_surface(render, "waiting"), 0));

    assert!(server.focus_shell_client_on_tab(7, &first_tab_id));
    assert!(!server.handle_scheduled_tasks_headless(start + delay / 2, false));
    assert!(!seen(server, finished));
    assert!(server.app.state.pane_is_unseen(0, finished));
    assert_eq!(server.app.state.next_seen_wait_deadline(), None);

    let back = start + delay;
    assert!(server.focus_shell_client_on_tab(7, &finished_tab_id));
    server.handle_scheduled_tasks_headless(back, false);
    assert!(!seen(server, finished));
    assert!(
        server
            .app
            .next_headless_loop_deadline_with_git_refresh(back, false, false)
            .unwrap()
            <= back + delay
    );
    assert!(server.handle_scheduled_tasks_headless(back + delay, false));
    assert!(seen(server, finished));
    assert!(!server.app.state.pane_is_unseen(0, finished));
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    assert!(!framed(&recv_pane_surface(render, "seen"), 0));
}

/// A pane hidden behind its tab's zoom (zen mode zooms too) isn't on screen: its finished agent
/// stays unseen however long the focused window shows the tab, and its wait starts once the zoom
/// is gone.
fn a_pane_behind_the_zoom_waits_until_it_shows(server: &mut HeadlessServer, render: &Render) {
    let delay = std::time::Duration::from_secs(1);
    server.app.state.seen_delay = delay;
    set_focus(server, true);
    let mut workspace = Workspace::test_new("zoom");
    let shown = workspace.tabs[0].root_pane;
    let hidden = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(shown);
    workspace.tabs[0].zoomed = true;
    show_workspace(server, workspace, render);
    set_unseen(server, hidden);

    let start = Instant::now();
    for now in [start, start + delay, start + delay * 3] {
        assert!(!server.handle_scheduled_tasks_headless(now, false));
    }
    assert!(!seen(server, hidden));
    assert!(server.app.state.pane_is_unseen(0, hidden));
    assert_eq!(server.app.state.next_seen_wait_deadline(), None);
    while render.try_recv().is_ok() {}
    server.render_and_stream();
    assert_eq!(recv_pane_surface(render, "zoomed").panes.len(), 1);

    // Unzoomed, it shows, and counts as seen once it has been on screen for the delay.
    server.app.state.workspaces[0].tabs[0].zoomed = false;
    server.render_and_stream();
    assert_eq!(recv_pane_surface(render, "unzoomed").panes.len(), 2);
    let back = start + delay * 3;
    assert!(!server.handle_scheduled_tasks_headless(back, false));
    assert!(!seen(server, hidden));
    assert!(server.handle_scheduled_tasks_headless(back + delay, false));
    assert!(seen(server, hidden));
}

// One server for every case: test servers created in the same instant share a socket path.
#[tokio::test]
async fn seen_pane_frame_on_a_server() {
    let mut server = test_headless_server();
    // Seen as soon as shown, until the delay's own case.
    server.app.state.seen_delay = std::time::Duration::ZERO;
    let (_control, render) =
        attaching_window_frames_its_tab_until_it_comes_to_the_front(&mut server);

    lone_pane_frame_lasts_until_its_agent_is_seen(&mut server, &render);
    typing_counts_a_finished_agent_as_seen_and_asks_for_a_full_render(&mut server);
    closing_the_tab_in_front_counts_as_seeing_the_tab_behind(&mut server);
    focused_window_sees_only_its_own_tab(&mut server, &render);
    bordered_pane_output_keeps_the_retained_path(&mut server, &render);
    shown_tab_counts_as_seen_after_the_delay(&mut server, &render);
    a_pane_behind_the_zoom_waits_until_it_shows(&mut server, &render);
    // Last: its agent reports can leave a toast or notification behind, which a later tick
    // would deliver and render.
    agent_working_again_takes_its_frame_away(&mut server, &render);
    shutdown_test_runtimes(&mut server);
}

/// The test's client socket variable and folder, put back and removed however the test ends.
struct FreshServerEnv {
    previous: Option<Option<std::ffi::OsString>>,
    dir: std::path::PathBuf,
}

impl FreshServerEnv {
    const VARIABLE: &'static str = crate::server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR;

    fn set(dir: std::path::PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        let previous = std::env::var_os(Self::VARIABLE);
        std::env::set_var(Self::VARIABLE, dir.join("client.sock"));
        Self {
            previous: Some(previous),
            dir,
        }
    }

    fn restore(&mut self) {
        match self.previous.take() {
            Some(Some(previous)) => std::env::set_var(Self::VARIABLE, previous),
            Some(None) => std::env::remove_var(Self::VARIABLE),
            None => {}
        }
    }
}

impl Drop for FreshServerEnv {
    fn drop(&mut self) {
        self.restore();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A server that has just started has no window, so nobody is looking at its current tab: an
/// agent whose finish the screen shows there stays unseen until a window looks. The config
/// tests' lock covers the variable; no other test reads it unlocked but autodetect's, which
/// sets it under its own lock: so the test binds only the path it set, and checks the server
/// did too.
#[test]
fn a_fresh_server_keeps_an_agent_finishing_in_its_current_tab_unseen() {
    let _lock = crate::config::test_config_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut env =
        FreshServerEnv::set(std::env::temp_dir().join(format!("hf-{}", std::process::id())));
    let socket = env.dir.join("client.sock");
    let path = client_socket_path();
    assert_eq!(
        path, socket,
        "never bind anything but the test's own socket"
    );
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::app::App::new(
        &crate::config::Config::default(),
        crate::app::AppPolicy::TEST,
        None,
        api_rx,
        api::EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("only")];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    let created = HeadlessServer::new(app, &[], None, None, ServerStop::default());
    env.restore();
    let mut server = created.expect("server");
    assert_eq!(server.client_socket_path, socket);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;

    for state in [
        crate::detect::AgentState::Working,
        crate::detect::AgentState::Idle,
    ] {
        server.app.state.handle_app_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(crate::detect::Agent::Pi),
            state,
            visible_blocker: false,
            visible_working: state == crate::detect::AgentState::Working,
            process_exited: false,
            observed_at: Instant::now(),
        });
    }

    let unseen = !seen(&server, pane_id);
    shutdown_test_runtimes(&mut server);
    drop(server);
    assert!(unseen);
}

#[test]
fn agent_finishing_in_active_tab_without_a_window_stays_unseen() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("only")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.sync_foreground_client_state();
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();

    report_test_agent_state(
        &mut server,
        &public_pane_id,
        1,
        api::schema::PaneAgentState::Working,
    );
    report_test_agent_state(
        &mut server,
        &public_pane_id,
        2,
        api::schema::PaneAgentState::Idle,
    );

    assert!(!server.app.state.workspaces[0].panes[&pane_id].seen);
}

#[test]
fn focused_window_seeing_finished_agents_emits_focus_events() {
    let event_hub = api::EventHub::default();
    let mut server = test_headless_server_with_event_hub(event_hub.clone());
    // Seen as soon as shown; other tests here cover the delay.
    server.app.state.seen_delay = std::time::Duration::ZERO;
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("only")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    server.app.state.workspaces[0]
        .panes
        .get_mut(&pane_id)
        .unwrap()
        .seen = false;
    let mut client = ClientConnection::new(
        (80, 24),
        Default::default(),
        1,
        RenderEncoding::SemanticFrame,
        None,
    );
    client.outer_terminal_focus = Some(true);
    server.clients.insert(1, client);
    server.foreground_client_id = Some(1);
    let focus_events = || {
        event_hub
            .events_after(0)
            .into_iter()
            .map(|(_, event)| event.event)
            .collect::<Vec<_>>()
    };

    server.sync_foreground_client_state();
    server.handle_scheduled_tasks_headless(Instant::now(), false);
    assert!(server.app.state.workspaces[0].panes[&pane_id].seen);
    assert_eq!(
        focus_events(),
        [
            api::schema::EventKind::WorkspaceFocused,
            api::schema::EventKind::TabFocused,
            api::schema::EventKind::PaneFocused,
        ]
    );

    server.sync_foreground_client_state();
    server.handle_scheduled_tasks_headless(Instant::now(), false);
    assert_eq!(focus_events().len(), 3, "nothing newly seen to announce");
}

fn focused_test_window(server: &mut HeadlessServer) {
    let mut client = ClientConnection::new(
        (80, 24),
        Default::default(),
        1,
        RenderEncoding::SemanticFrame,
        None,
    );
    client.outer_terminal_focus = Some(true);
    server.clients.insert(1, client);
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
}

#[tokio::test]
async fn focused_window_counts_finished_agents_as_seen_after_the_delay_and_announces_it() {
    let event_hub = api::EventHub::default();
    let mut server = test_headless_server_with_event_hub(event_hub.clone());
    let delay = std::time::Duration::from_secs(1);
    server.app.state.seen_delay = delay;
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("only")];
    server.app.state.active = Some(0);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    server.app.state.test_finish_unseen(0, pane_id);
    let focus_events = || {
        event_hub
            .events_after(0)
            .into_iter()
            .map(|(_, event)| event.event)
            .collect::<Vec<_>>()
    };

    let set_focus = |server: &mut HeadlessServer, focused: bool| {
        server.clients.get_mut(&1).unwrap().outer_terminal_focus = Some(focused);
        server.sync_foreground_client_state();
    };

    // The window comes back: the wait starts and the frame shows, but nothing is seen yet.
    focused_test_window(&mut server);
    let now = Instant::now();
    server.handle_scheduled_tasks_headless(now, false);
    assert!(!server.app.state.workspaces[0].panes[&pane_id].seen);
    assert!(server.app.state.pane_is_unseen(0, pane_id));
    assert_eq!(focus_events(), []);

    // In the background before the delay is up: the wait ends, and the frame stays as it is.
    set_focus(&mut server, false);
    assert!(!server.handle_scheduled_tasks_headless(now + delay / 2, false));
    assert!(!server.app.state.workspaces[0].panes[&pane_id].seen);
    assert!(server.app.state.pane_is_unseen(0, pane_id));
    assert_eq!(server.app.state.next_seen_wait_deadline(), None);

    // Back again, the wait starts over.
    set_focus(&mut server, true);
    let back = Instant::now();
    server.handle_scheduled_tasks_headless(back, false);
    assert!(!server.app.state.workspaces[0].panes[&pane_id].seen);
    assert!(server.handle_scheduled_tasks_headless(back + delay, false));
    assert!(server.app.state.workspaces[0].panes[&pane_id].seen);
    assert!(!server.app.state.pane_is_unseen(0, pane_id));
    // Announced the way coming back would announce agents seen at once.
    assert_eq!(
        focus_events(),
        [
            api::schema::EventKind::WorkspaceFocused,
            api::schema::EventKind::TabFocused,
            api::schema::EventKind::PaneFocused,
        ]
    );
    server.handle_scheduled_tasks_headless(back + delay, false);
    assert_eq!(focus_events().len(), 3, "nothing newly seen to announce");
}

#[test]
fn agent_finishing_in_the_focused_windows_tab_counts_as_seen_at_once_despite_the_delay() {
    let mut server = test_headless_server();
    server.app.state.seen_delay = std::time::Duration::from_secs(1);
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("only")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    focused_test_window(&mut server);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();

    report_test_agent_state(
        &mut server,
        &public_pane_id,
        1,
        api::schema::PaneAgentState::Working,
    );
    report_test_agent_state(
        &mut server,
        &public_pane_id,
        2,
        api::schema::PaneAgentState::Idle,
    );

    assert!(server.app.state.workspaces[0].panes[&pane_id].seen);
}

#[test]
fn agent_finishing_behind_the_zoom_in_the_focused_windows_tab_stays_unseen() {
    let mut server = test_headless_server();
    // Zen mode zooms too: only the zoomed pane is on screen.
    let mut workspace = crate::workspace::Workspace::test_new("zoomed");
    let shown = workspace.tabs[0].root_pane;
    let hidden = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(shown);
    workspace.tabs[0].zoomed = true;
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    focused_test_window(&mut server);

    for pane_id in [shown, hidden] {
        let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();
        for (seq, state) in [
            (1, api::schema::PaneAgentState::Working),
            (2, api::schema::PaneAgentState::Idle),
        ] {
            report_test_agent_state(&mut server, &public_pane_id, seq, state);
        }
    }

    assert!(server.app.state.workspaces[0].panes[&shown].seen);
    assert!(!server.app.state.workspaces[0].panes[&hidden].seen);
    assert!(server.app.state.pane_is_unseen(0, hidden));
}

fn request_surface(server: &mut HeadlessServer, client_id: u64, request_id: &str, active: bool) {
    let boot_id = server.client_shell_boot_id.clone();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellEndpointRequest {
            client_id,
            boot_id,
            request: Box::new(api::schema::Request {
                id: request_id.into(),
                method: api::schema::Method::ClientShellSurfaceSet(
                    api::schema::ClientShellSurfaceSetParams { active },
                ),
            }),
        })
    );
}

fn connect_shell_window(server: &mut HeadlessServer, client_id: u64, surface_active: bool) {
    let (writer, _control_rx, _render_rx) = test_client_writer();
    assert!(
        server.handle_server_event(ServerEvent::ClientShellConnected {
            surface_reuse: false,
            surface_delta: false,
            surface_scroll: false,
            client_id,
            surface_cols: 80,
            surface_rows: 24,
            cell_width_px: 8,
            cell_height_px: 16,
            pixel_mouse: false,
            direct_graphics: false,
            endpoint_keybindings: true,
            mouse_capture: true,
            surface_active,
            writer,
        })
    );
}

#[tokio::test]
async fn window_switching_to_another_machine_leaves_later_completions_unseen() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("only")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    let pane_id = server.app.state.workspaces[0].tabs[0].root_pane;
    let public_pane_id = server.app.public_pane_id(0, pane_id).unwrap();
    connect_shell_window(&mut server, 52, true);
    assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
        client_id: 52,
        focused: true,
    }));
    assert_eq!(server.app.state.outer_terminal_focus, Some(true));

    // The window moves to another machine: it revokes focus, then releases this surface.
    assert!(server.handle_server_event(ServerEvent::ClientShellFocus {
        client_id: 52,
        focused: false,
    }));
    request_surface(&mut server, 52, "surface-off", false);
    assert_eq!(server.foreground_client_id, None);
    assert_eq!(server.app.state.outer_terminal_focus, Some(false));

    report_test_agent_state(
        &mut server,
        &public_pane_id,
        1,
        api::schema::PaneAgentState::Working,
    );
    report_test_agent_state(
        &mut server,
        &public_pane_id,
        2,
        api::schema::PaneAgentState::Idle,
    );
    assert!(!server.app.state.workspaces[0].panes[&pane_id].seen);
}
