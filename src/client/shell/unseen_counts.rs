//! Fork: the red ` N ` on a space row, like an app badge on macOS: how many of the space's agents
//! wait for the user, having finished or asked a question since their pane was last on screen. A
//! folded worktree parent counts its linked worktrees too, and a folded space group's header its
//! members (`space_groups::unseen_by_group`). `[ui.sidebar.spaces] unseen_indicator` draws a red
//! `●` instead (`dot`), or nothing (`off`).

use super::render::{display_width, put_text};
use super::*;
use crate::config::UnseenIndicator;

/// What `dot` draws: the dot where a one-digit count's number would be, in the count's columns.
const DOT: &str = " ● ";

/// One machine's waiting agents per space, counted once a frame so that rows only look them up.
#[derive(Default)]
pub(super) struct UnseenCounts<'a> {
    spaces: HashMap<&'a str, usize>,
    indicator: UnseenIndicator,
}

impl<'a> UnseenCounts<'a> {
    /// `endpoint`'s waiting agents, shown as `indicator`; none at all while it is `off`.
    pub(super) fn of(endpoint: &'a ClientShellEndpoint, indicator: UnseenIndicator) -> Self {
        let mut spaces = HashMap::<&str, usize>::new();
        if indicator != UnseenIndicator::Off {
            for agent in endpoint.unseen_agents() {
                *spaces.entry(agent.workspace_id.as_str()).or_default() += 1;
            }
        }
        Self { spaces, indicator }
    }

    /// The counts of machine `endpoint_id`; none when it isn't listed.
    pub(super) fn of_endpoint(
        endpoints: &'a [ClientShellEndpoint],
        endpoint_id: &ClientEndpointId,
        indicator: UnseenIndicator,
    ) -> Self {
        endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .map(|endpoint| Self::of(endpoint, indicator))
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

    /// A collapsed rail's space number: `style`, or on red (`count`) or red (`dot`) while agents
    /// wait there.
    pub(super) fn number_style(
        &self,
        workspace_id: &str,
        style: Style,
        palette: &Palette,
    ) -> Style {
        if self.own(workspace_id) == 0 {
            return style;
        }
        match self.indicator {
            UnseenIndicator::Count => badge_style(palette),
            UnseenIndicator::Dot => style.fg(palette.red).add_modifier(Modifier::BOLD),
            UnseenIndicator::Off => style,
        }
    }
}

/// `count`: bold, on the theme's red.
fn badge_style(palette: &Palette) -> Style {
    Style::default()
        .fg(super::panel_contrast_fg(palette))
        .bg(palette.red)
        .add_modifier(Modifier::BOLD)
}

/// `dot`: the theme's red, on whatever the row has under it.
fn dot_style(palette: &Palette) -> Style {
    Style::default().fg(palette.red)
}

/// What a row shows for `count` waiting agents: ` N `, or `dot`'s ` ● `; nothing for none.
fn badge_text(count: usize, indicator: UnseenIndicator) -> Option<String> {
    match indicator {
        _ if count == 0 => None,
        UnseenIndicator::Count => Some(format!(" {count} ")),
        UnseenIndicator::Dot => Some(DOT.to_owned()),
        UnseenIndicator::Off => None,
    }
}

fn indicator_style(indicator: UnseenIndicator, palette: &Palette) -> Style {
    if indicator == UnseenIndicator::Dot {
        dot_style(palette)
    } else {
        badge_style(palette)
    }
}

/// The narrow multi-machine sidebar's total for a folded machine: `N` (`9+` past nine) on red,
/// or a red `●`; nothing for none.
pub(super) fn compact_badge(
    count: usize,
    indicator: UnseenIndicator,
    palette: &Palette,
) -> Option<(String, Style)> {
    match indicator {
        _ if count == 0 => None,
        UnseenIndicator::Count if count > 9 => Some(("9+".to_owned(), badge_style(palette))),
        UnseenIndicator::Count => Some((count.to_string(), badge_style(palette))),
        UnseenIndicator::Dot => Some((DOT.trim().to_owned(), dot_style(palette))),
        UnseenIndicator::Off => None,
    }
}

/// The columns the badge takes; none for no waiting agent.
pub(super) fn badge_width(count: usize, indicator: UnseenIndicator) -> u16 {
    badge_text(count, indicator).map_or(0, |text| display_width(&text))
}

/// Draws the badge on `row`'s first line, ending at `right`; nothing when no agent is waiting.
pub(super) fn render_badge(
    buffer: &mut Buffer,
    row: Rect,
    right: u16,
    count: usize,
    indicator: UnseenIndicator,
    palette: &Palette,
) {
    let Some(text) = badge_text(count, indicator) else {
        return;
    };
    let width = display_width(&text).min(right.saturating_sub(row.x));
    if width > 0 {
        put_text(
            buffer,
            right.saturating_sub(width),
            row.y,
            width,
            &text,
            indicator_style(indicator, palette),
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
    indicator: UnseenIndicator,
    palette: &Palette,
) {
    render_badge(
        buffer,
        row,
        space_badge_right(row, has_toggle),
        count,
        indicator,
        palette,
    );
}

/// The column a space row's first line of text ends before: two before the edge, as upstream
/// leaves them, or before its count, so that a cut-off name keeps its `…`. `has_toggle` (whether
/// the row has a worktree ▾/▸) is only asked when there is a count.
pub(super) fn text_end(
    row: Rect,
    count: usize,
    indicator: UnseenIndicator,
    has_toggle: impl FnOnce() -> bool,
) -> u16 {
    let end = row.right().saturating_sub(2);
    let width = badge_width(count, indicator);
    if width == 0 {
        return end;
    }
    space_badge_right(row, has_toggle())
        .saturating_sub(width)
        .min(end)
}
