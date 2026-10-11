//! Space groups: a space the user named `Web/Miniteam` is drawn as `Miniteam` under a ` ▾ Web`
//! header in the one-machine sidebar. Only the window knows about groups; the server keeps the
//! full name and its own order. A click on a header folds the group (` ▸ ✓ Web`); a right-click
//! opens its menu, which folds it too or renames the group (every member's name). A space's own
//! menu moves it to a group by its name, and the window then moves it in the server's order too,
//! so that order matches the drawing.

use super::render::put_segment;
use super::*;
use crate::api::schema::{AgentStatus, Method, WorkspaceMoveBlockParams, WorkspaceRenameParams};

/// Columns a group's member rows are indented by.
const MEMBER_INDENT: u16 = 2;

/// A named group's key in the window's `collapsed_groups` set, next to the worktree folds (whose
/// keys are absolute paths, so the two never collide).
pub(super) fn fold_key(group: &str) -> String {
    format!("group:{group}")
}

/// A group header's place in the sidebar, for clicks. Never a `WorkspaceHit`.
pub(super) struct GroupHeaderHit {
    pub(super) rect: Rect,
    pub(super) group: String,
}

/// A left press on a group header, waiting for its release.
pub(super) struct GroupHeaderPress {
    group: String,
    start: (u16, u16),
    dragged: bool,
}

/// One row of the one-machine sidebar's space list.
pub(super) enum SidebarRow<'a> {
    /// A group's header, with its members' rolled-up status when the group is folded. Not a
    /// space, so it never gets a workspace hit.
    Header {
        name: &'a str,
        folded: Option<AgentStatus>,
    },
    /// A space row, with the named group it is drawn in and the short name it shows there.
    Space {
        entry: WorkspaceEntry,
        group: Option<&'a str>,
        label: Option<&'a str>,
    },
}

impl SidebarRow<'_> {
    pub(super) fn entry(&self) -> Option<&WorkspaceEntry> {
        match self {
            Self::Header { .. } => None,
            Self::Space { entry, .. } => Some(entry),
        }
    }

    /// Rows `row_gap` goes before: headers, and spaces outside groups and worktree blocks.
    pub(super) fn starts_top_level(&self) -> bool {
        match self {
            Self::Header { .. } => true,
            Self::Space { entry, group, .. } => group.is_none() && !entry.indented,
        }
    }
}

/// The group and short name of a space the user named `Group/name`: split at the first `/`,
/// both sides trimmed and non-empty. Automatic labels never group.
pub(super) fn split_name(workspace: &ClientShellWorkspace) -> Option<(&str, &str)> {
    if !workspace.custom_label {
        return None;
    }
    split_label(&workspace.label)
}

/// The group and short name a user-set name `label` gives (see `split_name`).
fn split_label(label: &str) -> Option<(&str, &str)> {
    let (group, name) = label.split_once('/')?;
    let (group, name) = (group.trim(), name.trim());
    (!group.is_empty() && !name.is_empty()).then_some((group, name))
}

fn has_groups(snapshot: &ClientShellSnapshot) -> bool {
    snapshot
        .workspaces
        .iter()
        .any(|workspace| split_name(workspace).is_some())
}

/// The navigation order: `entries` (server order with worktree blocks) with each group's blocks
/// gathered at its first block's position.
pub(super) fn group_entries(
    snapshot: &ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
) -> Vec<WorkspaceEntry> {
    if !has_groups(snapshot) {
        return entries;
    }
    grouped(snapshot, &entries)
        .into_iter()
        .map(|(entry, _)| entry)
        .collect()
}

/// The drawn rows: `entries` (upstream's or already grouped) in navigation order, with a header
/// before each group's members. A folded group keeps only its focused member and `preview` (the
/// Navigate mode selection), so navigation can still visit every space.
pub(super) fn sidebar_rows<'a>(
    snapshot: &'a ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
    collapsed_groups: &HashSet<String>,
    preview: Option<&str>,
) -> Vec<SidebarRow<'a>> {
    if !has_groups(snapshot) {
        return entries
            .into_iter()
            .map(|entry| SidebarRow::Space {
                entry,
                group: None,
                label: None,
            })
            .collect();
    }
    let grouped = grouped(snapshot, &entries);
    let mut rows = Vec::with_capacity(grouped.len());
    // A worktree child keeps its own name unless that name is in its block's group.
    let space_row = |entry: WorkspaceEntry, group: Option<&'a str>| {
        let label = group.and_then(|group| {
            let (own, short) = split_name(snapshot.workspaces.get(entry.index)?)?;
            (own == group).then_some(short)
        });
        SidebarRow::Space {
            entry,
            group,
            label,
        }
    };
    // Gathering made each group one run of entries.
    for run in grouped.chunk_by(|(_, left), (_, right)| left == right) {
        let Some(name) = run.first().and_then(|(_, group)| *group) else {
            rows.extend(run.iter().map(|(entry, _)| space_row(*entry, None)));
            continue;
        };
        let members = run.iter().map(|(entry, _)| *entry);
        if !collapsed_groups.contains(&fold_key(name)) {
            rows.push(SidebarRow::Header { name, folded: None });
            rows.extend(members.map(|entry| space_row(entry, Some(name))));
            continue;
        }
        let members = members.collect::<Vec<_>>();
        rows.push(SidebarRow::Header {
            name,
            folded: Some(rolled_up_status(snapshot, &members, collapsed_groups)),
        });
        rows.extend(
            folded_members(snapshot, &members, preview)
                .into_iter()
                .map(|entry| space_row(entry, Some(name))),
        );
    }
    rows
}

/// A folded group's status: its most urgent member's, a folded worktree block counting its
/// hidden children through its parent row.
fn rolled_up_status(
    snapshot: &ClientShellSnapshot,
    members: &[WorkspaceEntry],
    collapsed_groups: &HashSet<String>,
) -> AgentStatus {
    members
        .iter()
        .filter_map(|entry| snapshot.workspaces.get(entry.index))
        .map(|workspace| {
            super::sidebar::displayed_workspace_status(snapshot, workspace, collapsed_groups)
        })
        .max_by_key(|status| status_priority(*status))
        .unwrap_or(AgentStatus::Unknown)
}

/// The members a folded group still shows: the focused space and the Navigate preview. A worktree
/// child comes with its block's parent rows, and is drawn as the last child shown.
fn folded_members(
    snapshot: &ClientShellSnapshot,
    members: &[WorkspaceEntry],
    preview: Option<&str>,
) -> Vec<WorkspaceEntry> {
    let worktree_key = |entry: &WorkspaceEntry| {
        snapshot
            .workspaces
            .get(entry.index)
            .and_then(|workspace| workspace.worktree.as_ref())
            .map(|worktree| worktree.key.as_str())
    };
    let mut shown = vec![false; members.len()];
    for (position, entry) in members.iter().enumerate() {
        let Some(workspace) = snapshot.workspaces.get(entry.index) else {
            continue;
        };
        if !workspace.focused && preview != Some(workspace.workspace_id.as_str()) {
            continue;
        }
        shown[position] = true;
        let Some(key) = worktree_key(entry).filter(|_| entry.indented) else {
            continue;
        };
        // The parent rows are the unindented rows of the repository right above its children.
        for (parent, _) in members[..position]
            .iter()
            .enumerate()
            .rev()
            .skip_while(|(_, above)| above.indented)
            .take_while(|(_, above)| !above.indented && worktree_key(above) == Some(key))
        {
            shown[parent] = true;
        }
    }
    let mut visible = members
        .iter()
        .zip(shown)
        .filter_map(|(entry, shown)| shown.then_some(*entry))
        .collect::<Vec<_>>();
    for position in 0..visible.len() {
        let next_is_child = visible.get(position + 1).is_some_and(|next| next.indented);
        if let Some(entry) = visible.get_mut(position).filter(|entry| entry.indented) {
            entry.last_child = !next_is_child;
        }
    }
    visible
}

/// The sidebar row (headers counted, the unit of its scroll) to put at the top to show a space:
/// the space's own row, or its group's header when it is the group's first drawn member.
/// `entries` is the navigation list. The space is counted as shown even in a folded group, as it
/// is about to be focused or previewed.
pub(super) fn reveal_row(
    snapshot: &ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
    collapsed_groups: &HashSet<String>,
    workspace_id: &str,
) -> Option<usize> {
    let rows = sidebar_rows(snapshot, entries, collapsed_groups, Some(workspace_id));
    let position = rows.iter().position(|row| {
        row.entry()
            .and_then(|entry| snapshot.workspaces.get(entry.index))
            .is_some_and(|workspace| workspace.workspace_id == workspace_id)
    })?;
    Some(header_above(&rows, position).unwrap_or(position))
}

/// Each row's height in lines, as the sidebar draws it.
pub(super) fn row_heights(
    snapshot: &ClientShellSnapshot,
    rows: &[SidebarRow<'_>],
    collapsed_groups: &HashSet<String>,
    spaces: &SpacesSidebarConfig,
) -> Vec<u16> {
    use super::sidebar::{displayed_workspace_status, labeled_workspace_rows};

    rows.iter()
        .map(|row| {
            let SidebarRow::Space { entry, label, .. } = row else {
                return 1;
            };
            snapshot
                .workspaces
                .get(entry.index)
                .map(|workspace| {
                    labeled_workspace_rows(
                        workspace,
                        *label,
                        displayed_workspace_status(snapshot, workspace, collapsed_groups),
                        entry.indented,
                        spaces,
                    )
                    .len()
                    .max(1)
                    .min(u16::MAX as usize) as u16
                })
                .unwrap_or(1)
        })
        .collect()
}

/// The `row_gap` lines after each row: before each top-level row that follows.
pub(super) fn row_gaps(rows: &[SidebarRow<'_>], row_gap: u16) -> Vec<u16> {
    rows.iter()
        .enumerate()
        .map(|(index, _)| {
            rows.get(index + 1)
                .map_or(0, |next| u16::from(next.starts_top_level()) * row_gap)
        })
        .collect()
}

/// The scroll start that shows row `target`, and its group's header above it as well when the
/// two fit in the body together.
pub(super) fn reveal_start(
    rows: &[SidebarRow<'_>],
    heights: &[u16],
    gaps: &[u16],
    body_height: u16,
    start: usize,
    target: usize,
) -> usize {
    use super::scroll::list_scroll_start_to_reveal;

    let start = list_scroll_start_to_reveal(heights, gaps, body_height, start, target);
    let Some(header) = header_above(rows, target) else {
        return start;
    };
    let height = |row: usize| heights.get(row).copied().unwrap_or(1);
    let together = height(header)
        .saturating_add(gaps.get(header).copied().unwrap_or(0))
        .saturating_add(height(target));
    if together > body_height {
        return start;
    }
    list_scroll_start_to_reveal(heights, gaps, body_height, start, header)
}

fn header_above(rows: &[SidebarRow<'_>], position: usize) -> Option<usize> {
    let above = position.checked_sub(1)?;
    matches!(rows.get(above), Some(SidebarRow::Header { .. })).then_some(above)
}

/// The collapsed rail's spaces, all of them (no folds): in the grouped order when any space has
/// a group, so the one-machine rail's numbers match the number keys; in server order otherwise
/// and on the multi-machine rail, which draws server order.
pub(super) fn rail_entries(
    snapshot: &ClientShellSnapshot,
    one_machine: bool,
) -> Vec<WorkspaceEntry> {
    if one_machine && has_groups(snapshot) {
        return super::render::workspace_entries(snapshot, &HashSet::new());
    }
    snapshot
        .workspaces
        .iter()
        .enumerate()
        .map(|(index, _)| WorkspaceEntry {
            index,
            indented: false,
            last_child: false,
        })
        .collect()
}

pub(super) fn rail_workspaces(snapshot: &ClientShellSnapshot) -> Vec<&ClientShellWorkspace> {
    rail_entries(snapshot, true)
        .iter()
        .filter_map(|entry| snapshot.workspaces.get(entry.index))
        .collect()
}

/// Each entry with its block's group, the blocks of a group gathered at the first one's place.
/// Gathering is idempotent, so already grouped entries come back unchanged.
fn grouped<'a>(
    snapshot: &'a ClientShellSnapshot,
    entries: &[WorkspaceEntry],
) -> Vec<(WorkspaceEntry, Option<&'a str>)> {
    let groups = block_groups(snapshot, entries);
    let mut members = HashMap::<&str, Vec<WorkspaceEntry>>::new();
    for (entry, group) in entries.iter().zip(&groups) {
        if let Some(group) = group {
            members.entry(group).or_default().push(*entry);
        }
    }
    let mut ordered = Vec::with_capacity(entries.len());
    for (entry, group) in entries.iter().zip(groups) {
        match group {
            None => ordered.push((*entry, None)),
            Some(group) => {
                if let Some(members) = members.remove(group) {
                    ordered.extend(members.into_iter().map(|member| (member, Some(group))));
                }
            }
        }
    }
    ordered
}

/// The named group of each entry's block: its head's (see `block_heads`).
fn block_groups<'a>(
    snapshot: &'a ClientShellSnapshot,
    entries: &[WorkspaceEntry],
) -> Vec<Option<&'a str>> {
    let mut group = None;
    entries
        .iter()
        .zip(block_heads(snapshot, entries))
        .map(|(entry, head)| {
            if head {
                group = snapshot
                    .workspaces
                    .get(entry.index)
                    .and_then(split_name)
                    .map(|(group, _)| group);
            }
            group
        })
        .collect()
}

/// Whether each entry heads a block. A block is what upstream emits together for one worktree
/// group (its parent rows, then its linked children) and goes with its first parent's name; any
/// other space is a block of its own.
fn block_heads(snapshot: &ClientShellSnapshot, entries: &[WorkspaceEntry]) -> Vec<bool> {
    let linked_keys = snapshot
        .workspaces
        .iter()
        .filter_map(|workspace| workspace.worktree.as_ref())
        .filter(|worktree| worktree.is_linked_worktree)
        .map(|worktree| worktree.key.as_str())
        .collect::<HashSet<_>>();
    // A non-linked space whose repository has linked worktrees heads (or shares) a block.
    let parent_key = |entry: &WorkspaceEntry| {
        snapshot
            .workspaces
            .get(entry.index)?
            .worktree
            .as_ref()
            .filter(|worktree| {
                !entry.indented
                    && !worktree.is_linked_worktree
                    && linked_keys.contains(worktree.key.as_str())
            })
            .map(|worktree| worktree.key.as_str())
    };
    let mut previous_parent = None;
    entries
        .iter()
        .map(|entry| {
            let parent = parent_key(entry);
            let continues_block = entry.indented || (parent.is_some() && parent == previous_parent);
            previous_parent = parent;
            !continues_block
        })
        .collect()
}

/// The part of a space row drawn with its content: indented when the row is in a group.
pub(super) fn content_area(rect: Rect, row: &SidebarRow<'_>) -> Rect {
    if !matches!(row, SidebarRow::Space { group: Some(_), .. }) {
        return rect;
    }
    Rect::new(
        rect.x.saturating_add(MEMBER_INDENT),
        rect.y,
        rect.width.saturating_sub(MEMBER_INDENT),
        rect.height,
    )
}

/// Gives the indent columns left of `content` the background its row got (focused, selected,
/// dragged), so a member's highlight spans the full row like any other row's.
pub(super) fn fill_indent(buffer: &mut Buffer, rect: Rect, content: Rect) {
    if content.is_empty() {
        return;
    }
    for y in rect.y..rect.bottom() {
        let Some(background) = buffer.cell((content.x, y)).map(|cell| cell.bg) else {
            continue;
        };
        for x in rect.x..content.x {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_bg(background);
            }
        }
    }
}

/// A group's header row, ` ▾ Web`, bold like the multi-machine sidebar's machine rows; folded,
/// ` ▸ ✓ Web` with the rolled-up status icon.
pub(super) fn render_header(
    buffer: &mut Buffer,
    rect: Rect,
    name: &str,
    folded: Option<AgentStatus>,
    config: &ClientShellConfig,
) -> GroupHeaderHit {
    let palette = &config.palette;
    let style = Style::default()
        .fg(palette.text)
        .add_modifier(Modifier::BOLD);
    // Leave the last two columns free, as space rows do.
    let (y, right) = (rect.y, rect.right().saturating_sub(2));
    let chevron = if folded.is_some() { "▸ " } else { "▾ " };
    let mut x = put_segment(buffer, rect.x.saturating_add(1), y, right, chevron, style);
    if let Some(status) = folded {
        let icon_style = Style::default().fg(status_color(status, palette));
        let icon = status_icon(status, config.status_indicators);
        x = put_segment(buffer, x, y, right, icon, icon_style);
        x = put_segment(buffer, x, y, right, " ", icon_style);
    }
    let name_text = crate::ui::truncate_end(name, usize::from(right.saturating_sub(x)));
    put_segment(buffer, x, y, right, &name_text, style);
    GroupHeaderHit {
        rect,
        group: name.to_owned(),
    }
}

/// The items of a group header's right-click menu.
pub(super) fn header_menu_items(collapsed: bool) -> Vec<ClientContextMenuItem> {
    use ClientContextMenuAction as Action;

    let item = |label: &'static str, action| ClientContextMenuItem {
        label: label.into(),
        action,
    };
    vec![
        item(
            if collapsed { "Expand" } else { "Collapse" },
            Action::ToggleGroup,
        ),
        item("Rename group...", Action::Rename),
    ]
}

/// The renames that rename group `old` to `new` (trimmed): `(workspace id, "<new>/<short name>")`
/// for every space whose own name is in `old`. A linked worktree child drawn in the group under
/// its block's head, but named otherwise, keeps its name. None when `new` is empty, contains `/`
/// or is `old`.
pub(super) fn group_renames(
    snapshot: &ClientShellSnapshot,
    old: &str,
    new: &str,
) -> Vec<(String, String)> {
    if new.is_empty() || new.contains('/') || new == old {
        return Vec::new();
    }
    snapshot
        .workspaces
        .iter()
        .filter_map(|workspace| {
            let (group, short) = split_name(workspace)?;
            (group == old).then(|| (workspace.workspace_id.clone(), format!("{new}/{short}")))
        })
        .collect()
}

/// Moves a folded group's fold to its new name; false when `old` was not folded. Renamed onto a
/// group that is folded already, the merged group stays folded.
pub(super) fn move_fold(collapsed_groups: &mut HashSet<String>, old: &str, new: &str) -> bool {
    if !collapsed_groups.remove(&fold_key(old)) {
        return false;
    }
    collapsed_groups.insert(fold_key(new));
    true
}

/// A space menu's items with "Move to group" after "Rename" when its target offers it.
pub(super) fn with_move_to_group(
    target: &ClientContextMenuTarget,
    mut items: Vec<ClientContextMenuItem>,
) -> Vec<ClientContextMenuItem> {
    if matches!(
        target,
        ClientContextMenuTarget::Workspace {
            move_to_group: true,
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
                label: "Move to group".into(),
                action: ClientContextMenuAction::MoveToGroup,
            },
        );
    }
    items
}

/// Whether a space's menu offers "Move to group": every space but the rest of a worktree block
/// (linked children under their parent, second parent checkouts), which its head places.
pub(super) fn can_move_to_group(
    snapshot: &ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
) -> bool {
    let entries = super::render::workspace_entries(snapshot, &HashSet::new());
    moving_block(snapshot, &entries, &workspace.workspace_id).is_some()
}

/// The items of "Move to group"'s menu: the other groups, "New group..." and, for a space named
/// into a group, "No group".
pub(super) fn move_menu_items(groups: &[String], in_group: bool) -> Vec<ClientContextMenuItem> {
    use ClientContextMenuAction as Action;

    let mut items = groups
        .iter()
        .enumerate()
        .map(|(index, group)| ClientContextMenuItem {
            label: group.clone().into(),
            action: Action::JoinGroup(index),
        })
        .collect::<Vec<_>>();
    items.push(ClientContextMenuItem {
        label: "New group...".into(),
        action: Action::NewGroup,
    });
    if in_group {
        items.push(ClientContextMenuItem {
            label: "No group".into(),
            action: Action::LeaveGroup,
        });
    }
    items
}

/// The groups "Move to group" offers `workspace_id`: every group with a header, in drawn order,
/// except the one the space is drawn in.
fn other_groups(snapshot: &ClientShellSnapshot, workspace_id: &str) -> Vec<String> {
    let entries = super::render::workspace_entries(snapshot, &HashSet::new());
    let groups = block_groups(snapshot, &entries);
    let own = drawn_group(snapshot, &entries, &groups, workspace_id);
    let mut seen = HashSet::new();
    groups
        .into_iter()
        .flatten()
        .filter(|group| Some(*group) != own && seen.insert(*group))
        .map(str::to_owned)
        .collect()
}

/// The name a space shows inside a group: its short name when it is named into one, otherwise its
/// whole name (user-set or automatic).
fn short_name(workspace: &ClientShellWorkspace) -> &str {
    split_name(workspace).map_or_else(|| workspace.label.trim(), |(_, short)| short)
}

/// What choosing group `group` in `workspace_id`'s "Move to group" menu sends: the rename to
/// `<group>/<short name>`, then the move that puts the space after the group's last space.
pub(super) fn join_group_methods(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    group: &str,
) -> Vec<Method> {
    let Some(workspace) = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == workspace_id)
    else {
        return Vec::new();
    };
    let mut methods = vec![Method::WorkspaceRename(WorkspaceRenameParams {
        workspace_id: workspace_id.to_owned(),
        label: format!("{group}/{}", short_name(workspace)),
    })];
    methods.extend(join_group_move(snapshot, workspace_id, group));
    methods
}

/// What the rename box "New group..." opened sends for its trimmed text `label`: the space's
/// rename, but nothing while the group side is still empty (`/<short>` as prefilled, or `/`).
pub(super) fn new_group_rename(workspace_id: String, label: &str) -> Option<Method> {
    (!label.is_empty() && !label.starts_with('/')).then(|| {
        Method::WorkspaceRename(WorkspaceRenameParams {
            workspace_id,
            label: label.to_owned(),
        })
    })
}

/// The move that goes with the rename box naming `workspace_id` `label` (trimmed): into another
/// group that already has spaces, it moves after that group's last space as "Move to group" does.
/// A new group, leaving a group or staying in one keeps the server position.
pub(super) fn rename_move(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    label: &str,
) -> Option<Method> {
    let (group, _) = split_label(label)?;
    let entries = super::render::workspace_entries(snapshot, &HashSet::new());
    let groups = block_groups(snapshot, &entries);
    if drawn_group(snapshot, &entries, &groups, workspace_id) == Some(group) {
        return None;
    }
    join_group_move(snapshot, workspace_id, group)
}

/// The move that puts `workspace_id`, with its block, right after the last space drawn in `group`
/// (its members and their worktree blocks) in server order, so the server order matches the
/// drawing. None when nothing else is drawn in the group, when the space moves only with its
/// block's head, and when its block is there already.
fn join_group_move(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    group: &str,
) -> Option<Method> {
    let entries = super::render::workspace_entries(snapshot, &HashSet::new());
    let block = moving_block(snapshot, &entries, workspace_id)?;
    let server_order = || {
        snapshot
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.as_str())
    };
    let last = entries
        .iter()
        .zip(block_groups(snapshot, &entries))
        .filter(|(_, drawn_in)| *drawn_in == Some(group))
        .filter(|(entry, _)| {
            snapshot
                .workspaces
                .get(entry.index)
                .is_some_and(|workspace| !block.contains(&workspace.workspace_id.as_str()))
        })
        .map(|(entry, _)| entry.index)
        .max()?;
    let before = server_order()
        .skip(last + 1)
        .find(|workspace_id| !block.contains(workspace_id));
    // The order the server makes of the move; nothing to send when it is the current one.
    let mut moved = server_order()
        .filter(|workspace_id| !block.contains(workspace_id))
        .collect::<Vec<_>>();
    let at = before
        .and_then(|before| {
            moved
                .iter()
                .position(|workspace_id| *workspace_id == before)
        })
        .unwrap_or(moved.len());
    moved.splice(at..at, block.iter().copied());
    if moved.into_iter().eq(server_order()) {
        return None;
    }
    Some(Method::WorkspaceMoveBlock(WorkspaceMoveBlockParams {
        workspace_ids: block.into_iter().map(str::to_owned).collect(),
        before_workspace_id: before.map(str::to_owned),
    }))
}

/// The spaces that move with `workspace_id` when it changes group, the space first: a worktree
/// block's head brings every other space of its repository (its block), as upstream's drag moves
/// it; any other space moves alone. None for the rest of a block (a linked child under its parent,
/// a second parent checkout), which its head places, and for a space that is gone. `entries` is
/// the full list (no folds).
fn moving_block<'a>(
    snapshot: &'a ClientShellSnapshot,
    entries: &[WorkspaceEntry],
    workspace_id: &str,
) -> Option<Vec<&'a str>> {
    let (index, workspace) = snapshot
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, workspace)| workspace.workspace_id == workspace_id)?;
    let heads_block = entries
        .iter()
        .zip(block_heads(snapshot, entries))
        .any(|(entry, head)| entry.index == index && head);
    if !heads_block {
        return None;
    }
    let parent_key = workspace
        .worktree
        .as_ref()
        .filter(|worktree| !worktree.is_linked_worktree)
        .map(|worktree| worktree.key.as_str())
        .filter(|key| {
            snapshot.workspaces.iter().any(|other| {
                other
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| worktree.is_linked_worktree && worktree.key == *key)
            })
        });
    let mut block = vec![workspace.workspace_id.as_str()];
    if let Some(key) = parent_key {
        block.extend(
            snapshot
                .workspaces
                .iter()
                .filter(|other| {
                    other.workspace_id != workspace_id
                        && other
                            .worktree
                            .as_ref()
                            .is_some_and(|worktree| worktree.key == key)
                })
                .map(|other| other.workspace_id.as_str()),
        );
    }
    Some(block)
}

/// The named group `workspace_id` is drawn in: its block's. `groups` is `block_groups` of
/// `entries`.
fn drawn_group<'a>(
    snapshot: &ClientShellSnapshot,
    entries: &[WorkspaceEntry],
    groups: &[Option<&'a str>],
    workspace_id: &str,
) -> Option<&'a str> {
    entries
        .iter()
        .zip(groups)
        .find(|(entry, _)| {
            snapshot
                .workspaces
                .get(entry.index)
                .is_some_and(|workspace| workspace.workspace_id == workspace_id)
        })
        .and_then(|(_, group)| *group)
}

impl ClientShellState {
    /// Folds or unfolds a named group of the one-machine sidebar and saves the fold state.
    pub(super) fn toggle_group_fold(&mut self, group: &str, outcome: &mut ClientShellInput) {
        self.toggle_collapsed_group(&ClientEndpointId::Local, fold_key(group));
        outcome.repaint = true;
        self.persist_chrome_preferences(outcome);
    }

    /// Scrolls the one-machine sidebar as little as needed to show `workspace_id` when it is a
    /// member of a folded group, which the next frame draws (focused or previewed) though the
    /// last one may not have: its row is predicted from the rows as they will be drawn, with its
    /// header when it is the first member drawn. False for any other space, which keeps
    /// upstream's reveal.
    pub(super) fn reveal_folded_member(&mut self, workspace_id: &str) -> bool {
        if self.endpoints.len() != 1 || self.sidebar_collapsed || self.mobile_layout_active() {
            return false;
        }
        let Some(snapshot) = self
            .snapshot
            .as_deref()
            .filter(|snapshot| has_groups(snapshot))
        else {
            return false;
        };
        let entries = self.navigation_workspace_entries(snapshot);
        let rows = sidebar_rows(
            snapshot,
            entries,
            &self.collapsed_groups,
            Some(workspace_id),
        );
        let Some(target) = rows.iter().position(|row| {
            row.entry()
                .and_then(|entry| snapshot.workspaces.get(entry.index))
                .is_some_and(|workspace| workspace.workspace_id == workspace_id)
        }) else {
            return false;
        };
        let header_folded = rows[..target].iter().rev().find_map(|row| match row {
            SidebarRow::Header { folded, .. } => Some(folded.is_some()),
            SidebarRow::Space { .. } => None,
        });
        let grouped = matches!(rows[target], SidebarRow::Space { group: Some(_), .. });
        if !grouped || header_folded != Some(true) {
            return false;
        }
        let heights = row_heights(snapshot, &rows, &self.collapsed_groups, &self.config.spaces);
        let gaps = row_gaps(&rows, self.config.spaces.row_gap);
        self.workspace_scroll = reveal_start(
            &rows,
            &heights,
            &gaps,
            self.hits.workspace_body.height,
            self.workspace_scroll,
            target,
        );
        true
    }

    /// Records a left press on a group header; true when there was one under the pointer.
    pub(super) fn press_group_header(&mut self, point: (u16, u16)) -> bool {
        let Some(hit) = self
            .hits
            .group_headers
            .iter()
            .find(|hit| super::contains(hit.rect, point))
        else {
            return false;
        };
        self.group_header_press = Some(GroupHeaderPress {
            group: hit.group.clone(),
            start: point,
            dragged: false,
        });
        true
    }

    /// Notes a held header press moving off its cell; true while a header press is held.
    pub(super) fn drag_group_header(&mut self, point: (u16, u16)) -> bool {
        let Some(press) = self.group_header_press.as_mut() else {
            return false;
        };
        press.dragged |= point != press.start;
        true
    }

    /// Ends a header press: released on the same header without a drag, it toggles the fold.
    /// True when a header press was held.
    pub(super) fn release_group_header(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(press) = self.group_header_press.take() else {
            return false;
        };
        let on_header = self
            .hits
            .group_headers
            .iter()
            .any(|hit| hit.group == press.group && super::contains(hit.rect, point));
        if on_header && !press.dragged {
            self.toggle_group_fold(&press.group, outcome);
        }
        true
    }

    /// Opens the right-click menu of the group header under `point`, if there is one.
    pub(super) fn open_group_header_menu(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.sidebar_collapsed {
            return false;
        }
        let Some(name) = self
            .hits
            .group_headers
            .iter()
            .find(|hit| super::contains(hit.rect, point))
            .map(|hit| hit.group.clone())
        else {
            return false;
        };
        // A left press held on a header must not fold it under the menu when released.
        self.group_header_press = None;
        let collapsed = self.collapsed_groups.contains(&fold_key(&name));
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::SpaceGroup { name, collapsed },
            x: point.0,
            y: point.1,
            highlighted: 0,
        }));
        outcome.repaint = true;
        true
    }

    /// Runs an item of a group header's menu.
    pub(super) fn activate_group_header_action(
        &mut self,
        name: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        match action {
            // The group may have gone while its menu was open; a fold key for it would be stale.
            ClientContextMenuAction::ToggleGroup if self.group_is_drawn(&name) => {
                self.toggle_group_fold(&name, outcome)
            }
            ClientContextMenuAction::Rename => {
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename group",
                    input: TextEditor::new(&name, false),
                    target: ClientRenameTarget::SpaceGroup { name },
                }));
            }
            _ => {}
        }
    }

    /// Saves the rename box of group `old` with its trimmed text `new`: renames every member and
    /// moves the fold with the group.
    pub(super) fn rename_space_group(
        &mut self,
        old: &str,
        new: &str,
        outcome: &mut ClientShellInput,
    ) {
        let renames = self
            .snapshot
            .as_deref()
            .map(|snapshot| group_renames(snapshot, old, new))
            .unwrap_or_default();
        let mut queued = false;
        for (workspace_id, label) in renames {
            // A refusal (machine not ready, method unsupported) would refuse the rest too.
            if !self.push_endpoint_method_with_kind(
                Method::WorkspaceRename(WorkspaceRenameParams {
                    workspace_id,
                    label,
                }),
                PendingEndpointKind::Generic,
                outcome,
            ) {
                break;
            }
            queued = true;
        }
        if queued && move_fold(&mut self.collapsed_groups, old, new) {
            self.persist_chrome_preferences(outcome);
        }
    }

    /// Replaces a space's menu with its "Move to group" menu, at the same place.
    pub(super) fn open_move_to_group_menu(&mut self, workspace_id: String, x: u16, y: u16) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
            .filter(|workspace| can_move_to_group(snapshot, workspace))
        else {
            return;
        };
        let in_group = split_name(workspace).is_some();
        let groups = other_groups(snapshot, &workspace_id);
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::MoveToGroup {
                workspace_id,
                groups,
                in_group,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    /// Runs an item of a space's "Move to group" menu.
    pub(super) fn activate_move_to_group_action(
        &mut self,
        workspace_id: String,
        groups: &[String],
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        let Some(workspace) = self.snapshot.as_deref().and_then(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
        }) else {
            return;
        };
        match action {
            ClientContextMenuAction::JoinGroup(index) => {
                let Some(group) = groups.get(index) else {
                    return;
                };
                let methods = self
                    .snapshot
                    .as_deref()
                    .map(|snapshot| join_group_methods(snapshot, &workspace_id, group))
                    .unwrap_or_default();
                for method in methods {
                    // The move goes only after its rename, which a refusal would refuse too.
                    if !self.push_endpoint_method_with_kind(
                        method,
                        PendingEndpointKind::Generic,
                        outcome,
                    ) {
                        break;
                    }
                }
            }
            ClientContextMenuAction::LeaveGroup => {
                let Some((_, short)) = split_name(workspace) else {
                    return;
                };
                // `app/x` would land in group `app`: the user picks the name instead.
                if short.contains('/') {
                    let input = TextEditor::new(short, false);
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename workspace",
                        input,
                        target: ClientRenameTarget::Workspace { workspace_id },
                    }));
                    return;
                }
                let label = short.to_owned();
                self.push_endpoint_method(
                    Method::WorkspaceRename(WorkspaceRenameParams {
                        workspace_id,
                        label,
                    }),
                    outcome,
                );
            }
            ClientContextMenuAction::NewGroup => {
                // The group name goes before the slash: the cursor starts there.
                let mut input = TextEditor::new(&format!("/{}", short_name(workspace)), false);
                input.handle_key(&crate::input::TerminalKey::new(
                    KeyCode::Home,
                    crossterm::event::KeyModifiers::NONE,
                ));
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename workspace",
                    input,
                    target: ClientRenameTarget::SpaceNewGroup { workspace_id },
                }));
            }
            _ => {}
        }
    }

    /// The move that goes with saving the rename box of a space (`rename_move`).
    pub(super) fn rename_group_move(
        &self,
        target: &ClientRenameTarget,
        label: &str,
    ) -> Option<Method> {
        let (ClientRenameTarget::Workspace { workspace_id }
        | ClientRenameTarget::SpaceNewGroup { workspace_id }) = target
        else {
            return None;
        };
        rename_move(self.snapshot.as_deref()?, workspace_id, label)
    }

    /// Sends what a rename box saves, then its group move (`rename_group_move`) if that was
    /// queued: a refused rename moves nothing.
    pub(super) fn push_rename(
        &mut self,
        method: Method,
        group_move: Option<Method>,
        outcome: &mut ClientShellInput,
    ) {
        let queued =
            self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome);
        if let Some(group_move) = group_move.filter(|_| queued) {
            self.push_endpoint_method(group_move, outcome);
        }
    }

    /// Whether the one-machine sidebar draws a header for `group`.
    fn group_is_drawn(&self, group: &str) -> bool {
        self.snapshot.as_deref().is_some_and(|snapshot| {
            let entries = self.navigation_workspace_entries(snapshot);
            block_groups(snapshot, &entries).contains(&Some(group))
        })
    }
}
