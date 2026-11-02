//! Fork: text with SGR sequences (`ESC [ … m`), as the server keeps them in the output of
//! `[ui.sidebar.agents] title_command`, split into styled parts for the agents panel's title.

use ratatui::style::{Color, Modifier, Style};

/// The parts of `text` with their styles, the sequences applied on top of `base` (`ESC[0m` goes
/// back to it). Empty parts are left out, and so is anything after a sequence with no end.
pub(super) fn styled_parts(text: &str, base: Style) -> Vec<(String, Style)> {
    let mut parts = Vec::new();
    let mut style = base;
    let mut rest = text;
    loop {
        let (plain, sequence) = match rest.find("\x1b[") {
            Some(start) => (&rest[..start], Some(&rest[start + 2..])),
            None => (rest, None),
        };
        let plain: String = plain
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        if !plain.is_empty() {
            parts.push((plain, style));
        }
        let Some(sequence) = sequence else {
            return parts;
        };
        let Some(end) = sequence.find('m') else {
            return parts;
        };
        style = apply_sgr(style, base, &sequence[..end]);
        rest = &sequence[end + 1..];
    }
}

fn apply_sgr(mut style: Style, base: Style, parameters: &str) -> Style {
    // An empty parameter is 0, as in a terminal; one too large for any code is no code at all
    // rather than a reset.
    let mut codes = parameters.split(';').map(|code| {
        if code.is_empty() {
            0
        } else {
            code.parse::<u64>()
                .ok()
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(u16::MAX)
        }
    });
    while let Some(code) = codes.next() {
        match code {
            0 => style = base,
            1 => style = style.add_modifier(Modifier::BOLD),
            2 => style = style.add_modifier(Modifier::DIM),
            3 => style = style.add_modifier(Modifier::ITALIC),
            4 => style = style.add_modifier(Modifier::UNDERLINED),
            7 => style = style.add_modifier(Modifier::REVERSED),
            22 => style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style = style.remove_modifier(Modifier::ITALIC),
            24 => style = style.remove_modifier(Modifier::UNDERLINED),
            27 => style = style.remove_modifier(Modifier::REVERSED),
            30..=37 => style.fg = Some(Color::Indexed((code - 30) as u8)),
            90..=97 => style.fg = Some(Color::Indexed((code - 90 + 8) as u8)),
            39 => style.fg = base.fg,
            40..=47 => style.bg = Some(Color::Indexed((code - 40) as u8)),
            100..=107 => style.bg = Some(Color::Indexed((code - 100 + 8) as u8)),
            49 => style.bg = base.bg,
            38 => {
                if let Some(color) = extended_color(&mut codes) {
                    style.fg = Some(color);
                }
            }
            48 => {
                if let Some(color) = extended_color(&mut codes) {
                    style.bg = Some(color);
                }
            }
            // The underline colour isn't shown, but its numbers mustn't count as codes.
            58 => {
                extended_color(&mut codes);
            }
            _ => {}
        }
    }
    style
}

/// `5;N` (one of 256 colours) or `2;R;G;B`, after a `38`, `48` or `58`. Takes all of its
/// numbers even when one is out of range, so none of them is read as a code of its own.
fn extended_color(codes: &mut impl Iterator<Item = u16>) -> Option<Color> {
    let component = |code: Option<u16>| code.and_then(|code| u8::try_from(code).ok());
    match codes.next()? {
        5 => component(codes.next()).map(Color::Indexed),
        2 => {
            let (red, green, blue) = (codes.next(), codes.next(), codes.next());
            Some(Color::Rgb(
                component(red)?,
                component(green)?,
                component(blue)?,
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_style_the_text_after_them_on_top_of_the_base() {
        let base = Style::default().fg(Color::Rgb(1, 1, 1));
        let chip = Style::default()
            .fg(Color::Rgb(169, 177, 214))
            .bg(Color::Rgb(36, 40, 59));
        assert_eq!(
            styled_parts(
                "\x1b[48;2;36;40;59m\x1b[38;2;169;177;214m 7d 6% \x1b[38;5;60m· 6d \x1b[0m!",
                base,
            ),
            vec![
                (" 7d 6% ".to_string(), chip),
                ("· 6d ".to_string(), chip.fg(Color::Indexed(60))),
                ("!".to_string(), base),
            ]
        );
        assert_eq!(
            styled_parts("\x1b[1;31mred\x1b[22;39m plain\x1b[m", base),
            vec![
                (
                    "red".to_string(),
                    base.fg(Color::Indexed(1)).add_modifier(Modifier::BOLD)
                ),
                (
                    " plain".to_string(),
                    base.remove_modifier(Modifier::BOLD | Modifier::DIM)
                ),
            ]
        );
    }

    #[test]
    fn plain_text_keeps_the_base_and_broken_sequences_show_nothing() {
        let base = Style::default().fg(Color::Rgb(1, 1, 1));
        assert_eq!(
            styled_parts("7d 6%", base),
            vec![("7d 6%".to_string(), base)]
        );
        assert_eq!(
            styled_parts("ok\x1b[38;2;999;0;0mstill\x1b[31", base),
            vec![("ok".to_string(), base), ("still".to_string(), base)],
            "an out-of-range colour is ignored, an unfinished sequence ends the text"
        );
        let bold = base.add_modifier(Modifier::BOLD);
        assert_eq!(
            styled_parts(
                "\x1b[1mA\x1b[38;2;999;0;0mB\x1b[58;2;1;3;4mC\x1b[65536mD\x1b[38;5;99999mE",
                base,
            ),
            vec![
                ("A".to_string(), bold),
                ("B".to_string(), bold),
                ("C".to_string(), bold),
                ("D".to_string(), bold),
                ("E".to_string(), bold),
            ],
            "out-of-range numbers and underline colours change nothing, nor do the numbers after them"
        );
        assert!(styled_parts("\x1b[0m", base).is_empty());
    }
}
