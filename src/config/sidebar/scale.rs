//! Fork: `fg_scale`, a token's colour blended between stops by the number it shows, e.g. green
//! for a small context turning yellow, orange and then red as it grows:
//!
//! ```toml
//! { token = "$cache_tokens", fg_scale = [
//!   { at = 75_000, fg = "#9ece6a" },
//!   { at = 225_000, fg = "#e0af68" },
//! ] }
//! ```
//!
//! Below the first stop the token takes the first stop's colour and above the last the last
//! one's. The number may end in `k` or `M`, as token counts do: `70.0k`, `1.2M`.

use serde::{Deserialize, Serialize};

use super::SidebarTokenColor;

pub(super) const MAX_STOPS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawStop", into = "RawStop")]
pub struct SidebarColorStop {
    at: f64,
    fg: SidebarTokenColor,
}

// Deserialization rejects non-finite stops, so equality is reflexive.
impl Eq for SidebarColorStop {}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStop {
    at: f64,
    fg: SidebarTokenColor,
}

impl TryFrom<RawStop> for SidebarColorStop {
    type Error = String;

    fn try_from(raw: RawStop) -> Result<Self, Self::Error> {
        if !raw.at.is_finite() {
            return Err("sidebar fg_scale stops must be at finite numbers".into());
        }
        Ok(Self {
            at: raw.at,
            fg: raw.fg,
        })
    }
}

impl From<SidebarColorStop> for RawStop {
    fn from(stop: SidebarColorStop) -> Self {
        Self {
            at: stop.at,
            fg: stop.fg,
        }
    }
}

pub(super) fn validate(stops: &[SidebarColorStop]) -> Result<(), String> {
    if stops.len() > MAX_STOPS {
        return Err(format!(
            "sidebar fg_scale may contain at most {MAX_STOPS} stops"
        ));
    }
    if stops.windows(2).any(|pair| pair[1].at < pair[0].at) {
        return Err("sidebar fg_scale stops must be in ascending order".into());
    }
    Ok(())
}

/// The colour `value` takes on the scale, if it is a number and there are stops.
pub(super) fn color_at(stops: &[SidebarColorStop], value: &str) -> Option<SidebarTokenColor> {
    let number = parse_number(value)?;
    let (first, last) = (stops.first()?, stops.last()?);
    if number <= first.at {
        return Some(first.fg);
    }
    if number >= last.at {
        return Some(last.fg);
    }
    let pair = stops
        .windows(2)
        .find(|pair| pair[0].at <= number && number < pair[1].at)?;
    let share = (number - pair[0].at) / (pair[1].at - pair[0].at);
    let blend = |from: u8, to: u8| {
        (f64::from(from) + (f64::from(to) - f64::from(from)) * share).round() as u8
    };
    let (from, to) = (pair[0].fg, pair[1].fg);
    Some(SidebarTokenColor {
        r: blend(from.r, to.r),
        g: blend(from.g, to.g),
        b: blend(from.b, to.b),
    })
}

/// A whole finite number, optionally ending in `k` (thousands) or `M` (millions).
fn parse_number(value: &str) -> Option<f64> {
    let (digits, factor) = if let Some(digits) = value.strip_suffix(['k', 'K']) {
        (digits, 1e3)
    } else if let Some(digits) = value.strip_suffix('M') {
        (digits, 1e6)
    } else {
        (value, 1.0)
    };
    digits
        .parse::<f64>()
        .ok()
        .map(|number| number * factor)
        .filter(|number| number.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stops(toml: &str) -> Result<Vec<SidebarColorStop>, String> {
        #[derive(Deserialize)]
        struct Holder {
            fg_scale: Vec<SidebarColorStop>,
        }
        let holder: Holder = toml::from_str(toml).map_err(|error| error.to_string())?;
        validate(&holder.fg_scale)?;
        Ok(holder.fg_scale)
    }

    fn rgb(color: Option<SidebarTokenColor>) -> Option<(u8, u8, u8)> {
        color.map(|color| (color.r, color.g, color.b))
    }

    #[test]
    fn colours_blend_between_stops_and_hold_beyond_the_ends() {
        let scale = stops(
            r##"fg_scale = [{ at = 100, fg = "#000000" }, { at = 200, fg = "#ff8000" }, { at = 300, fg = "#00ff00" }]"##,
        )
        .unwrap();
        for (value, color) in [
            ("0", (0, 0, 0)),
            ("100", (0, 0, 0)),
            ("150", (128, 64, 0)),
            ("200", (255, 128, 0)),
            ("275", (64, 223, 0)),
            ("300", (0, 255, 0)),
            ("1e9", (0, 255, 0)),
        ] {
            assert_eq!(rgb(color_at(&scale, value)), Some(color), "{value}");
        }
    }

    #[test]
    fn numbers_may_end_in_k_or_m() {
        let scale = stops(
            r##"fg_scale = [{ at = 0, fg = "#000000" }, { at = 2_000_000, fg = "#c8c8c8" }]"##,
        )
        .unwrap();
        for (value, grey) in [
            ("812", 0),
            ("70.0k", 7),
            ("500K", 50),
            ("1.2M", 120),
            ("2M", 200),
        ] {
            assert_eq!(
                rgb(color_at(&scale, value)),
                Some((grey, grey, grey)),
                "{value}"
            );
        }
        for value in [
            "", "k", "70.0 k", " 70k", "70km", "5m", "NaN", "infk", "1e999",
        ] {
            assert_eq!(color_at(&scale, value), None, "{value}");
        }
    }

    #[test]
    fn equal_stops_switch_at_once_and_a_single_stop_is_one_colour() {
        let scale = stops(
            r##"fg_scale = [{ at = 0, fg = "#000" }, { at = 10, fg = "#000" }, { at = 10, fg = "#fff" }, { at = 20, fg = "#fff" }]"##,
        )
        .unwrap();
        assert_eq!(rgb(color_at(&scale, "9.9")), Some((0, 0, 0)));
        assert_eq!(rgb(color_at(&scale, "10")), Some((255, 255, 255)));
        let single = stops(r##"fg_scale = [{ at = 5, fg = "#f00" }]"##).unwrap();
        for value in ["1", "5", "9"] {
            assert_eq!(rgb(color_at(&single, value)), Some((255, 0, 0)));
        }
        assert_eq!(color_at(&[], "5"), None);
    }

    #[test]
    fn stops_must_be_ascending_finite_and_few() {
        for (toml, error) in [
            (
                r##"fg_scale = [{ at = 2, fg = "#000" }, { at = 1, fg = "#fff" }]"##,
                "ascending order",
            ),
            (r##"fg_scale = [{ at = inf, fg = "#000" }]"##, "finite"),
            (r##"fg_scale = [{ at = nan, fg = "#000" }]"##, "finite"),
            (r##"fg_scale = [{ at = 1 }]"##, "missing field"),
            (
                r##"fg_scale = [{ at = 1, fg = "#000", bold = true }]"##,
                "unknown field",
            ),
        ] {
            let message = stops(toml).unwrap_err();
            assert!(message.contains(error), "{toml}: {message}");
        }
        let many = (0..=MAX_STOPS)
            .map(|at| format!(r##"{{ at = {at}, fg = "#000" }}"##))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(stops(&format!("fg_scale = [{many}]"))
            .unwrap_err()
            .contains("at most 16 stops"));
    }
}
