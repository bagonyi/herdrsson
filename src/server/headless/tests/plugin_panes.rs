#![cfg(unix)]

use super::*;

#[tokio::test]
async fn public_plugin_split_reapplies_controller_geometry() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("plugin-split-geometry");
    let first_pane = workspace.tabs[0].root_pane;
    workspace.insert_test_runtime(
        first_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;

    let root = std::env::temp_dir().join(format!(
        "hh-plugin-split-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&root).expect("plugin dir");
    fs::write(
        root.join("herdr-plugin.toml"),
        r#"
id = "example.split"
name = "Split Plugin"
version = "0.1.0"
min_herdr_version = "0.6.10"
platforms = ["linux", "macos"]

[[panes]]
id = "board"
title = "Plugin Board"
placement = "split"
command = ["sh", "-c", "sleep 5"]
"#,
    )
    .expect("plugin manifest");
    let linked = server.app.handle_api_request(api::schema::Request {
        id: "link".into(),
        method: api::schema::Method::PluginLink(api::schema::PluginLinkParams {
            path: root.display().to_string(),
            enabled: true,
            source: None,
        }),
    });
    assert!(linked.contains("plugin_linked"), "{linked}");

    let (control, _) = connect_test_shell(&mut server, 67, 100, 30);
    let _ = control.recv().expect("snapshot");
    let before = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "plugin-split".into(),
            method: api::schema::Method::PluginPaneOpen(api::schema::PluginPaneOpenParams {
                plugin_id: "example.split".into(),
                entrypoint: "board".into(),
                placement: Some(api::schema::PluginPanePlacement::Split),
                width: Some(crate::popup_size::PopupSize::Percent(30)),
                height: None,
                workspace_id: None,
                target_pane_id: None,
                direction: Some(api::schema::SplitDirection::Right),
                cwd: None,
                focus: false,
                env: HashMap::new(),
            }),
        },
        respond_to,
        response_write_complete: None,
    });
    let response = response_rx.recv().expect("plugin pane response");
    assert!(response.contains("plugin_pane_opened"), "{response}");

    // The split pane takes the client's geometry at once, not at the next click.
    let after = server.app.state.workspaces[0].test_runtimes[&first_pane].current_size();
    assert!(
        after.1 < before.1 * 3 / 4,
        "split pane kept {after:?} after the split, was {before:?}"
    );

    shutdown_test_runtimes(&mut server);
    let _ = fs::remove_dir_all(root);
}
