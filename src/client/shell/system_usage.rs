//! Fork: the machine's CPU and memory use at the right of the spaces header, from the snapshot's
//! `system_usage` (`[ui.sidebar.spaces] system_usage`): ` 78%  󰍛 13.3G` in a rounded pill, as
//! the agents panel's Claude usage pill is drawn. CPU goes green, yellow, orange and red as it
//! gets busier; memory takes the colour of the system's memory pressure, as a Mac fills its memory
//! by design, so how full it is says little alone.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::app::state::Palette;
use crate::protocol::{ClientShellMemoryPressure, ClientShellSystemUsage};

/// Nerd Font icons, as the agent rows' `$subagents` and `$shells` prefixes are: a chip for the
/// CPU (nf-oct-cpu), a memory module for memory (nf-md-memory).
const CPU_ICON: &str = "\u{f4bc}";
const MEMORY_ICON: &str = "\u{f035b}";
/// The pill's rounded ends, Powerline characters that Ghostty draws itself.
const LEFT_END: &str = "\u{e0b6}";
const RIGHT_END: &str = "\u{e0b4}";

/// Draws the pill on row `y`, ending at `right` (exclusive) and starting no further left than
/// `left`; without its icons where it doesn't fit with them (`78%  13.3G`). Returns where it's
/// drawn, none when it doesn't fit even so.
pub(super) fn render_system_usage(
    buffer: &mut Buffer,
    left: u16,
    right: u16,
    y: u16,
    usage: &ClientShellSystemUsage,
    palette: &Palette,
) -> Option<Rect> {
    let (parts, start, width) = [true, false].into_iter().find_map(|icons| {
        let parts = pill_parts(usage, palette, icons);
        let width = parts
            .iter()
            .map(|(text, _)| unicode_width::UnicodeWidthStr::width(text.as_str()))
            .sum::<usize>();
        let width = u16::try_from(width).ok()?;
        let start = right.checked_sub(width).filter(|start| *start >= left)?;
        Some((parts, start, width))
    })?;
    let mut x = start;
    for (text, style) in &parts {
        (x, _) = buffer.set_stringn(x, y, text, usize::from(right.saturating_sub(x)), *style);
    }
    Some(Rect::new(start, y, width, 1))
}

/// The pill's texts in their styles, left to right, with or without the icons.
fn pill_parts(
    usage: &ClientShellSystemUsage,
    palette: &Palette,
    icons: bool,
) -> Vec<(String, Style)> {
    let ends = Style::default().fg(palette.surface0);
    let inside = Style::default().bg(palette.surface0);
    let icon = |glyph: &str| (format!("{glyph} "), inside.fg(palette.overlay1));
    let mut parts = vec![(LEFT_END.to_owned(), ends), (" ".to_owned(), inside)];
    if let Some(cpu) = usage.cpu_percent {
        parts.extend(icons.then(|| icon(CPU_ICON)));
        parts.push((format!("{cpu}%  "), inside.fg(cpu_colour(cpu, palette))));
    }
    parts.extend(icons.then(|| icon(MEMORY_ICON)));
    let memory = memory_colour(usage.memory_pressure, palette);
    parts.push((
        format!("{} ", gigabytes(usage.memory_used_bytes)),
        inside.fg(memory),
    ));
    parts.push((RIGHT_END.to_owned(), ends));
    parts
}

/// Green below 40%, yellow below 70%, orange below 90%, then red.
fn cpu_colour(percent: u8, palette: &Palette) -> Color {
    match percent {
        0..40 => palette.green,
        40..70 => palette.yellow,
        70..90 => palette.peach,
        _ => palette.red,
    }
}

fn memory_colour(pressure: ClientShellMemoryPressure, palette: &Palette) -> Color {
    match pressure {
        ClientShellMemoryPressure::Normal => palette.green,
        ClientShellMemoryPressure::Warning => palette.peach,
        ClientShellMemoryPressure::Critical => palette.red,
        ClientShellMemoryPressure::Unknown => palette.subtext0,
    }
}

/// `13.3G`, in GiB as Activity Monitor counts; `128G` from 100 on.
fn gigabytes(bytes: u64) -> String {
    let gib = bytes as f64 / f64::from(1_u32 << 30);
    if gib >= 99.95 {
        format!("{gib:.0}G")
    } else {
        format!("{gib:.1}G")
    }
}
