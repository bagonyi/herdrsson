#![cfg(unix)]

use super::*;

#[tokio::test]
async fn plugin_pane_open_split_takes_its_size_as_a_percentage() {
    let mut app = test_app();
    app.state.workspaces = vec![crate::workspace::Workspace::test_new("plugin-split-size")];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = crate::app::Mode::Terminal;
    let root = unique_temp_path("plugin-pane-split-size");
    write_manifest_content(
        &root,
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
command = ["sh", "-c", "sleep 1"]
"#,
    );
    link_manifest(&mut app, &root);

    use crate::api::schema::SplitDirection::{self, Down, Right};
    use crate::popup_size::PopupSize::{self, Cells, Percent};
    fn open(
        app: &mut App,
        direction: SplitDirection,
        width: Option<PopupSize>,
        height: Option<PopupSize>,
    ) -> serde_json::Value {
        let response = app.handle_api_request(Request {
            id: "pane-open-split-size".into(),
            method: Method::PluginPaneOpen(PluginPaneOpenParams {
                plugin_id: "example.split".into(),
                entrypoint: "board".into(),
                placement: Some(PluginPanePlacement::Split),
                width,
                height,
                workspace_id: None,
                target_pane_id: None,
                direction: Some(direction),
                cwd: None,
                focus: false,
                env: std::collections::HashMap::new(),
            }),
        });
        serde_json::from_str(&response).unwrap()
    }
    // The program starts at the size its pane is laid out at, not at half.
    fn assert_spawned_at_laid_out_size(app: &App, response: &serde_json::Value) {
        let public_id = response["result"]["plugin_pane"]["pane"]["pane_id"]
            .as_str()
            .expect("opened pane id");
        let (ws_idx, pane) = app.parse_pane_id(public_id).expect("pane id parses");
        let spawned = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, ws_idx, pane)
            .expect("opened pane runtime")
            .current_size();
        assert_eq!(
            spawned,
            app.state
                .new_pane_size(crate::ui::NewPanePlacement::Existing { ws_idx, pane })
        );
    }
    let area = ratatui::layout::Rect::new(0, 0, 100, 40);
    let new_pane_rect = |app: &App, response: &serde_json::Value| {
        let public_id = response["result"]["plugin_pane"]["pane"]["pane_id"]
            .as_str()
            .expect("opened pane id");
        let (_, pane_id) = app.parse_pane_id(public_id).expect("pane id parses");
        app.state.workspaces[0].tabs[0]
            .layout
            .panes(area)
            .into_iter()
            .find(|pane| pane.id == pane_id)
            .expect("opened pane in layout")
            .rect
    };

    assert_eq!(
        open(&mut app, Down, Some(Percent(30)), None)["error"]["code"],
        "invalid_params"
    );
    assert_eq!(
        open(&mut app, Right, Some(Cells(30)), None)["error"]["code"],
        "invalid_params"
    );
    // The new pane, not the one split, gets the size: 30% of the width,
    // then 25% of the height of the focused pane it splits below.
    let right = open(&mut app, Right, Some(Percent(30)), None);
    assert_eq!(new_pane_rect(&app, &right).width, 30);
    assert_spawned_at_laid_out_size(&app, &right);
    let down = open(&mut app, Down, None, Some(Percent(25)));
    assert_eq!(new_pane_rect(&app, &down).height, 10);
    assert_spawned_at_laid_out_size(&app, &down);

    for (_, runtime) in app.terminal_runtimes.drain() {
        runtime.shutdown();
    }
    let _ = std::fs::remove_dir_all(root);
}
