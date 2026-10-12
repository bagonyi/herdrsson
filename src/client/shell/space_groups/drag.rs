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
    /// A space, with the rest of its worktree block when it heads one (`Blocks::block`).
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
    Gap {
        group: Option<String>,
        before: Option<String>,
        row: u16,
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

/// The group drag among the window's drags.
fn group_drag(drag: Option<&ClientChromeDrag>) -> Option<&GroupDrag> {
    match drag {
        Some(ClientChromeDrag::SpaceGroups(drag)) => Some(drag),
        _ => None,
    }
}

/// Draws what upstream's sidebar doesn't of a group drag, over the space list `body`: the dragged
/// header's background, the header a space would join, and a gap's line between a group's members
/// (from the member indent, so it reads "inside").
fn render_group_drag(
    buffer: &mut Buffer,
    body: Rect,
    headers: &[GroupHeaderHit],
    drag: &GroupDrag,
    palette: &Palette,
) {
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

/// What dropping space `workspace_id` on `target` sends. Changing group renames the space:
/// `<group>/<short name>` into a group, its short name at the top level. Then come the moves that
/// put its block before the row below the gap (`group_moves`). Nothing at all when that row's
/// space has gone since. `Err` with the short name to ask the user to finish, and nothing sent
/// yet, when it leaves its group with a `/` in that name (`app/x` would land in group `app`).
fn space_drop(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    target: &GroupDropTarget,
) -> Result<Vec<Method>, String> {
    let Some(workspace) = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == workspace_id)
    else {
        return Ok(Vec::new());
    };
    let own = Blocks::new(snapshot).group(workspace_id);
    let (group, before) = match target {
        GroupDropTarget::Header(group) if own != Some(group.as_str()) => {
            return Ok(join_group_methods(snapshot, workspace_id, group));
        }
        GroupDropTarget::Gap { group, before, .. } => (group.as_deref(), before.as_deref()),
        _ => return Ok(Vec::new()),
    };
    let Some(moves) = group_moves(snapshot, workspace_id, group, Place::Before(before)) else {
        return Ok(Vec::new());
    };
    let mut methods = Vec::new();
    if group != own {
        match group {
            Some(group) => methods.push(join_rename(workspace, group)),
            None => match leave_rename(workspace) {
                Some(Ok(rename)) => methods.push(rename),
                Some(Err(short)) => return Err(short.to_owned()),
                None => {}
            },
        }
    }
    methods.extend(moves);
    Ok(methods)
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
        ..
    } = target
    else {
        return Vec::new();
    };
    let spaces = Blocks::new(snapshot).group_spaces(group);
    let mut plan = OrderPlan::new(snapshot);
    if !spaces.is_empty() {
        plan.move_block(&spaces, before.as_deref());
    }
    plan.methods
}

/// A drawn row of the space list, from its hits.
enum DrawnRow<'a> {
    Header(&'a str),
    Space(&'a str),
}

impl ClientShellState {
    /// Draws the group drag (`render_group_drag`) over the frame just rendered when that frame's
    /// sidebar is the one-machine expanded one, as `render_shell` picks it: a drag left from
    /// before a machine came online draws nothing over the multi-machine sidebar.
    pub(in crate::client::shell) fn draw_group_drag(&self, buffer: &mut Buffer) {
        if self.endpoints.len() > 1 || self.sidebar_collapsed {
            return;
        }
        if let Some(drag) = group_drag(self.chrome_drag.as_ref()) {
            let hits = &self.hits;
            render_group_drag(
                buffer,
                hits.workspace_body,
                &hits.group_headers,
                drag,
                &self.config.palette,
            );
        }
    }

    /// The snapshot's blocks when the sidebar draws a group header: one machine, expanded
    /// (`Blocks::drawn`).
    fn grouped_sidebar(&self) -> Option<(&ClientShellSnapshot, Blocks<'_>)> {
        if self.endpoints.len() != 1 || self.sidebar_collapsed || self.mobile_layout_active() {
            return None;
        }
        let snapshot = self.snapshot.as_deref()?;
        Some((snapshot, Blocks::drawn(snapshot)?))
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
        let Some((_, blocks)) = self.grouped_sidebar() else {
            return false;
        };
        if blocks.block(workspace_id).is_none() {
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

    /// Ends a group drag: sends the renames and the move its drop makes, rename first, or asks
    /// for the dropped space's name first (`space_drop`).
    pub(in crate::client::shell) fn drop_group_drag(
        &mut self,
        drag: GroupDrag,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        let (Some(snapshot), Some(target)) = (self.snapshot.as_deref(), drag.target) else {
            return;
        };
        match drag.source {
            GroupDragSource::Space(workspace_id) => {
                match space_drop(snapshot, &workspace_id, &target) {
                    Ok(methods) => {
                        self.push_in_order(methods, outcome);
                    }
                    // Saved, the name takes it to the drop's place; Esc sends nothing.
                    Err(name) => {
                        let GroupDropTarget::Gap { before, .. } = target else {
                            return;
                        };
                        let target = SpaceName::Dropped {
                            workspace_id,
                            before,
                        };
                        self.ask_space_name(ClientRenameTarget::SpaceName(target), &name);
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
        let (snapshot, blocks) = self.grouped_sidebar()?;
        // The group dragged spaces are drawn in (None for a header: top level).
        let own_group = match source {
            GroupDragSource::Space(workspace_id) => {
                blocks.block(workspace_id)?;
                blocks.group(workspace_id)
            }
            GroupDragSource::Header(_) => None,
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

        // The gap before a row, its line on `row`: none inside a block or a folded group.
        let mut gaps = Vec::new();
        let mut on_header = None;
        let mut push_gap = |row: u16, next: &DrawnRow<'_>| {
            let (group, before) = match *next {
                DrawnRow::Header(group) => (None, blocks.group_spaces(group).first().copied()),
                DrawnRow::Space(workspace_id) => {
                    let Some(first) = blocks.block_first(workspace_id) else {
                        return;
                    };
                    let group = blocks.group(workspace_id);
                    if group.is_some_and(|group| self.collapsed_groups.contains(&fold_key(group))) {
                        return;
                    }
                    (group, Some(first))
                }
            };
            gaps.push(GroupDropTarget::Gap {
                group: group.map(str::to_owned),
                before: before.map(str::to_owned),
                row,
            });
        };
        for (rect, row) in &rows {
            if let DrawnRow::Header(group) = *row {
                if (rect.y..rect.bottom()).contains(&point.1) {
                    on_header = Some((group, rect.y));
                }
            }
            push_gap(rect.y.saturating_sub(1), row);
        }
        // Below the last row shown, when a line is left above the footer: the gap before the next
        // row (scrolled out or too tall to fit), or the end.
        if let Some((rect, last)) = rows
            .last()
            .filter(|(rect, _)| rect.bottom() < self.hits.new_workspace.y)
        {
            let drawn = self.drawn_rows(snapshot);
            let is_last = |row: &SidebarRow<'_>| match last {
                DrawnRow::Header(group) => {
                    matches!(row, SidebarRow::Header { name, .. } if name == group)
                }
                DrawnRow::Space(workspace_id) => row.is_space(snapshot, workspace_id),
            };
            if let Some(position) = drawn.iter().position(is_last) {
                match drawn.get(position + 1) {
                    Some(SidebarRow::Header { name, .. }) => {
                        push_gap(rect.bottom(), &DrawnRow::Header(name));
                    }
                    Some(SidebarRow::Space { entry, .. }) => {
                        if let Some(workspace) = snapshot.workspaces.get(entry.index) {
                            let next = DrawnRow::Space(workspace.workspace_id.as_str());
                            push_gap(rect.bottom(), &next);
                        }
                    }
                    None => gaps.push(GroupDropTarget::Gap {
                        group: None,
                        before: None,
                        row: rect.bottom(),
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
                return gaps.into_iter().find(|gap| {
                    matches!(gap, GroupDropTarget::Gap { group: None, row: at, .. } if *at == row)
                });
            }
            if own_group != Some(group) {
                return Some(GroupDropTarget::Header(group.to_owned()));
            }
            // Back on its own group's header: the group's first place.
            return Some(GroupDropTarget::Gap {
                group: Some(group.to_owned()),
                before: blocks
                    .group_spaces(group)
                    .first()
                    .map(|first| (*first).to_owned()),
                row,
            });
        }
        let (_, gap) = gaps.into_iter().enumerate().min_by_key(|(index, gap)| {
            let GroupDropTarget::Gap { row, .. } = gap else {
                return (u16::MAX, *index);
            };
            (point.1.abs_diff(*row), *index)
        })?;
        if header_drag && matches!(gap, GroupDropTarget::Gap { group: Some(_), .. }) {
            return None;
        }
        Some(gap)
    }
}
