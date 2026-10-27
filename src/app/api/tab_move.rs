//! Fork: `tab.move_to_workspace`, a whole tab moving to the end of another workspace with its
//! layout, zoom and running panes. As after a cross-workspace `pane.move`, its panes take public
//! IDs in the target workspace (the old ones stay as aliases), and a workspace left without tabs
//! closes.

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, TabMoveToWorkspaceParams,
};
use crate::app::{App, Mode};

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_tab_move_to_workspace(
        &mut self,
        id: String,
        params: TabMoveToWorkspaceParams,
    ) -> String {
        let TabMoveToWorkspaceParams {
            tab_id,
            workspace_id,
            focus,
        } = params;
        let Some((source_ws_idx, source_tab_idx)) = self.parse_tab_id(&tab_id) else {
            return encode_error(id, "tab_not_found", format!("tab {tab_id} not found"));
        };
        let Some(mut target_ws_idx) = self.parse_workspace_id(&workspace_id) else {
            return encode_error(
                id,
                "workspace_not_found",
                format!("workspace {workspace_id} not found"),
            );
        };
        if source_ws_idx == target_ws_idx {
            return encode_error(id, "tab_move_failed", "tab is already in that workspace");
        }
        let Some(previous_tab_id) = self.public_tab_id(source_ws_idx, source_tab_idx) else {
            return encode_error(id, "tab_not_found", format!("tab {tab_id} not found"));
        };
        let previous_workspace_id = self.public_workspace_id(source_ws_idx);
        let previous_pane_ids = self.state.workspaces[source_ws_idx].tabs[source_tab_idx]
            .layout
            .pane_ids()
            .into_iter()
            .filter_map(|pane_id| Some((pane_id, self.public_pane_id(source_ws_idx, pane_id)?)))
            .collect::<Vec<_>>();
        let previous_focus = self.state.current_pane_focus_target();

        let Some(tab) = self.state.workspaces[source_ws_idx].take_tab_for_move(source_tab_idx)
        else {
            return encode_error(id, "tab_move_failed", "tab could not be moved");
        };
        let source_empty = self.state.workspaces[source_ws_idx].tabs.is_empty();
        let target_tab_idx = self.state.workspaces[target_ws_idx].insert_moved_tab(tab);
        for (pane_id, previous_pane_id) in &previous_pane_ids {
            self.state
                .public_pane_id_aliases
                .insert(previous_pane_id.clone(), *pane_id);
            self.state.remove_alias_shadowed_by_new_pane(*pane_id);
        }
        let closed_workspace_id = source_empty.then(|| {
            self.remove_workspace_emptied_by_tab_move(source_ws_idx);
            if source_ws_idx < target_ws_idx {
                target_ws_idx -= 1;
            }
            previous_workspace_id.clone()
        });

        if focus || self.state.active.is_none() {
            self.state
                .switch_workspace_tab(target_ws_idx, target_tab_idx);
            let focused = self.state.workspaces[target_ws_idx].tabs[target_tab_idx]
                .layout
                .focused();
            self.state
                .record_pane_focus_change(previous_focus, target_ws_idx, focused);
            self.state.mode = Mode::Terminal;
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();

        let Some(tab) = self.tab_info(target_ws_idx, target_tab_idx) else {
            return encode_error(id, "tab_move_failed", "moved tab is unavailable");
        };
        self.emit_event(EventEnvelope {
            event: EventKind::TabClosed,
            data: EventData::TabClosed {
                tab_id: previous_tab_id.clone(),
                workspace_id: previous_workspace_id.clone(),
            },
        });
        if let Some(closed_workspace_id) = &closed_workspace_id {
            self.emit_event(EventEnvelope {
                event: EventKind::WorkspaceClosed,
                data: EventData::WorkspaceClosed {
                    workspace_id: closed_workspace_id.clone(),
                    workspace: None,
                },
            });
        }
        self.emit_event(EventEnvelope {
            event: EventKind::TabCreated,
            data: EventData::TabCreated { tab: tab.clone() },
        });
        for (pane_id, previous_pane_id) in previous_pane_ids {
            let Some(pane) = self.pane_info(target_ws_idx, pane_id) else {
                continue;
            };
            self.emit_event(EventEnvelope {
                event: EventKind::PaneMoved,
                data: EventData::PaneMoved {
                    previous_pane_id,
                    previous_workspace_id: previous_workspace_id.clone(),
                    previous_tab_id: previous_tab_id.clone(),
                    pane: Box::new(pane),
                    created_workspace: None,
                    created_tab: None,
                    closed_workspace_id: None,
                    closed_tab_id: None,
                },
            });
        }
        if let Some(layout) = self.pane_layout_snapshot(target_ws_idx, target_tab_idx) {
            self.emit_layout_updated_snapshot(layout);
        }

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    /// Removes workspace `ws_idx`, emptied by a tab move, keeping the active and selected
    /// workspaces (a removed one gives way to its neighbour) as a cross-workspace `pane.move` does.
    fn remove_workspace_emptied_by_tab_move(&mut self, ws_idx: usize) {
        self.state.workspaces.remove(ws_idx);
        let last = self.state.workspaces.len().saturating_sub(1);
        if self.state.workspaces.is_empty() {
            self.state.active = None;
            self.state.selected = 0;
            return;
        }
        if let Some(active) = self.state.active {
            if active == ws_idx {
                self.state.active = Some(ws_idx.min(last));
            } else if active > ws_idx {
                self.state.active = Some(active - 1);
            }
        }
        if self.state.selected == ws_idx {
            self.state.selected = ws_idx.min(last);
        } else if self.state.selected > ws_idx {
            self.state.selected -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Direction;

    use super::*;
    use crate::{
        api::schema::{ErrorResponse, SuccessResponse},
        config::Config,
        workspace::Workspace,
    };

    fn app_with(workspaces: Vec<Workspace>) -> (App, crate::api::EventHub) {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            event_hub.clone(),
        );
        app.state.workspaces = workspaces;
        app.state.active = Some(0);
        app.state.selected = 0;
        for ws in &app.state.workspaces {
            for tab in &ws.tabs {
                for pane in tab.panes.values() {
                    app.state
                        .terminals
                        .entry(pane.attached_terminal_id.clone())
                        .or_insert_with(|| {
                            crate::terminal::TerminalState::new(
                                pane.attached_terminal_id.clone(),
                                std::path::PathBuf::from("/herdr-test"),
                            )
                        });
                }
            }
        }
        (app, event_hub)
    }

    fn move_tab(app: &mut App, tab_id: &str, workspace_id: &str, focus: bool) -> String {
        app.handle_tab_move_to_workspace(
            "req".into(),
            TabMoveToWorkspaceParams {
                tab_id: tab_id.into(),
                workspace_id: workspace_id.into(),
                focus,
            },
        )
    }

    /// Workspace "source" with tab 1 and tab "split" (two panes, zoomed), and "target".
    fn split_tab_setup() -> (App, crate::api::EventHub) {
        let mut source = Workspace::test_new("source");
        source.test_add_tab(Some("split"));
        source.switch_tab(1);
        source.test_split(Direction::Horizontal);
        source.tabs[1].zoomed = true;
        source.switch_tab(0);
        app_with(vec![source, Workspace::test_new("target")])
    }

    #[test]
    fn moves_the_whole_tab_with_its_layout_and_zoom() {
        let (mut app, event_hub) = split_tab_setup();
        let tab_id = app.public_tab_id(0, 1).unwrap();
        let panes = app.state.workspaces[0].tabs[1].layout.pane_ids();
        let old_pane_ids = panes
            .iter()
            .map(|&pane| app.public_pane_id(0, pane).unwrap())
            .collect::<Vec<_>>();
        let layout_before = app.state.workspaces[0].tabs[1].layout_shape_for_test();
        let target_id = app.public_workspace_id(1);

        let response = move_tab(&mut app, &tab_id, &target_id, false);

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabInfo { tab } = success.result else {
            panic!("expected tab info");
        };
        assert_eq!(tab.workspace_id, target_id);
        assert_eq!(tab.label, "split");
        assert_eq!(tab.pane_count, 2);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        let moved = &app.state.workspaces[1].tabs[1];
        assert!(moved.zoomed);
        assert_eq!(moved.layout_shape_for_test(), layout_before);
        assert_eq!(
            app.public_tab_id(1, 1).as_deref(),
            Some(tab.tab_id.as_str())
        );
        for (pane, old_id) in panes.iter().zip(&old_pane_ids) {
            let new_id = app.public_pane_id(1, *pane).unwrap();
            assert!(new_id.starts_with(&format!("{target_id}:p")));
            assert_eq!(app.parse_pane_id(old_id), Some((1, *pane)));
            assert_eq!(app.parse_pane_id(&new_id), Some((1, *pane)));
        }
        // Without focus the window stays where it was.
        assert_eq!(app.state.active, Some(0));
        for ws in &app.state.workspaces {
            ws.assert_invariants_for_test();
        }

        let events = event_hub.events_after(0);
        assert!(events.iter().any(|(_, event)| matches!(
            &event.data,
            EventData::TabClosed { tab_id: closed, .. } if closed == &tab_id
        )));
        assert!(events.iter().any(|(_, event)| matches!(
            &event.data,
            EventData::TabCreated { tab: created } if created.tab_id == tab.tab_id
        )));
        let moved_panes = events
            .iter()
            .filter_map(|(_, event)| match &event.data {
                EventData::PaneMoved {
                    previous_pane_id, ..
                } => Some(previous_pane_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(moved_panes, old_pane_ids);
    }

    #[test]
    fn focus_follows_the_tab_to_its_new_workspace() {
        let (mut app, _) = split_tab_setup();
        let tab_id = app.public_tab_id(0, 1).unwrap();
        let target_id = app.public_workspace_id(1);

        move_tab(&mut app, &tab_id, &target_id, true);

        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.selected, 1);
        assert_eq!(app.state.workspaces[1].active_tab, 1);
    }

    #[test]
    fn moving_the_last_tab_closes_its_workspace() {
        let mut target = Workspace::test_new("target");
        target.test_add_tab(Some("second"));
        let (mut app, event_hub) = app_with(vec![
            Workspace::test_new("first"),
            Workspace::test_new("source"),
            target,
        ]);
        app.state.active = Some(1);
        app.state.selected = 1;
        let tab_id = app.public_tab_id(1, 0).unwrap();
        let source_id = app.public_workspace_id(1);
        let target_id = app.public_workspace_id(2);
        let root = app.state.workspaces[1].tabs[0].root_pane;

        move_tab(&mut app, &tab_id, &target_id, true);

        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.workspaces[1].id, target_id);
        assert_eq!(app.state.workspaces[1].tabs.len(), 3);
        assert_eq!(app.state.workspaces[1].tabs[2].root_pane, root);
        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.workspaces[1].active_tab, 2);
        assert!(event_hub.events_after(0).iter().any(|(_, event)| matches!(
            &event.data,
            EventData::WorkspaceClosed { workspace_id, .. } if workspace_id == &source_id
        )));
    }

    #[test]
    fn emptied_workspace_before_the_active_one_keeps_the_active_one() {
        let (mut app, _) = app_with(vec![
            Workspace::test_new("source"),
            Workspace::test_new("target"),
            Workspace::test_new("active"),
        ]);
        app.state.active = Some(2);
        app.state.selected = 2;
        let tab_id = app.public_tab_id(0, 0).unwrap();
        let target_id = app.public_workspace_id(1);
        let active_id = app.public_workspace_id(2);

        move_tab(&mut app, &tab_id, &target_id, false);

        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.selected, 1);
        assert_eq!(app.state.workspaces[1].id, active_id);
    }

    #[test]
    fn refuses_its_own_workspace_and_unknown_targets() {
        let (mut app, _) = split_tab_setup();
        let tab_id = app.public_tab_id(0, 1).unwrap();
        let source_id = app.public_workspace_id(0);

        for (tab, workspace, code) in [
            (tab_id.as_str(), source_id.as_str(), "tab_move_failed"),
            (tab_id.as_str(), "missing", "workspace_not_found"),
            ("missing", source_id.as_str(), "tab_not_found"),
        ] {
            let response = move_tab(&mut app, tab, workspace, true);
            let error: ErrorResponse = serde_json::from_str(&response).unwrap();
            assert_eq!(error.error.code, code);
        }
        assert_eq!(app.state.workspaces[0].tabs.len(), 2);
    }
}
