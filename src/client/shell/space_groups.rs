//! Space groups: a space the user named `Web/Miniteam` is drawn as `Miniteam` under a ` ▾ Web`
//! header in the one-machine sidebar. Only the window knows about groups; the server keeps the
//! full name and its own order.

use super::render::put_segment;
use super::*;

/// Columns a group's member rows are indented by.
const MEMBER_INDENT: u16 = 2;

/// One row of the one-machine sidebar's space list.
pub(super) enum SidebarRow<'a> {
    /// A group's header. Not a space, so it never gets a workspace hit.
    Header(&'a str),
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
            Self::Header(_) => None,
            Self::Space { entry, .. } => Some(entry),
        }
    }

    /// Rows `row_gap` goes before: headers, and spaces outside groups and worktree blocks.
    pub(super) fn starts_top_level(&self) -> bool {
        match self {
            Self::Header(_) => true,
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
    let (group, name) = workspace.label.split_once('/')?;
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
/// before each group's members.
pub(super) fn sidebar_rows(
    snapshot: &ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
) -> Vec<SidebarRow<'_>> {
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
    let mut rows = Vec::with_capacity(entries.len());
    let mut previous = None;
    for (entry, group) in grouped(snapshot, &entries) {
        if let Some(name) = group.filter(|_| group != previous) {
            rows.push(SidebarRow::Header(name));
        }
        previous = group;
        // A worktree child keeps its own name unless that name is in its block's group.
        let label = group.and_then(|group| {
            let (own, short) = split_name(snapshot.workspaces.get(entry.index)?)?;
            (own == group).then_some(short)
        });
        rows.push(SidebarRow::Space {
            entry,
            group,
            label,
        });
    }
    rows
}

/// The sidebar row (headers counted, the unit of its scroll) to put at the top to show a space:
/// the space's own row, or its group's header when it is the group's first member. `entries` is
/// the navigation list.
pub(super) fn reveal_row(
    snapshot: &ClientShellSnapshot,
    entries: Vec<WorkspaceEntry>,
    workspace_id: &str,
) -> Option<usize> {
    let rows = sidebar_rows(snapshot, entries);
    let position = rows.iter().position(|row| {
        row.entry()
            .and_then(|entry| snapshot.workspaces.get(entry.index))
            .is_some_and(|workspace| workspace.workspace_id == workspace_id)
    })?;
    Some(header_above(&rows, position).unwrap_or(position))
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
    matches!(rows.get(above), Some(SidebarRow::Header(_))).then_some(above)
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

/// The named group of each entry's block. A block is what upstream emits together for one
/// worktree group (its parent rows, then its linked children) and goes with its first parent's
/// name; any other space is a block of its own.
fn block_groups<'a>(
    snapshot: &'a ClientShellSnapshot,
    entries: &[WorkspaceEntry],
) -> Vec<Option<&'a str>> {
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
    let mut groups = Vec::with_capacity(entries.len());
    let mut group = None;
    let mut previous_parent = None;
    for entry in entries {
        let parent = parent_key(entry);
        let continues_block = entry.indented || (parent.is_some() && parent == previous_parent);
        if !continues_block {
            group = snapshot
                .workspaces
                .get(entry.index)
                .and_then(split_name)
                .map(|(group, _)| group);
        }
        previous_parent = parent;
        groups.push(group);
    }
    groups
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

/// A group's header row, ` ▾ Web`, bold like the multi-machine sidebar's machine rows.
pub(super) fn render_header(buffer: &mut Buffer, rect: Rect, name: &str, palette: &Palette) {
    let style = Style::default()
        .fg(palette.text)
        .add_modifier(Modifier::BOLD);
    // Leave the last two columns free, as space rows do.
    let right = rect.right().saturating_sub(2);
    let x = put_segment(buffer, rect.x.saturating_add(1), rect.y, right, "▾ ", style);
    let name = crate::ui::truncate_end(name, usize::from(right.saturating_sub(x)));
    put_segment(buffer, x, rect.y, right, &name, style);
}
