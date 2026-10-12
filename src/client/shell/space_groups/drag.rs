//! Dragging in a sidebar that draws groups. A space dropped on a group's header joins the group at
//! its end (on its own group's header it goes to the group's first place); dropped between a
//! group's members it joins the group there; dropped between top-level rows it leaves its group. A
//! header drags its whole group between top-level rows. As in the menu, joining or leaving is a
//! rename and the place is the server's order (`workspace.move_block`, `group_moves`). Without a
//! group header drawn (or with several machines) upstream's drag runs unchanged.

use super::*;

/// A drag in a sidebar that draws groups (`ClientChromeDrag::SpaceGroups`).
#[derive(Debug)]
pub(in crate::client::shell) struct GroupDrag {
    pub(in crate::client::shell) source: GroupDragSource,
    pub(in crate::client::shell) target: Option<GroupDropTarget>,
}

/// What a drag carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::client::shell) enum GroupDragSource {
    /// A space, with the rest of its worktree block when it heads one (`moving_block`).
    Space(String),
    /// A group's header: every space drawn in the group.
    Header(String),
}

/// Where a drag would drop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::client::shell) enum GroupDropTarget {
    /// On a group's header: the space joins the group at its end.
    Header(String),
    /// A gap, its line drawn on `row`: between `group`'s members, or between top-level rows
    /// (`None`). `before` is the first space in server order of the row below it, None at the end.
    /// `beside` marks a gap right next to what is dragged, in its own group: nothing changes.
    Gap {
        group: Option<String>,
        before: Option<String>,
        row: u16,
        beside: bool,
    },
}

impl GroupDrag {
    /// What upstream's sidebar draws of the drag: the dragged space's background and a top-level
    /// gap's full-width line.
    pub(in crate::client::shell) fn upstream_view(&self) -> (Option<&str>, Option<u16>) {
        let dragged = match &self.source {
            GroupDragSource::Space(workspace_id) => Some(workspace_id.as_str()),
            GroupDragSource::Header(_) => None,
        };
        let line = match &self.target {
            Some(GroupDropTarget::Gap {
                group: None, row, ..
            }) => Some(*row),
            _ => None,
        };
        (dragged, line)
    }
}

/// The group drag among the window's drags, for the sidebar to draw.
pub(in crate::client::shell) fn group_drag(drag: Option<&ClientChromeDrag>) -> Option<&GroupDrag> {
    match drag {
        Some(ClientChromeDrag::SpaceGroups(drag)) => Some(drag),
        _ => None,
    }
}

/// Draws what upstream's sidebar doesn't of a group drag, over the space list `body`: the dragged
/// header's background, the header a space would join, and a gap's line between a group's members
/// (from the member indent, so it reads "inside").
pub(in crate::client::shell) fn render_group_drag(
    buffer: &mut Buffer,
    body: Rect,
    headers: &[GroupHeaderHit],
    drag: Option<&GroupDrag>,
    palette: &Palette,
) {
    let Some(drag) = drag else {
        return;
    };
    let header = |group: &str| {
        headers
            .iter()
            .find(|hit| hit.group == group)
            .map(|hit| hit.rect)
    };
    if let GroupDragSource::Header(group) = &drag.source {
        if let Some(rect) = header(group) {
            buffer.set_style(rect, Style::default().bg(palette.surface1));
        }
    }
    match &drag.target {
        Some(GroupDropTarget::Header(group)) => {
            if let Some(rect) = header(group) {
                buffer.set_style(rect, Style::default().bg(drop_background(palette)));
            }
        }
        // The rows upstream draws its line on: from the blank line above the list to its end.
        Some(GroupDropTarget::Gap {
            group: Some(_),
            row,
            ..
        }) if *row >= body.y.saturating_sub(1) && *row < body.bottom() => {
            let x = body.x.saturating_add(MEMBER_INDENT);
            let width = body.right().saturating_sub(x);
            super::super::render::put_text(
                buffer,
                x,
                *row,
                width,
                &"─".repeat(usize::from(width)),
                Style::default().fg(palette.accent),
            );
        }
        _ => {}
    }
}

/// A header a space would join: the Navigate-mode cursor's background, or the focused row's where
/// the theme leaves that unset (as the sidebar's own fallback).
fn drop_background(palette: &Palette) -> ratatui::style::Color {
    if palette.selection_bg == ratatui::style::Color::Reset {
        palette.active_row_bg
    } else {
        palette.selection_bg
    }
}

/// What a drag needs to know of the full list (no folds): each block head's group and its block's
/// first space in server order, and each group's first space in server order.
struct Blocks<'a> {
    heads: HashMap<&'a str, (Option<&'a str>, &'a str)>,
    groups: HashMap<&'a str, &'a str>,
}

fn blocks<'a>(snapshot: &'a ClientShellSnapshot, entries: &[WorkspaceEntry]) -> Blocks<'a> {
    let groups = block_groups(snapshot, entries);
    // (head's index, the block's smallest index, its group)
    let mut heads = Vec::<(usize, usize, Option<&str>)>::new();
    let mut group_first = HashMap::<&str, usize>::new();
    for ((entry, head), group) in entries
        .iter()
        .zip(block_heads(snapshot, entries))
        .zip(groups)
    {
        if head {
            heads.push((entry.index, entry.index, group));
        } else if let Some((_, first, _)) = heads.last_mut() {
            *first = (*first).min(entry.index);
        }
        if let Some(group) = group {
            let first = group_first.entry(group).or_insert(entry.index);
            *first = (*first).min(entry.index);
        }
    }
    let id = |index: usize| {
        snapshot
            .workspaces
            .get(index)
            .map(|workspace| workspace.workspace_id.as_str())
    };
    Blocks {
        heads: heads
            .into_iter()
            .filter_map(|(head, first, group)| Some((id(head)?, (group, id(first)?))))
            .collect(),
        groups: group_first
            .into_iter()
            .filter_map(|(group, first)| Some((group, id(first)?)))
            .collect(),
    }
}

/// What dropping space `workspace_id` on `target` sends, and a short name to ask the user to finish.
/// Changing group renames the space: `<group>/<short name>` into a group, its short name at the
/// top level. A short name with a `/` (`app/x`) would put it in another group, so it isn't sent
/// but asked for. Then come the moves that put its block before the row below the gap
/// (`group_moves`). Nothing at all when that row's space has gone since.
fn space_drop(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    target: &GroupDropTarget,
) -> (Vec<Method>, Option<String>) {
    let entries = super::super::render::workspace_entries(snapshot, &HashSet::new());
    let Some(workspace) = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == workspace_id)
    else {
        return Default::default();
    };
    let own = drawn_group(
        snapshot,
        &entries,
        &block_groups(snapshot, &entries),
        workspace_id,
    );
    let (group, before) = match target {
        GroupDropTarget::Header(group) if own != Some(group.as_str()) => {
            return (join_group_methods(snapshot, workspace_id, group), None);
        }
        GroupDropTarget::Gap {
            group,
            before,
            beside: false,
            ..
        } => (group.as_deref(), before.as_deref()),
        _ => return Default::default(),
    };
    let Some(moves) = group_moves(snapshot, workspace_id, group, Place::Before(before)) else {
        return Default::default();
    };
    let rename = |label: String| {
        Method::WorkspaceRename(WorkspaceRenameParams {
            workspace_id: workspace_id.to_owned(),
            label,
        })
    };
    let mut methods = Vec::new();
    let mut ask = None;
    if group != own {
        match (group, split_name(workspace)) {
            (Some(group), _) => methods.push(rename(format!("{group}/{}", short_name(workspace)))),
            (None, Some((_, short))) if short.contains('/') => ask = Some(short.to_owned()),
            (None, Some((_, short))) => methods.push(rename(short.to_owned())),
            (None, None) => {}
        }
    }
    methods.extend(moves);
    (methods, ask)
}

/// What dropping group `group`'s header on `target` sends: every space drawn in it moves, in server
/// order, before the row below a top-level gap.
fn header_drop(
    snapshot: &ClientShellSnapshot,
    group: &str,
    target: &GroupDropTarget,
) -> Vec<Method> {
    let GroupDropTarget::Gap {
        group: None,
        before,
        beside: false,
        ..
    } = target
    else {
        return Vec::new();
    };
    let entries = super::super::render::workspace_entries(snapshot, &HashSet::new());
    let spaces = group_spaces(snapshot, &entries, group);
    let mut plan = OrderPlan::new(snapshot);
    if !spaces.is_empty() {
        plan.move_block(&spaces, before.as_deref());
    }
    plan.methods
}

/// A candidate gap while the drawn rows are walked.
struct Gap<'a> {
    group: Option<&'a str>,
    before: Option<&'a str>,
    row: u16,
    beside: bool,
}

impl Gap<'_> {
    fn target(self) -> GroupDropTarget {
        GroupDropTarget::Gap {
            group: self.group.map(str::to_owned),
            before: self.before.map(str::to_owned),
            row: self.row,
            beside: self.beside,
        }
    }
}

/// A drawn row of the space list, from its hits.
enum DrawnRow<'a> {
    Header(&'a str),
    Space(&'a str),
}

impl ClientShellState {
    /// The snapshot and its full list (no folds) when the sidebar draws a group header: one
    /// machine, expanded, a block head named into a group (a linked child's own `/` name draws none).
    fn grouped_sidebar(&self) -> Option<(&ClientShellSnapshot, Vec<WorkspaceEntry>)> {
        if self.endpoints.len() != 1 || self.sidebar_collapsed || self.mobile_layout_active() {
            return None;
        }
        let snapshot = self
            .snapshot
            .as_deref()
            .filter(|snapshot| has_groups(snapshot))?;
        let entries = super::super::render::workspace_entries(snapshot, &HashSet::new());
        block_groups(snapshot, &entries)
            .iter()
            .any(Option::is_some)
            .then_some((snapshot, entries))
    }

    /// Every row of the space list as the last frame drew them, scrolled out ones too: folds and
    /// the Navigate preview (or a pending focus highlight) as `compose` passes them.
    fn drawn_rows<'a>(&self, snapshot: &'a ClientShellSnapshot) -> Vec<SidebarRow<'a>> {
        let target = if self.mode == ClientShellMode::Navigate {
            self.navigate_workspace_id.as_ref()
        } else {
            self.pending_workspace_highlight
                .as_ref()
                .map(|pending| &pending.target)
        };
        let preview = target
            .filter(|target| {
                target.endpoint_id == self.active_endpoint_id
                    && self.navigation_target_valid(target)
            })
            .map(|target| target.workspace_id.as_str());
        sidebar_rows(
            snapshot,
            self.navigation_workspace_entries(snapshot),
            &self.collapsed_groups,
            preview,
        )
    }

    /// Starts dragging space `workspace_id` when the sidebar draws groups, as upstream does once a
    /// drop target is under the pointer; false when it doesn't, for upstream's drag. Only a block's
    /// head drags: a second parent checkout stays where its head puts it.
    pub(in crate::client::shell) fn start_space_drag_in_groups(
        &mut self,
        workspace_id: &str,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some((snapshot, entries)) = self.grouped_sidebar() else {
            return false;
        };
        if moving_block(snapshot, &entries, workspace_id).is_none() {
            return true;
        }
        let source = GroupDragSource::Space(workspace_id.to_owned());
        if let Some(target) = self.group_drop_target_at(&source, point) {
            self.chrome_drag = Some(ClientChromeDrag::SpaceGroups(GroupDrag {
                source,
                target: Some(target),
            }));
            outcome.repaint = true;
        }
        true
    }

    /// Starts dragging group `group` by its header.
    pub(super) fn start_header_drag(
        &mut self,
        group: String,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) {
        let source = GroupDragSource::Header(group);
        let target = self.group_drop_target_at(&source, point);
        self.chrome_drag = Some(ClientChromeDrag::SpaceGroups(GroupDrag { source, target }));
        outcome.repaint = true;
    }

    /// Follows the pointer of a group drag.
    pub(in crate::client::shell) fn drag_in_groups(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) {
        let Some(drag) = group_drag(self.chrome_drag.as_ref()) else {
            return;
        };
        let target = self.group_drop_target_at(&drag.source, point);
        if let Some(ClientChromeDrag::SpaceGroups(drag)) = self.chrome_drag.as_mut() {
            drag.target = target;
        }
        outcome.repaint = true;
    }

    /// Ends a group drag: sends the renames and the move its drop makes, rename first.
    pub(in crate::client::shell) fn drop_group_drag(
        &mut self,
        drag: GroupDrag,
        outcome: &mut ClientShellInput,
    ) {
        // A header drag's press ends here, so its release doesn't fold the group.
        self.group_header_press = None;
        outcome.repaint = true;
        let (Some(snapshot), Some(target)) = (self.snapshot.as_deref(), drag.target) else {
            return;
        };
        match drag.source {
            GroupDragSource::Space(workspace_id) => {
                let (methods, ask) = space_drop(snapshot, &workspace_id, &target);
                if self.push_in_order(methods, outcome) {
                    if let Some(name) = ask {
                        self.ask_space_name(workspace_id, &name);
                    }
                }
            }
            GroupDragSource::Header(group) => {
                let methods = header_drop(snapshot, &group, &target);
                self.push_in_order(methods, outcome);
            }
        }
    }

    /// The drop target under `point` for a drag of `source`, picked as upstream picks: the gap
    /// nearest the pointer. A gap starts before each header and each block outside groups (top
    /// level), and before each block of an unfolded group (inside it); never inside a worktree
    /// block. Its line is on the line above its row (on a header's line for a group's first gap).
    /// On a header's line:
    /// - a space joins that group; on its own group's header it goes to the group's first place;
    /// - a header drops in the top-level gap on that line (after a folded group with nothing
    ///   shown), and nowhere on its own header or a line inside a group.
    ///
    /// A header drops only in top-level gaps. None outside the space list.
    fn group_drop_target_at(
        &self,
        source: &GroupDragSource,
        point: (u16, u16),
    ) -> Option<GroupDropTarget> {
        let body = self.hits.workspace_body;
        if body.height == 0
            || point.1 < body.y.saturating_sub(1)
            || point.1 >= self.hits.new_workspace.y
        {
            return None;
        }
        let (snapshot, entries) = self.grouped_sidebar()?;
        let blocks = blocks(snapshot, &entries);
        // The spaces dragged, and the group they are drawn in (None for a header: top level).
        let (dragged, own_group) = match source {
            GroupDragSource::Space(workspace_id) => (
                moving_block(snapshot, &entries, workspace_id)?,
                blocks.heads.get(workspace_id.as_str())?.0,
            ),
            GroupDragSource::Header(group) => (group_spaces(snapshot, &entries, group), None),
        };
        let mut rows = self
            .hits
            .group_headers
            .iter()
            .map(|hit| (hit.rect, DrawnRow::Header(hit.group.as_str())))
            .chain(
                self.hits
                    .workspaces
                    .iter()
                    .filter(|hit| hit.endpoint_id == self.active_endpoint_id)
                    .map(|hit| (hit.rect, DrawnRow::Space(hit.workspace_id.as_str()))),
            )
            .collect::<Vec<_>>();
        rows.sort_by_key(|(rect, _)| rect.y);

        // The gap before a row (its group and `before`; None for no gap), whether the row is
        // dragged, and whether it is where the dragged rows start.
        let row_facts = |row: &DrawnRow<'_>| match *row {
            DrawnRow::Header(group) => {
                let dragged_header =
                    matches!(source, GroupDragSource::Header(name) if name == group);
                (
                    Some((None, blocks.groups.get(group).copied())),
                    dragged_header,
                    dragged_header,
                )
            }
            DrawnRow::Space(workspace_id) => {
                let gap = blocks.heads.get(workspace_id).and_then(|&(group, first)| {
                    let folded =
                        group.is_some_and(|group| self.collapsed_groups.contains(&fold_key(group)));
                    (!folded).then_some((group, Some(first)))
                });
                (
                    gap,
                    dragged.contains(&workspace_id),
                    matches!(source, GroupDragSource::Space(id) if id == workspace_id),
                )
            }
        };
        let mut gaps = Vec::new();
        let mut on_header = None;
        let mut above_dragged = false;
        let mut push_gap = |row: u16, next: &DrawnRow<'_>, above_dragged: bool| {
            let (gap, dragged_row, dragged_start) = row_facts(next);
            if let Some((group, before)) = gap {
                gaps.push(Gap {
                    group,
                    before,
                    row,
                    beside: group == own_group && (above_dragged || dragged_start),
                });
            }
            dragged_row
        };
        for (rect, row) in &rows {
            if let DrawnRow::Header(group) = *row {
                if (rect.y..rect.bottom()).contains(&point.1) {
                    on_header = Some((group, rect.y));
                }
            }
            above_dragged = push_gap(rect.y.saturating_sub(1), row, above_dragged);
        }
        // Below the last row shown, when a line is left above the footer: the gap before the next
        // row (scrolled out or too tall to fit), or the end.
        if let Some((rect, last)) = rows
            .last()
            .filter(|(rect, _)| rect.bottom() < self.hits.new_workspace.y)
        {
            let drawn = self.drawn_rows(snapshot);
            let is_last = |row: &SidebarRow<'_>| match (row, last) {
                (SidebarRow::Header { name, .. }, DrawnRow::Header(group)) => name == group,
                (SidebarRow::Space { entry, .. }, DrawnRow::Space(workspace_id)) => snapshot
                    .workspaces
                    .get(entry.index)
                    .is_some_and(|workspace| workspace.workspace_id == *workspace_id),
                _ => false,
            };
            if let Some(position) = drawn.iter().position(is_last) {
                match drawn.get(position + 1) {
                    Some(SidebarRow::Header { name, .. }) => {
                        push_gap(rect.bottom(), &DrawnRow::Header(name), above_dragged);
                    }
                    Some(SidebarRow::Space { entry, .. }) => {
                        if let Some(workspace) = snapshot.workspaces.get(entry.index) {
                            let next = DrawnRow::Space(workspace.workspace_id.as_str());
                            push_gap(rect.bottom(), &next, above_dragged);
                        }
                    }
                    None => gaps.push(Gap {
                        group: None,
                        before: None,
                        row: rect.bottom(),
                        beside: own_group.is_none() && above_dragged,
                    }),
                }
            }
        }

        let header_drag = matches!(source, GroupDragSource::Header(_));
        if let Some((group, row)) = on_header {
            if header_drag {
                if matches!(source, GroupDragSource::Header(name) if name == group) {
                    return None;
                }
                return gaps
                    .into_iter()
                    .find(|gap| gap.row == row && gap.group.is_none())
                    .map(Gap::target);
            }
            if own_group != Some(group) {
                return Some(GroupDropTarget::Header(group.to_owned()));
            }
            // Back on its own group's header: the group's first place.
            let first = blocks.groups.get(group).copied();
            return Some(GroupDropTarget::Gap {
                group: Some(group.to_owned()),
                before: first.map(str::to_owned),
                row,
                beside: first.is_some_and(|first| dragged.contains(&first)),
            });
        }
        let (_, gap) = gaps
            .into_iter()
            .enumerate()
            .min_by_key(|(index, gap)| (point.1.abs_diff(gap.row), *index))?;
        if header_drag && gap.group.is_some() {
            return None;
        }
        Some(gap.target())
    }
}
