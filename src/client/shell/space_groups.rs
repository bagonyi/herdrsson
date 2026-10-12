//! Space groups: a space the user named `Web/Miniteam` is drawn as `Miniteam` under a ` ▾ Web`
//! header in the one-machine sidebar. Only the window knows about groups; the server keeps the
//! full name and its own order. A click on a header folds the group (` ▸ ✓ Web`); a right-click
//! opens its menu, which folds it too or renames the group (every member's name). A space's own
//! menu moves it to a group by its name, and the window then moves it in the server's order too,
//! so that order matches the drawing. Dragging does the same (`drag`).

pub(super) mod drag;

pub(super) use drag::GroupDrag;

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

    /// Whether this is space `workspace_id`'s row.
    pub(super) fn is_space(&self, snapshot: &ClientShellSnapshot, workspace_id: &str) -> bool {
        self.entry()
            .and_then(|entry| snapshot.workspaces.get(entry.index))
            .is_some_and(|workspace| workspace.workspace_id == workspace_id)
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
    match header_with(rows, heights, gaps, body_height, target) {
        Some(header) => list_scroll_start_to_reveal(heights, gaps, body_height, start, header),
        None => start,
    }
}

/// The header to show with row `target`: its own group's, right above it, when the two fit in
/// the body together.
fn header_with(
    rows: &[SidebarRow<'_>],
    heights: &[u16],
    gaps: &[u16],
    body_height: u16,
    target: usize,
) -> Option<usize> {
    let header = target.checked_sub(1)?;
    let own = match (rows.get(header), rows.get(target)) {
        (
            Some(SidebarRow::Header { name, .. }),
            Some(SidebarRow::Space {
                group: Some(group), ..
            }),
        ) => name == group,
        _ => false,
    };
    let height = |row: usize| heights.get(row).copied().unwrap_or(1);
    let together = height(header)
        .saturating_add(gaps.get(header).copied().unwrap_or(0))
        .saturating_add(height(target));
    (own && together <= body_height).then_some(header)
}

/// The collapsed rail's spaces, all of them (no folds): in the grouped order when the sidebar
/// draws a group header, so the one-machine rail's numbers match the number keys; in server order
/// otherwise and on the multi-machine rail, which draws server order.
pub(super) fn rail_entries(
    snapshot: &ClientShellSnapshot,
    one_machine: bool,
) -> Vec<WorkspaceEntry> {
    if let Some(blocks) = one_machine.then(|| Blocks::drawn(snapshot)).flatten() {
        return blocks.entries;
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
    let groups = Blocks::of(snapshot, entries.to_vec()).groups;
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

/// A space list cut into blocks, each drawn in its head's named group. A block is what upstream
/// emits together for one worktree group (its parent rows, then its linked children) and goes
/// with its first parent's name; any other space is a block of its own.
pub(super) struct Blocks<'a> {
    snapshot: &'a ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
    /// Per entry: its block head's position, and the named group the block is drawn in.
    heads: Vec<usize>,
    groups: Vec<Option<&'a str>>,
}

impl<'a> Blocks<'a> {
    /// The full list (no folds), which the window plans its moves on.
    fn new(snapshot: &'a ClientShellSnapshot) -> Self {
        Self::of(
            snapshot,
            super::render::workspace_entries(snapshot, &HashSet::new()),
        )
    }

    /// The full list when the sidebar draws a group header for it: a block head named into a
    /// group (a linked child's own `/` name draws none).
    fn drawn(snapshot: &'a ClientShellSnapshot) -> Option<Self> {
        if !has_groups(snapshot) {
            return None;
        }
        let blocks = Self::new(snapshot);
        blocks.groups.iter().any(Option::is_some).then_some(blocks)
    }

    fn of(snapshot: &'a ClientShellSnapshot, entries: Vec<WorkspaceEntry>) -> Self {
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
        let (mut heads, mut groups) = (Vec::new(), Vec::new());
        let mut previous_parent = None;
        for (position, entry) in entries.iter().enumerate() {
            let parent = parent_key(entry);
            let continues = entry.indented || (parent.is_some() && parent == previous_parent);
            previous_parent = parent;
            let (head, group) = match (continues, heads.last(), groups.last()) {
                (true, Some(&head), Some(&group)) => (head, group),
                _ => (
                    position,
                    snapshot
                        .workspaces
                        .get(entry.index)
                        .and_then(split_name)
                        .map(|(group, _)| group),
                ),
            };
            heads.push(head);
            groups.push(group);
        }
        Self {
            snapshot,
            entries,
            heads,
            groups,
        }
    }

    fn id(&self, position: usize) -> Option<&'a str> {
        let entry = self.entries.get(position)?;
        let workspace = self.snapshot.workspaces.get(entry.index)?;
        Some(workspace.workspace_id.as_str())
    }

    fn position(&self, workspace_id: &str) -> Option<usize> {
        (0..self.entries.len()).find(|position| self.id(*position) == Some(workspace_id))
    }

    /// The named group `workspace_id` is drawn in.
    fn group(&self, workspace_id: &str) -> Option<&'a str> {
        self.groups
            .get(self.position(workspace_id)?)
            .copied()
            .flatten()
    }

    /// The spaces that move with `workspace_id` when it changes group, the space first, the rest
    /// of its block in server order (as upstream's drag moves a worktree block). None for the rest
    /// of a block (a linked child under its parent, a second parent checkout), which its head
    /// places, and for a space that is gone.
    fn block(&self, workspace_id: &str) -> Option<Vec<&'a str>> {
        let head = self
            .position(workspace_id)
            .filter(|position| self.heads.get(*position) == Some(position))?;
        let mut rest = (head + 1..self.entries.len())
            .take_while(|position| self.heads.get(*position) == Some(&head))
            .filter_map(|position| self.entries.get(position).map(|entry| entry.index))
            .collect::<Vec<_>>();
        rest.sort_unstable();
        let rest = rest
            .into_iter()
            .filter_map(|index| self.snapshot.workspaces.get(index));
        let head = self.id(head)?;
        Some(
            std::iter::once(head)
                .chain(rest.map(|workspace| workspace.workspace_id.as_str()))
                .collect(),
        )
    }

    /// The first space in server order of `workspace_id`'s block, when it heads one.
    fn block_first(&self, workspace_id: &str) -> Option<&'a str> {
        let head = self
            .position(workspace_id)
            .filter(|position| self.heads.get(*position) == Some(position))?;
        let first = (head..self.entries.len())
            .take_while(|position| self.heads.get(*position) == Some(&head))
            .filter_map(|position| self.entries.get(position).map(|entry| entry.index))
            .min()?;
        Some(self.snapshot.workspaces.get(first)?.workspace_id.as_str())
    }

    /// Every space drawn in `group` (its members and their worktree blocks), in server order.
    fn group_spaces(&self, group: &str) -> Vec<&'a str> {
        let mut indices = self
            .entries
            .iter()
            .zip(&self.groups)
            .filter(|(_, drawn_in)| **drawn_in == Some(group))
            .map(|(entry, _)| entry.index)
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|index| self.snapshot.workspaces.get(index))
            .map(|workspace| workspace.workspace_id.as_str())
            .collect()
    }
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
    Blocks::new(snapshot)
        .block(&workspace.workspace_id)
        .is_some()
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
    let blocks = Blocks::new(snapshot);
    let own = blocks.group(workspace_id);
    let mut seen = HashSet::new();
    blocks
        .groups
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

fn rename(workspace_id: &str, label: impl Into<String>) -> Method {
    Method::WorkspaceRename(WorkspaceRenameParams {
        workspace_id: workspace_id.to_owned(),
        label: label.into(),
    })
}

/// The rename that puts `workspace` into group `group`: `<group>/<short name>`.
fn join_rename(workspace: &ClientShellWorkspace, group: &str) -> Method {
    rename(
        &workspace.workspace_id,
        format!("{group}/{}", short_name(workspace)),
    )
}

/// The rename that takes `workspace` out of its group: to its short name, or `Err` with that name
/// for the user to finish when it holds a `/` itself (`app/x` would land in group `app`). None
/// for a space not named into a group.
fn leave_rename(workspace: &ClientShellWorkspace) -> Option<Result<Method, &str>> {
    let (_, short) = split_name(workspace)?;
    Some(if short.contains('/') {
        Err(short)
    } else {
        Ok(rename(&workspace.workspace_id, short))
    })
}

/// What choosing group `group` in `workspace_id`'s "Move to group" menu sends: the rename to
/// `<group>/<short name>`, then the moves that put the space after the group's last space
/// (`group_moves`).
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
    let mut methods = vec![join_rename(workspace, group)];
    methods.extend(
        group_moves(snapshot, workspace_id, Some(group), Place::AfterGroup).unwrap_or_default(),
    );
    methods
}

/// A space's name the rename box asks for on behalf of groups (`ClientRenameTarget::SpaceName`).
#[derive(Debug)]
pub(super) enum SpaceName {
    /// From "Move to group" → "New group...".
    NewGroup { workspace_id: String },
    /// A space dragged out of its group whose short name holds a `/`: nothing is sent until the
    /// name is saved, which takes it to the drop's place, before `before` or at the end.
    Dropped {
        workspace_id: String,
        before: Option<String>,
    },
}

/// The moves that go with the rename box naming `workspace_id` `label` (trimmed): into another
/// group that already has spaces, after that group's last space as "Move to group" does; out of
/// its group (to the top level or a new group), right after the group it leaves. Staying in its
/// group moves nothing.
pub(super) fn rename_moves(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    label: &str,
) -> Vec<Method> {
    let group = split_label(label).map(|(group, _)| group);
    if Blocks::new(snapshot).group(workspace_id) == group {
        return Vec::new();
    }
    group_moves(snapshot, workspace_id, group, Place::AfterGroup).unwrap_or_default()
}

/// The moves that go with naming `workspace_id` `label` (trimmed) after it was dragged out of its
/// group to before `before` (None: the end): to that place when the name keeps it at the top
/// level or starts a new group (a top-level item too); into a group that has other spaces, or
/// when that place has gone, the rename box's moves (`rename_moves`).
fn dropped_name_moves(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    label: &str,
    before: Option<&str>,
) -> Vec<Method> {
    let group = split_label(label).map(|(group, _)| group);
    let blocks = Blocks::new(snapshot);
    let joins = group
        .zip(blocks.block(workspace_id))
        .is_some_and(|(group, block)| {
            blocks
                .group_spaces(group)
                .iter()
                .any(|id| !block.contains(id))
        });
    (!joins)
        .then(|| group_moves(snapshot, workspace_id, group, Place::Before(before)))
        .flatten()
        .unwrap_or_else(|| rename_moves(snapshot, workspace_id, label))
}

/// Where `group_moves` puts a space's block in the server's order.
#[derive(Clone, Copy)]
enum Place<'a> {
    /// Right after the last other space of the group it joins; when that group has none (the top
    /// level, a new group), right after the last other space of the group it leaves; where it is
    /// when neither has one.
    AfterGroup,
    /// Right before this space, or at the end.
    Before(Option<&'a str>),
}

/// The server-order moves that go with `workspace_id` going into group `to` (None: the top level)
/// at `place`, so the server order keeps matching the drawing. A group is drawn at its first space
/// in server order, so first:
/// - the group the space is drawn in is gathered behind its first space when the moving block
///   holds that space and other spaces sit between the group's (else the group would jump to its
///   next space's place), or, when the block goes right after that group, with the block counted
///   as one of its spaces;
/// - the group it joins is gathered too, so its last space is the one drawn last.
///
/// Then the block moves. None when the space only moves with its block's head, or `place`'s space
/// is gone.
fn group_moves(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    to: Option<&str>,
    place: Place<'_>,
) -> Option<Vec<Method>> {
    let blocks = Blocks::new(snapshot);
    let block = blocks.block(workspace_id)?;
    let from = blocks.group(workspace_id);
    let left = from
        .map(|from| blocks.group_spaces(from))
        .unwrap_or_default();
    let joined = to
        .filter(|to| Some(*to) != from)
        .map(|to| blocks.group_spaces(to))
        .unwrap_or_default();
    let after_left = matches!(place, Place::AfterGroup) && joined.is_empty() && to != from;
    let mut plan = OrderPlan::new(snapshot);
    if after_left {
        plan.gather(&left, &[]);
    } else if left.first().is_some_and(|first| block.contains(first)) {
        plan.gather(&left, &block);
    }
    plan.gather(&joined, &block);
    match place {
        Place::AfterGroup => {
            let group = if after_left { &left } else { &joined };
            if let Some(last) = plan
                .order
                .iter()
                .rposition(|id| group.contains(id) && !block.contains(id))
            {
                let before = plan.order.get(last + 1).copied();
                plan.move_block(&block, before);
            }
        }
        Place::Before(before) => {
            if !plan.move_block(&block, before) {
                return None;
            }
        }
    }
    Some(plan.methods)
}

/// The move that gathers group `new` once group `old`'s spaces are renamed into it: its spaces
/// follow its first one in the server order as in the drawing (renames leave the order as it is).
/// Nothing when `new` has no spaces yet.
fn merge_moves(snapshot: &ClientShellSnapshot, old: &str, new: &str) -> Vec<Method> {
    let blocks = Blocks::new(snapshot);
    let mut merged = blocks.group_spaces(new);
    if merged.is_empty() {
        return Vec::new();
    }
    merged.extend(blocks.group_spaces(old));
    let mut plan = OrderPlan::new(snapshot);
    plan.gather(&merged, &[]);
    plan.methods
}

/// `workspace.move_block` requests planned on a copy of the server's order, each applied to it as
/// the server will, so each request is computed against the order the one before leaves.
struct OrderPlan<'a> {
    order: Vec<&'a str>,
    methods: Vec<Method>,
}

impl<'a> OrderPlan<'a> {
    fn new(snapshot: &'a ClientShellSnapshot) -> Self {
        Self {
            order: snapshot
                .workspaces
                .iter()
                .map(|workspace| workspace.workspace_id.as_str())
                .collect(),
            methods: Vec::new(),
        }
    }

    /// Moves `block` (in this order) right before `before`, or to the end. A `before` in the block
    /// gives way to the first space after it that isn't (the server refuses a `before` in the
    /// block). Nothing is requested when the order stays the same. False when `before` is gone.
    fn move_block(&mut self, block: &[&'a str], before: Option<&str>) -> bool {
        let before = match before {
            Some(before) => {
                let Some(at) = self.order.iter().position(|id| *id == before) else {
                    return false;
                };
                self.order[at..]
                    .iter()
                    .copied()
                    .find(|id| !block.contains(id))
            }
            None => None,
        };
        let mut moved = self
            .order
            .iter()
            .copied()
            .filter(|id| !block.contains(id))
            .collect::<Vec<_>>();
        let at = before
            .and_then(|before| moved.iter().position(|id| *id == before))
            .unwrap_or(moved.len());
        moved.splice(at..at, block.iter().copied());
        if moved != self.order {
            self.methods
                .push(Method::WorkspaceMoveBlock(WorkspaceMoveBlockParams {
                    workspace_ids: block.iter().map(|id| (*id).to_owned()).collect(),
                    before_workspace_id: before.map(str::to_owned),
                }));
            self.order = moved;
        }
        true
    }

    /// Gathers group `spaces` right behind its first space when other spaces sit between them
    /// (`block`, about to move, doesn't count): they go before the first space after the group's
    /// first that is neither the group's nor the block's, so the group stays where it is drawn.
    fn gather(&mut self, spaces: &[&'a str], block: &[&'a str]) {
        let in_group = |id: &&str| spaces.contains(id) && !block.contains(id);
        let (Some(first), Some(last)) = (
            self.order.iter().position(|id| spaces.contains(id)),
            self.order.iter().rposition(in_group),
        ) else {
            return;
        };
        let Some(before) = self.order[first..last]
            .iter()
            .copied()
            .find(|id| !spaces.contains(id) && !block.contains(id))
        else {
            return;
        };
        let anchor = self.order[first];
        let rest = self
            .order
            .iter()
            .copied()
            .filter(|id| *id != anchor && in_group(id))
            .collect::<Vec<_>>();
        self.move_block(&rest, Some(before));
    }
}

impl ClientShellState {
    /// Folds or unfolds a named group of the one-machine sidebar and saves the fold state.
    pub(super) fn toggle_group_fold(&mut self, group: &str, outcome: &mut ClientShellInput) {
        self.toggle_collapsed_group(&ClientEndpointId::Local, fold_key(group));
        outcome.repaint = true;
        self.persist_chrome_preferences(outcome);
    }

    /// Scrolls the one-machine sidebar to show `workspace_id` when it draws groups; false for
    /// upstream's reveal. A member of a folded group, which the next frame draws (focused or
    /// previewed) though the last one may not have, scrolls as little as needed; any other space
    /// goes to the top as upstream, under its group's header when it is the first member drawn
    /// and the two fit (`header_with`).
    pub(super) fn reveal_in_groups(&mut self, workspace_id: &str) -> bool {
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
        let Some(target) = rows
            .iter()
            .position(|row| row.is_space(snapshot, workspace_id))
        else {
            return true;
        };
        let header_folded = rows[..target].iter().rev().find_map(|row| match row {
            SidebarRow::Header { folded, .. } => Some(folded.is_some()),
            SidebarRow::Space { .. } => None,
        });
        let grouped = matches!(rows[target], SidebarRow::Space { group: Some(_), .. });
        let heights = row_heights(snapshot, &rows, &self.collapsed_groups, &self.config.spaces);
        let gaps = row_gaps(&rows, self.config.spaces.row_gap);
        let body_height = self.hits.workspace_body.height;
        self.workspace_scroll = if grouped && header_folded == Some(true) {
            reveal_start(
                &rows,
                &heights,
                &gaps,
                body_height,
                self.workspace_scroll,
                target,
            )
        } else {
            header_with(&rows, &heights, &gaps, body_height, target)
                .unwrap_or(target)
                .min(self.hits.workspace_max_scroll)
        };
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
        });
        true
    }

    /// A held header press moving off its cell starts dragging its group; true while a header
    /// press is held.
    pub(super) fn drag_group_header(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(press) = self.group_header_press.take() else {
            return false;
        };
        if point == press.start {
            self.group_header_press = Some(press);
        } else {
            self.start_header_drag(press.group, point, outcome);
        }
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
        if on_header {
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

    /// Saves the rename box of group `old` with its trimmed text `new`: renames every member,
    /// gathers the merged group when `new` has spaces already (`merge_moves`), and moves the fold
    /// with the group.
    pub(super) fn rename_space_group(
        &mut self,
        old: &str,
        new: &str,
        outcome: &mut ClientShellInput,
    ) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let renames = group_renames(snapshot, old, new);
        if renames.is_empty() {
            return;
        }
        let mut methods = renames
            .into_iter()
            .map(|(workspace_id, label)| rename(&workspace_id, label))
            .collect::<Vec<_>>();
        methods.extend(merge_moves(snapshot, old, new));
        // A refusal (machine not ready, method unsupported) would refuse the rest too: the
        // merge's move goes only after every rename.
        let mut queued = false;
        for method in methods {
            if !self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome) {
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
                self.push_in_order(methods, outcome);
            }
            // As the rename box saving the short name: the space goes right after the group.
            ClientContextMenuAction::LeaveGroup => match leave_rename(workspace) {
                Some(Ok(rename)) => self.push_rename(rename, outcome),
                Some(Err(short)) => {
                    let short = short.to_owned();
                    self.ask_space_name(ClientRenameTarget::Workspace { workspace_id }, &short);
                }
                None => {}
            },
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
                    target: ClientRenameTarget::SpaceName(SpaceName::NewGroup { workspace_id }),
                }));
            }
            _ => {}
        }
    }

    /// Sends what a rename box saves; a space's rename brings the moves that go with it
    /// (`rename_moves`) if it was queued: a refused rename moves nothing.
    pub(super) fn push_rename(&mut self, method: Method, outcome: &mut ClientShellInput) {
        let moves = match (&method, self.snapshot.as_deref()) {
            (Method::WorkspaceRename(rename), Some(snapshot)) => {
                rename_moves(snapshot, &rename.workspace_id, &rename.label)
            }
            _ => Vec::new(),
        };
        if self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome) {
            self.push_in_order(moves, outcome);
        }
    }

    /// Sends `methods` in order and stops at the first one refused (a refusal would refuse the
    /// rest too, and a move must not go without its rename); false when one was refused.
    fn push_in_order(&mut self, methods: Vec<Method>, outcome: &mut ClientShellInput) -> bool {
        methods.into_iter().all(|method| {
            self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome)
        })
    }

    /// Opens a space's rename box for `target` prefilled with `name`, the cursor at its end, for a
    /// short name the window can't use as it is (`app/x` would land in group `app`).
    fn ask_space_name(&mut self, target: ClientRenameTarget, name: &str) {
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename workspace",
            input: TextEditor::new(name, false),
            target,
        }));
    }

    /// Saves a `SpaceName` rename box with its trimmed text `label`; what is left for the box to
    /// send (`push_rename`).
    /// - "New group...": the space's rename, but nothing while the group side is still empty
    ///   (`/<short>` as prefilled, or `/`).
    /// - Dropped: sent here, the rename, then its moves (`dropped_name_moves`). Nothing for an
    ///   empty name.
    pub(super) fn save_space_name(
        &mut self,
        target: SpaceName,
        label: &str,
        outcome: &mut ClientShellInput,
    ) -> Option<Method> {
        let (workspace_id, before) = match target {
            SpaceName::NewGroup { workspace_id } => {
                return (!label.is_empty() && !label.starts_with('/'))
                    .then(|| rename(&workspace_id, label));
            }
            SpaceName::Dropped {
                workspace_id,
                before,
            } => (workspace_id, before),
        };
        let snapshot = self.snapshot.as_deref().filter(|_| !label.is_empty())?;
        let moves = dropped_name_moves(snapshot, &workspace_id, label, before.as_deref());
        let mut methods = vec![rename(&workspace_id, label)];
        methods.extend(moves);
        self.push_in_order(methods, outcome);
        None
    }

    /// Whether the one-machine sidebar draws a header for `group`.
    fn group_is_drawn(&self, group: &str) -> bool {
        self.snapshot
            .as_deref()
            .and_then(Blocks::drawn)
            .is_some_and(|blocks| blocks.groups.contains(&Some(group)))
    }
}
