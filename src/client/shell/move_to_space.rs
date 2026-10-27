//! Fork: a tab's right-click menu moves the tab to another space. "Move to space" opens a second
//! menu of the other spaces in sidebar order; choosing one sends `tab.move_to_workspace`, which
//! takes the tab to the end of that space with its layout and running panes, and shows it there.

use super::*;
use crate::api::schema::{Method, TabMoveToWorkspaceParams};

/// A tab menu's items with "Move to space" after "Rename" when its target offers it.
pub(super) fn with_move_to_space(
    target: &ClientContextMenuTarget,
    mut items: Vec<ClientContextMenuItem>,
) -> Vec<ClientContextMenuItem> {
    if matches!(
        target,
        ClientContextMenuTarget::Tab {
            move_to_space: true,
            ..
        }
    ) {
        let after_rename = items
            .iter()
            .position(|item| item.action == ClientContextMenuAction::Rename)
            .map_or(0, |rename| rename + 1);
        items.insert(
            after_rename,
            ClientContextMenuItem {
                label: "Move to space".into(),
                action: ClientContextMenuAction::MoveToSpace,
            },
        );
    }
    items
}

/// Whether a tab in `workspace_id` has another space to move to.
pub(super) fn can_move_to_space(snapshot: &ClientShellSnapshot, workspace_id: &str) -> bool {
    snapshot
        .workspaces
        .iter()
        .any(|workspace| workspace.workspace_id != workspace_id)
}

/// The items of "Move to space"'s menu: the other spaces by their full names.
pub(super) fn move_menu_items(spaces: &[(String, String)]) -> Vec<ClientContextMenuItem> {
    spaces
        .iter()
        .enumerate()
        .map(|(index, (_, label))| ClientContextMenuItem {
            label: label.clone().into(),
            action: ClientContextMenuAction::JoinSpace(index),
        })
        .collect()
}

/// The spaces "Move to space" offers a tab in `workspace_id`: every other space, in sidebar
/// order, with its ID and full name (`Web/Miniteam`, not the `Miniteam` its group row shows).
fn other_spaces(snapshot: &ClientShellSnapshot, workspace_id: &str) -> Vec<(String, String)> {
    super::space_groups::rail_workspaces(snapshot)
        .into_iter()
        .filter(|workspace| workspace.workspace_id != workspace_id)
        .map(|workspace| (workspace.workspace_id.clone(), workspace.label.clone()))
        .collect()
}

impl ClientShellState {
    /// Replaces a tab's menu with its "Move to space" menu, at the same place.
    pub(super) fn open_move_to_space_menu(&mut self, tab_id: String, x: u16, y: u16) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(tab) = snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id) else {
            return;
        };
        let spaces = other_spaces(snapshot, &tab.workspace_id);
        if spaces.is_empty() {
            return;
        }
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::MoveToSpace { tab_id, spaces },
            x,
            y,
            highlighted: 0,
        }));
    }

    /// Runs an item of a tab's "Move to space" menu: the tab moves to that space, and the window
    /// follows it there.
    pub(super) fn activate_move_to_space_action(
        &mut self,
        tab_id: String,
        spaces: &[(String, String)],
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        let ClientContextMenuAction::JoinSpace(index) = action else {
            return;
        };
        let Some((workspace_id, _)) = spaces.get(index) else {
            return;
        };
        self.push_endpoint_method(
            Method::TabMoveToWorkspace(TabMoveToWorkspaceParams {
                tab_id,
                workspace_id: workspace_id.clone(),
                focus: true,
            }),
            outcome,
        );
    }
}
