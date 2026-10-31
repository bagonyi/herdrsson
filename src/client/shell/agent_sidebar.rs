use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::*;

pub(super) struct AgentRow {
    pub(super) pane_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
    /// Fork: the agent counts as older (`[ui.sidebar.agents.older]`).
    pub(super) older: bool,
}

pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
) -> Vec<String> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .cloned()
            .collect();
    }
    let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
    if sort == crate::config::AgentPanelSortConfig::Priority {
        agents.sort_by_key(|agent| {
            (
                std::cmp::Reverse(status_priority(agent.agent_status)),
                std::cmp::Reverse(agent.state_change_seq),
            )
        });
    }
    agents
        .into_iter()
        .map(|agent| agent.pane_id.clone())
        .collect()
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    let rows = agent_rows(snapshot, config, None);
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        &rows.iter().map(|row| row.status).collect::<Vec<_>>(),
        config,
        hits,
    ) {
        return;
    }

    render_agent_list(
        buffer,
        area,
        &rows,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |row| row.rows.len(),
        |row| row.older,
        |buffer, rect, row, hits| {
            hits.agents.push((rect, row.pane_id.clone()));
            render_agent_row(buffer, rect, row, config);
        },
    );
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    agent_statuses: &[crate::api::schema::AgentStatus],
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    // Fork: the number of agents in the list follows the title.
    let count_text = format!("· {}", agent_statuses.len());
    put_text(
        buffer,
        area.x + 8,
        area.y + 1,
        area.width.saturating_sub(8),
        &count_text,
        Style::default().fg(config.palette.overlay0),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    render_agent_status_counts(
        buffer,
        area.x + 8 + display_width(&count_text) as u16 + 1,
        area.y + 1,
        sort_rect.x.saturating_sub(1),
        agent_statuses,
        config,
    );
    true
}

// Fork: how many agents are blocked, done, working and idle follows the count,
// `· ● 2  ● 3  ○ 19` in their rows' colours. A status no agent has is left out, and so
// is one that would run into the sort label, idle first.
fn render_agent_status_counts(
    buffer: &mut Buffer,
    mut x: u16,
    y: u16,
    right: u16,
    agent_statuses: &[crate::api::schema::AgentStatus],
    config: &ClientShellConfig,
) {
    use crate::api::schema::AgentStatus;
    let mut separator = Some("· ");
    for status in [
        AgentStatus::Blocked,
        AgentStatus::Done,
        AgentStatus::Working,
        AgentStatus::Idle,
    ] {
        let count = agent_statuses
            .iter()
            .filter(|candidate| **candidate == status)
            .count();
        if count == 0 {
            continue;
        }
        let text = format!("{} {count}", status_icon(status, config.status_indicators));
        let width = display_width(&text) as u16;
        let lead = separator.map_or(0, |separator| display_width(separator) as u16);
        if x.saturating_add(lead + width) > right {
            return;
        }
        if let Some(separator) = separator.take() {
            put_text(
                buffer,
                x,
                y,
                lead,
                separator,
                Style::default().fg(config.palette.overlay0),
            );
            x = x.saturating_add(lead);
        }
        put_text(
            buffer,
            x,
            y,
            width,
            &text,
            Style::default().fg(status_color(status, &config.palette)),
        );
        x = x.saturating_add(width + 2);
    }
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    row_older: impl Fn(&T) -> bool,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let older = rows.iter().map(row_older).collect::<Vec<_>>();
    let (gaps, divider_after) = agent_list_gaps(&older, &config.agents);
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y.saturating_add(height);
        if divider_after == Some(index) {
            // In the middle of the gap, or one line nearer the older agents it heads.
            let divider_y = y.saturating_add(config.agents.row_gap.div_ceil(2));
            if divider_y < body.bottom() {
                render_older_divider(buffer, body.x, divider_y, content_width, config);
            }
        }
        y = y.saturating_add(gaps[index]);
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

/// The lines after each agent: `row_gap` between agents, and one more after the last agent
/// before the older ones for the line above them (`[ui.sidebar.agents.older] divider`). That
/// line shows only where the older agents come last, after at least one other.
pub(super) fn agent_list_gaps(
    older: &[bool],
    agents: &crate::config::AgentsSidebarConfig,
) -> (Vec<u16>, Option<usize>) {
    let divider_after = agents
        .older
        .as_ref()
        .and_then(|config| config.divider.as_ref())
        .and_then(|_| {
            let first = older.iter().position(|older| *older)?;
            (first > 0 && older[first..].iter().all(|older| *older)).then(|| first - 1)
        });
    let gaps = (0..older.len())
        .map(|index| {
            if index + 1 == older.len() {
                0
            } else {
                agents
                    .row_gap
                    .saturating_add(u16::from(divider_after == Some(index)))
            }
        })
        .collect();
    (gaps, divider_after)
}

/// Fork: the line above the first older agent, its text in the middle: `──── earlier ────`.
fn render_older_divider(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    config: &ClientShellConfig,
) {
    let text = config
        .agents
        .older
        .as_ref()
        .and_then(|older| older.divider.as_deref())
        .filter(|text| !text.is_empty())
        .map(|text| format!(" {text} "))
        .unwrap_or_default();
    let text_width = display_width(&text).min(width as usize);
    let left = (width as usize - text_width) / 2;
    let right = width as usize - text_width - left;
    // Not `surface_dim`, the panel's own top line: in dark themes that is the background colour
    // itself, and the line would vanish on a black terminal.
    let rule = Style::default().fg(config.palette.surface1);
    Paragraph::new(Line::from(vec![
        ratatui::text::Span::styled("─".repeat(left), rule),
        ratatui::text::Span::styled(text, Style::default().fg(config.palette.overlay0)),
        ratatui::text::Span::styled("─".repeat(right), rule),
    ]))
    .render(Rect::new(x, y, width, 1), buffer);
}

pub(super) fn agent_rows(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Vec<AgentRow> {
    ordered_agent_pane_ids(snapshot, config.agent_panel_sort)
        .into_iter()
        .filter_map(|pane_id| agent_row(snapshot, &pane_id, config, machine))
        .collect()
}

pub(super) fn agent_row(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Option<AgentRow> {
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = labels
        .get(status_text(agent.agent_status))
        .map(String::as_str)
        .unwrap_or_else(|| sidebar_status_text(agent.agent_status));
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let working = agent.agent_status == crate::api::schema::AgentStatus::Working;
    let older = crate::ui::sidebar_agent_is_older(&config.agents, &tokens, working);
    let mut rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: agent
                .title
                .as_deref()
                .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
            older,
        },
        state_text,
    );
    // Fork: a working agent shows `working` in place of its `ago` times.
    if working {
        crate::ui::sidebar_show_working(&mut rows);
    }
    Some(AgentRow {
        pane_id: agent.pane_id.clone(),
        status: agent.agent_status,
        focused: agent.focused,
        rows,
        older,
    })
}

pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, config.status_indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        let indent = if index == 0 { 1 } else { 3 };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent))];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}
