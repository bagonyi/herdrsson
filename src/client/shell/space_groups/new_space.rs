//! A space made from the window (the new space key or button) goes next to the space it was made
//! from. The server adds a new space at the end, so once it answers, the window moves the space
//! with `workspace.move_block`, as it does for a space it puts into a group. In a group, the box
//! that asks for the new space's name (`ui.prompt_new_workspace_name`) starts with `<group>/`.

use super::*;
use crate::api::schema::ResponseResult;

/// Where a space made from `source` and named `label` (None: shown by its folder name, which never
/// groups) goes in the server's order: the space to put it before, or None to leave it at the end.
/// - In `source`'s group, or with both at the top level: right after `source` (and the rest of its
///   worktree block).
/// - In another group that has spaces: after that group's last space, as "Move to group" does.
/// - Out of `source`'s group (the top level, a new group): right after that group.
pub(in crate::client::shell) fn new_space_before(
    snapshot: &ClientShellSnapshot,
    source: &str,
    label: Option<&str>,
) -> Option<String> {
    let blocks = Blocks::new(snapshot);
    let from = blocks.group(source);
    let to = label.and_then(split_label).map(|(group, _)| group);
    let order = snapshot
        .workspaces
        .iter()
        .map(|workspace| workspace.workspace_id.as_str())
        .collect::<Vec<_>>();
    let joined = to
        .filter(|to| Some(*to) != from)
        .map(|to| blocks.group_spaces(to))
        .unwrap_or_default();
    if let Some(last) = order.iter().rposition(|id| joined.contains(id)) {
        return order.get(last + 1).map(|id| (*id).to_owned());
    }
    // A group or a worktree block is drawn at its first space in server order, the rest of it
    // gathered behind, so the space goes before the first space after that one which isn't in it.
    let unit = match from {
        Some(from) if to != Some(from) => blocks.group_spaces(from),
        _ => blocks.whole_block(source)?,
    };
    let first = order.iter().position(|id| unit.contains(id))?;
    order[first..]
        .iter()
        .find(|id| !unit.contains(id))
        .map(|id| (*id).to_owned())
}

/// The name the new space box saves for its trimmed text `text`, `folder` being the name the space
/// shows without one of its own: none for an empty box or `folder` itself (upstream's rule), and
/// `<group>/<folder>` for a bare `<group>/` (the box's text in a group), as "Move to group" names
/// a space without a name of its own.
pub(in crate::client::shell) fn new_space_label(text: &str, folder: &str) -> Option<String> {
    match text.strip_suffix('/').map(str::trim) {
        Some(group) if !group.is_empty() && !group.contains('/') => {
            Some(format!("{group}/{folder}"))
        }
        _ => (!text.is_empty() && text != folder).then(|| text.to_owned()),
    }
}

impl<'a> Blocks<'a> {
    /// Every space of the block `workspace_id` is in (`block` of its head).
    fn whole_block(&self, workspace_id: &str) -> Option<Vec<&'a str>> {
        let head = *self.heads.get(self.position(workspace_id)?)?;
        self.block(self.id(head)?)
    }
}

impl ClientShellState {
    /// The new space box's text: `<group>/` with the cursor at its end when `source`, the space it
    /// is made from, is drawn in a group; else upstream's `suggested_name`, replaced on typing.
    pub(in crate::client::shell) fn new_space_input(
        &self,
        source: Option<&str>,
        suggested_name: &str,
    ) -> TextEditor {
        let group = self
            .snapshot
            .as_deref()
            .zip(source)
            .and_then(|(snapshot, source)| Blocks::new(snapshot).group(source).map(str::to_owned));
        match group {
            Some(group) => TextEditor::new(&format!("{group}/"), false),
            None => TextEditor::new(suggested_name, true),
        }
    }

    /// Sends a new space's `workspace.create`, with where the space goes (`new_space_before`) for
    /// its answer (`place_new_space`).
    pub(in crate::client::shell) fn push_new_space(
        &mut self,
        method: Method,
        outcome: &mut ClientShellInput,
    ) {
        let before = match (&method, self.snapshot.as_deref()) {
            (Method::WorkspaceCreate(params), Some(snapshot)) => params
                .source_workspace_id
                .as_deref()
                .and_then(|source| new_space_before(snapshot, source, params.label.as_deref())),
            _ => None,
        };
        let kind = before.map_or(PendingEndpointKind::Generic, |before| {
            PendingEndpointKind::NewSpace { before }
        });
        self.push_endpoint_method_with_kind(method, kind, outcome);
    }

    /// The answer to a new space's `workspace.create`: moves the space before `before`, unless that
    /// space has gone meanwhile, and has it revealed there (`reveal_placed_new_space`). Whether to
    /// repaint, as for any other answer.
    pub(in crate::client::shell) fn place_new_space(
        &mut self,
        before: String,
        result: Result<ResponseResult, ClientShellEndpointError>,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let workspace_id = match result {
            Ok(ResponseResult::WorkspaceCreated { workspace, .. }) => workspace.workspace_id,
            Ok(_) => return false,
            Err(_) => return true,
        };
        let there = self.snapshot.as_deref().is_some_and(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .any(|workspace| workspace.workspace_id == before)
        });
        if there
            && self.push_endpoint_method_with_kind(
                Method::WorkspaceMoveBlock(WorkspaceMoveBlockParams {
                    workspace_ids: vec![workspace_id.clone()],
                    before_workspace_id: Some(before.clone()),
                }),
                PendingEndpointKind::Generic,
                outcome,
            )
        {
            self.placed_new_space = Some((workspace_id, before));
        }
        false
    }

    /// Reveals a moved new space (`placed_new_space`) once `snapshot`, the window's next one, shows
    /// it before the space it went before. The sidebar scrolled to it at the end, where the server
    /// added it, and scrolls again by itself only when the focus changes. Until then it waits (the
    /// new space may not be in a snapshot yet); a gone `before` ends the wait.
    pub(in crate::client::shell) fn reveal_placed_new_space(
        &mut self,
        snapshot: &ClientShellSnapshot,
    ) {
        let Some((moved, before)) = &self.placed_new_space else {
            return;
        };
        let position = |id: &str| {
            snapshot
                .workspaces
                .iter()
                .position(|workspace| workspace.workspace_id == id)
        };
        match (position(moved), position(before)) {
            (Some(moved), Some(before)) if moved < before => self.reveal_focused_workspace = true,
            (_, Some(_)) => return,
            (_, None) => {}
        }
        self.placed_new_space = None;
    }
}
