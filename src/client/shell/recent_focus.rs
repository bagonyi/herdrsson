//! Fork: the window's focus history behind the `last_tab` and `last_workspace` keys.

use super::*;
use crate::input::KeybindAction;

/// Enough history to skip over closed tabs and spaces without growing forever.
const RECENT_FOCUS_LIMIT: usize = 64;

/// The tabs the window has shown, most recent first.
#[derive(Default)]
pub(super) struct RecentFocusHistory {
    entries: Vec<RecentFocus>,
}

/// A tab the window has shown, and its machine: each server numbers its spaces from the same
/// start. A restarted server keeps its space and tab IDs (they are saved with the session), so
/// they need no boot ID beside them, and a window on Local alone closes with its server anyway.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecentFocus {
    endpoint_id: ClientEndpointId,
    workspace_id: String,
    tab_id: String,
}

impl RecentFocus {
    fn same_workspace(&self, other: &Self) -> bool {
        self.endpoint_id == other.endpoint_id && self.workspace_id == other.workspace_id
    }
}

impl ClientShellState {
    fn current_focus(&self) -> Option<RecentFocus> {
        let snapshot = self.snapshot.as_deref()?;
        Some(RecentFocus {
            endpoint_id: self.active_endpoint_id.clone(),
            workspace_id: snapshot.focused_workspace_id.clone()?,
            tab_id: snapshot.focused_tab_id.clone()?,
        })
    }

    /// Moves the focused tab to the front of the history, most recent first.
    pub(super) fn note_recent_focus(&mut self) {
        let Some(focus) = self.current_focus() else {
            return;
        };
        let entries = &mut self.recent_focus.entries;
        if entries.first() == Some(&focus) {
            return;
        }
        entries.retain(|recent| recent != &focus);
        entries.insert(0, focus);
        entries.truncate(RECENT_FOCUS_LIMIT);
    }

    /// `last_tab` goes back to the tab shown before this one in the same space; `last_workspace`
    /// to the space shown before this one, on any machine. Pressing either again comes back.
    /// Closed tabs and spaces and machines not online are skipped.
    pub(super) fn handle_recent_focus(
        &mut self,
        action: KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !matches!(
            action,
            KeybindAction::LastTab | KeybindAction::LastWorkspace
        ) {
            return false;
        }
        let current = self.current_focus();
        let last_tab = action == KeybindAction::LastTab;
        let Some(target) = self
            .recent_focus
            .entries
            .iter()
            .filter(|recent| match &current {
                Some(current) if last_tab => {
                    recent.same_workspace(current) && recent.tab_id != current.tab_id
                }
                Some(current) => !recent.same_workspace(current),
                // Nothing on screen, say after a machine's last space closed: any space will do.
                None => !last_tab,
            })
            .find(|recent| self.recent_focus_exists(recent, last_tab))
            .cloned()
        else {
            return true;
        };
        if last_tab {
            self.push_endpoint_method(
                crate::api::schema::Method::TabFocus(crate::api::schema::TabTarget {
                    tab_id: target.tab_id,
                }),
                outcome,
            );
        } else {
            if !self.multi_endpoint_active() {
                self.reveal_workspace(&target.workspace_id);
            }
            self.focus_or_activate(
                target.endpoint_id,
                ClientEndpointFocusTarget::Workspace(target.workspace_id),
                outcome,
            );
        }
        true
    }

    fn recent_focus_exists(&self, recent: &RecentFocus, tab: bool) -> bool {
        let snapshot = if recent.endpoint_id == self.active_endpoint_id {
            self.snapshot.as_deref()
        } else {
            self.endpoints
                .iter()
                .find(|endpoint| {
                    endpoint.endpoint_id == recent.endpoint_id
                        && endpoint.status == ClientEndpointStatus::Online
                })
                .and_then(|endpoint| endpoint.snapshot.as_deref())
        };
        snapshot.is_some_and(|snapshot| {
            if tab {
                snapshot.tabs.iter().any(|candidate| {
                    candidate.workspace_id == recent.workspace_id
                        && candidate.tab_id == recent.tab_id
                })
            } else {
                snapshot
                    .workspaces
                    .iter()
                    .any(|candidate| candidate.workspace_id == recent.workspace_id)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{snapshot, surface};
    use super::*;
    use crate::api::schema::Method;
    use crate::client::endpoint::{ProfileId, SavedSshEndpoint};
    use crate::config::Config;

    /// Spaces ws_1 (tabs ws_1:t1, ws_1:t2, ws_1:t3) and ws_2 (ws_2:t1, ws_2:t2).
    fn focused(revision: u64, workspace_id: &str, tab_id: &str) -> Box<ClientShellSnapshot> {
        let mut snapshot = snapshot();
        let workspace = snapshot.workspaces[0].clone();
        let tab = snapshot.tabs[0].clone();
        snapshot.revision = revision;
        snapshot.workspaces = ["ws_1", "ws_2"]
            .into_iter()
            .map(|id| ClientShellWorkspace {
                workspace_id: id.into(),
                focused: id == workspace_id,
                ..workspace.clone()
            })
            .collect();
        snapshot.tabs = [("ws_1", 3), ("ws_2", 2)]
            .into_iter()
            .flat_map(|(workspace_id, count)| (1..=count).map(move |n| (workspace_id, n)))
            .map(|(workspace_id, n)| crate::protocol::ClientShellTab {
                tab_id: format!("{workspace_id}:t{n}"),
                workspace_id: workspace_id.into(),
                number: n,
                ..tab.clone()
            })
            .collect();
        snapshot.focused_workspace_id = Some(workspace_id.into());
        snapshot.focused_tab_id = Some(tab_id.into());
        Box::new(snapshot)
    }

    fn press(state: &mut ClientShellState, action: KeybindAction) -> Vec<ClientShellAction> {
        let mut outcome = ClientShellInput::default();
        state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
        outcome.actions
    }

    fn focused_tab(actions: &[ClientShellAction]) -> &str {
        match actions {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                Method::TabFocus(target) => &target.tab_id,
                method => panic!("expected a tab focus, got {method:?}"),
            },
            actions => panic!("expected one endpoint request, got {actions:?}"),
        }
    }

    fn focused_workspace(actions: &[ClientShellAction]) -> &str {
        match actions {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                Method::WorkspaceFocus(target) => &target.workspace_id,
                method => panic!("expected a workspace focus, got {method:?}"),
            },
            actions => panic!("expected one endpoint request, got {actions:?}"),
        }
    }

    #[test]
    fn last_tab_and_last_workspace_toggle_between_the_two_latest() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        assert!(press(&mut state, KeybindAction::LastTab).is_empty());

        let visits = [
            ("ws_1", "ws_1:t1"),
            ("ws_1", "ws_1:t2"),
            ("ws_2", "ws_2:t1"),
            ("ws_2", "ws_2:t2"),
            ("ws_1", "ws_1:t2"),
        ];
        for (revision, (workspace_id, tab_id)) in (1..).zip(visits) {
            state.set_snapshot(focused(revision, workspace_id, tab_id));
        }

        // Each space remembers its own previous tab.
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t1"
        );
        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_2"
        );

        state.set_snapshot(focused(6, "ws_1", "ws_1:t1"));
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t2"
        );

        state.set_snapshot(focused(7, "ws_2", "ws_2:t2"));
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_2:t1"
        );
        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_1"
        );
    }

    #[test]
    fn closed_tabs_and_spaces_are_skipped() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(focused(1, "ws_1", "ws_1:t1"));
        state.set_snapshot(focused(2, "ws_1", "ws_1:t2"));
        state.set_snapshot(focused(3, "ws_1", "ws_1:t3"));
        let mut closed = focused(4, "ws_1", "ws_1:t3");
        closed.tabs.retain(|tab| tab.tab_id != "ws_1:t2");
        state.set_snapshot(closed);
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t1"
        );

        state.set_snapshot(focused(5, "ws_2", "ws_2:t1"));
        let mut closed = focused(6, "ws_1", "ws_1:t1");
        closed
            .workspaces
            .retain(|workspace| workspace.workspace_id != "ws_2");
        closed.tabs.retain(|tab| tab.workspace_id != "ws_2");
        state.set_snapshot(closed);
        assert!(press(&mut state, KeybindAction::LastWorkspace).is_empty());

        // A restarted server restores its spaces and tabs with the same IDs: they are still
        // remembered, the space that came back too.
        let mut restarted = focused(1, "ws_1", "ws_1:t3");
        restarted.boot_id = "boot-2".into();
        state.set_snapshot(restarted);
        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_1:t1"
        );
        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_2"
        );
    }

    #[test]
    fn the_keys_work_in_zen_mode() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(focused(1, "ws_1", "ws_1:t1"));
        state.set_snapshot(focused(2, "ws_2", "ws_2:t1"));
        state.set_snapshot(focused(3, "ws_2", "ws_2:t2"));
        press(&mut state, KeybindAction::ZenMode);
        assert!(state.zen_on());

        assert_eq!(
            focused_tab(&press(&mut state, KeybindAction::LastTab)),
            "ws_2:t1"
        );
        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_1"
        );
    }

    #[test]
    fn last_workspace_goes_back_into_a_folded_group() {
        // ws_1 is "Web/a", drawn under the group header Web; ws_2 is "api".
        let grouped = |revision, workspace_id, tab_id| {
            let mut snapshot = focused(revision, workspace_id, tab_id);
            for (workspace, label) in snapshot.workspaces.iter_mut().zip(["Web/a", "api"]) {
                workspace.label = label.into();
                workspace.custom_label = true;
            }
            snapshot
        };
        let drawn = |state: &mut ClientShellState, revision| {
            let mut projected_surface = surface();
            projected_surface.projection_revision = revision;
            state.set_pane_surface(projected_surface);
            state.compose(106, 30).expect("composed frame");
            state
                .hits
                .workspaces
                .iter()
                .any(|hit| hit.workspace_id == "ws_1")
        };
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(grouped(1, "ws_1", "ws_1:t1"));
        state.set_snapshot(grouped(2, "ws_2", "ws_2:t1"));
        state.toggle_group_fold("Web", &mut ClientShellInput::default());
        assert!(!drawn(&mut state, 2), "the folded group hides ws_1");

        assert_eq!(
            focused_workspace(&press(&mut state, KeybindAction::LastWorkspace)),
            "ws_1"
        );

        // Once focused, the folded group shows it.
        state.set_snapshot(grouped(3, "ws_1", "ws_1:t1"));
        assert!(drawn(&mut state, 3));
    }

    #[test]
    fn last_workspace_switches_back_to_another_machine() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        let profile = SavedSshEndpoint {
            id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
            label: "Build".into(),
            target: "dev@build.example".into(),
            session: "agents".into(),
            enabled: true,
        };
        let remote = ClientEndpointId::Ssh(profile.id.clone());
        state.set_endpoint_catalog(&[profile]);
        state.set_endpoint_status(&ClientEndpointId::Local, ClientEndpointStatus::Online);
        state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
        state.set_snapshot(focused(1, "ws_1", "ws_1:t1"));
        // The machine has spaces ws_1 and ws_2 too: only Local's ws_1 is the last space.
        state.set_endpoint_snapshot(&remote, focused(1, "ws_2", "ws_2:t1"));
        assert!(state.activate_endpoint_projection(&remote));

        let actions = press(&mut state, KeybindAction::LastWorkspace);
        assert!(
            matches!(
                actions.as_slice(),
                [ClientShellAction::ActivateEndpoint {
                    endpoint_id: ClientEndpointId::Local,
                    target: Some(ClientEndpointFocusTarget::Workspace(workspace_id)),
                }] if workspace_id == "ws_1"
            ),
            "{actions:?}"
        );

        // With no space left on screen, any remembered space will do.
        let mut empty = focused(2, "ws_2", "ws_2:t1");
        empty.workspaces.clear();
        empty.tabs.clear();
        empty.focused_workspace_id = None;
        empty.focused_tab_id = None;
        state.set_endpoint_snapshot(&remote, empty);
        assert!(press(&mut state, KeybindAction::LastTab).is_empty());
        let actions = press(&mut state, KeybindAction::LastWorkspace);
        assert!(
            matches!(
                actions.as_slice(),
                [ClientShellAction::ActivateEndpoint {
                    endpoint_id: ClientEndpointId::Local,
                    target: Some(ClientEndpointFocusTarget::Workspace(workspace_id)),
                }] if workspace_id == "ws_1"
            ),
            "{actions:?}"
        );
    }
}
