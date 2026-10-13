//! Fork: the red ` N ` on a space row, like an app badge on macOS: how many of the space's agents
//! wait for the user, having finished or asked a question since their pane was last on screen. A
//! folded worktree parent counts its linked worktrees too, and a folded space group's header its
//! members (`space_groups::unseen_by_group`).

use super::render::{display_width, put_text};
use super::*;

/// One machine's waiting agents per space, counted once a frame so that rows only look them up.
#[derive(Default)]
pub(super) struct UnseenCounts<'a> {
    spaces: HashMap<&'a str, usize>,
}

impl<'a> UnseenCounts<'a> {
    pub(super) fn of(endpoint: &'a ClientShellEndpoint) -> Self {
        let mut spaces = HashMap::<&str, usize>::new();
        for agent in endpoint.unseen_agents() {
            *spaces.entry(agent.workspace_id.as_str()).or_default() += 1;
        }
        Self { spaces }
    }

    /// The counts of machine `endpoint_id`; none when it isn't listed.
    pub(super) fn of_endpoint(
        endpoints: &'a [ClientShellEndpoint],
        endpoint_id: &ClientEndpointId,
    ) -> Self {
        endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .map(Self::of)
            .unwrap_or_default()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.spaces.is_empty()
    }

    /// Every waiting agent on the machine.
    pub(super) fn total(&self) -> usize {
        self.spaces.values().sum()
    }

    /// Space `workspace_id`'s own waiting agents.
    pub(super) fn own(&self, workspace_id: &str) -> usize {
        self.spaces.get(workspace_id).copied().unwrap_or(0)
    }

    /// What `workspace`'s row shows: its own count and, while it is a folded worktree parent, its
    /// linked worktrees' too (the spaces `displayed_workspace_status` rolls up).
    pub(super) fn space(
        &self,
        snapshot: &ClientShellSnapshot,
        workspace: &ClientShellWorkspace,
        collapsed_groups: &HashSet<String>,
    ) -> usize {
        let own = self.own(&workspace.workspace_id);
        let Some(worktree) = workspace.worktree.as_ref().filter(|worktree| {
            !self.is_empty()
                && !worktree.is_linked_worktree
                && collapsed_groups.contains(&worktree.key)
        }) else {
            return own;
        };
        let linked = snapshot
            .workspaces
            .iter()
            .filter(|candidate| {
                candidate
                    .worktree
                    .as_ref()
                    .is_some_and(|member| member.is_linked_worktree && member.key == worktree.key)
            })
            .map(|candidate| self.own(&candidate.workspace_id))
            .sum::<usize>();
        own + linked
    }

    /// A collapsed rail's space number: `style`, or the count's red while agents wait there.
    pub(super) fn number_style(
        &self,
        workspace_id: &str,
        style: Style,
        palette: &Palette,
    ) -> Style {
        if self.own(workspace_id) > 0 {
            badge_style(palette)
        } else {
            style
        }
    }
}

/// Bold, on the theme's red.
pub(super) fn badge_style(palette: &Palette) -> Style {
    Style::default()
        .fg(super::panel_contrast_fg(palette))
        .bg(palette.red)
        .add_modifier(Modifier::BOLD)
}

/// The columns ` N ` takes; none for no waiting agent.
pub(super) fn badge_width(count: usize) -> u16 {
    if count == 0 {
        0
    } else {
        display_width(&format!(" {count} "))
    }
}

/// Draws ` N ` on `row`'s first line, ending at `right`; nothing when no agent is waiting.
pub(super) fn render_badge(
    buffer: &mut Buffer,
    row: Rect,
    right: u16,
    count: usize,
    palette: &Palette,
) {
    let width = badge_width(count).min(right.saturating_sub(row.x));
    if width > 0 {
        put_text(
            buffer,
            right.saturating_sub(width),
            row.y,
            width,
            &format!(" {count} "),
            badge_style(palette),
        );
    }
}

/// Where a space row's count ends: at its right edge, or left of its worktree ▾/▸.
fn space_badge_right(row: Rect, has_toggle: bool) -> u16 {
    row.right().saturating_sub(if has_toggle { 2 } else { 0 })
}

/// Draws a space row's count at its right edge, left of its worktree ▾/▸ when it has one.
pub(super) fn render_space_badge(
    buffer: &mut Buffer,
    row: Rect,
    has_toggle: bool,
    count: usize,
    palette: &Palette,
) {
    render_badge(
        buffer,
        row,
        space_badge_right(row, has_toggle),
        count,
        palette,
    );
}

/// The column a space row's first line of text ends before: two before the edge, as upstream
/// leaves them, or before its count, so that a cut-off name keeps its `…`. `has_toggle` (whether
/// the row has a worktree ▾/▸) is only asked when there is a count.
pub(super) fn text_end(row: Rect, count: usize, has_toggle: impl FnOnce() -> bool) -> u16 {
    let end = row.right().saturating_sub(2);
    if count == 0 {
        return end;
    }
    space_badge_right(row, has_toggle())
        .saturating_sub(badge_width(count))
        .min(end)
}
