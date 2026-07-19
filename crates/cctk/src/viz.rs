//! Braille-dot visualization primitives.
//!
//! Everything here is a pure `f64`/`&[f64]` → `String` mapping so it is unit-
//! testable without a terminal. Two shapes:
//! - [`dot_bar`] — a horizontal magnitude bar (0..1 ratio) at 2-dot-column
//!   resolution per cell.
//! - [`sparkline`] — a compact time-series graph packing two bottom-anchored
//!   samples (4 vertical levels each) per cell.
//!
//! Braille cell layout (Unicode U+2800 + bitmask):
//! ```text
//!   dot1 (0x01)  dot4 (0x08)
//!   dot2 (0x02)  dot5 (0x10)
//!   dot3 (0x04)  dot6 (0x20)
//!   dot7 (0x40)  dot8 (0x80)
//! ```

const BRAILLE_BASE: u32 = 0x2800;
/// Left column, all four dots (1,2,3,7).
const LEFT_COL: u8 = 0x47;
/// Right column, all four dots (4,5,6,8).
const RIGHT_COL: u8 = 0xB8;
/// Left-column dot bits bottom→top: 7,3,2,1.
const LEFT_DOTS: [u8; 4] = [0x40, 0x04, 0x02, 0x01];
/// Right-column dot bits bottom→top: 8,6,5,4.
const RIGHT_DOTS: [u8; 4] = [0x80, 0x20, 0x10, 0x08];

fn braille(bits: u8) -> char {
    // 0x2800..=0x28FF are all valid scalar values, so this never fails.
    char::from_u32(BRAILLE_BASE + u32::from(bits)).unwrap_or('⠀')
}

/// A horizontal braille magnitude bar of `width` cells for `ratio` in `[0, 1]`
/// (clamped). Each cell holds two dot-columns, so the effective resolution is
/// `2 * width`. Empty cells render as blank braille to preserve alignment.
#[must_use]
pub fn dot_bar(ratio: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let ratio = ratio.clamp(0.0, 1.0);
    let total_cols = width * 2;
    // `ratio` is clamped to [0,1], so the product lands in [0, total_cols] and
    // round() keeps it non-negative — the cast cannot truncate or lose a sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = (ratio * total_cols as f64).round() as usize;

    let mut s = String::with_capacity(width);
    for i in 0..width {
        let cols = filled.saturating_sub(i * 2);
        let bits = match cols {
            0 => 0,
            1 => LEFT_COL,
            _ => LEFT_COL | RIGHT_COL,
        };
        s.push(braille(bits));
    }
    s
}

/// Bottom-anchored dot bits for a single column at `level` (0..=4).
fn column_bits(level: u8, dots: [u8; 4]) -> u8 {
    let mut b = 0;
    for (i, dot) in dots.iter().enumerate() {
        if level as usize > i {
            b |= dot;
        }
    }
    b
}

/// A compact braille sparkline of `width` cells over `values`, scaled to the
/// series maximum. Two samples share each cell (left column, then right), each
/// drawn as a bottom-anchored bar of 0..4 dots. Returns an empty string for an
/// empty series or zero width.
#[must_use]
pub fn sparkline(values: &[f64], width: usize) -> String {
    if width == 0 || values.is_empty() {
        return String::new();
    }
    let max = values.iter().copied().fold(f64::MIN, f64::max);
    let cols = width * 2;

    // Resample the series to exactly `cols` points (nearest-neighbour).
    let level_at = |c: usize| -> u8 {
        let idx = if cols == 1 {
            0
        } else {
            c * (values.len() - 1) / (cols - 1)
        };
        if max > 0.0 {
            // value/max is in [_, 1]; *4 and round lands in [0,4] after clamp,
            // so the cast is non-negative and in range.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let level = (values[idx] / max * 4.0).round().clamp(0.0, 4.0) as u8;
            level
        } else {
            0
        }
    };

    let mut s = String::with_capacity(width);
    for cell in 0..width {
        let left = column_bits(level_at(cell * 2), LEFT_DOTS);
        let right = column_bits(level_at(cell * 2 + 1), RIGHT_DOTS);
        s.push(braille(left | right));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_bar_zero_width_is_empty() {
        assert_eq!(dot_bar(0.5, 0), "");
    }

    #[test]
    fn dot_bar_full_is_all_solid_cells() {
        assert_eq!(dot_bar(1.0, 4), "⣿⣿⣿⣿");
    }

    #[test]
    fn dot_bar_empty_is_blank_braille() {
        assert_eq!(dot_bar(0.0, 3), "⠀⠀⠀");
    }

    #[test]
    fn dot_bar_half_fills_left_half() {
        // 4 cells = 8 columns; 0.5 -> 4 filled columns -> 2 solid cells.
        assert_eq!(dot_bar(0.5, 4), "⣿⣿⠀⠀");
    }

    #[test]
    fn dot_bar_clamps_out_of_range() {
        assert_eq!(dot_bar(-1.0, 2), dot_bar(0.0, 2));
        assert_eq!(dot_bar(5.0, 2), dot_bar(1.0, 2));
    }

    #[test]
    fn dot_bar_odd_column_uses_left_only() {
        // 2 cells = 4 columns; 0.25 -> 1 filled column -> left col of cell 0.
        assert_eq!(dot_bar(0.25, 2), "⡇⠀");
    }

    #[test]
    fn sparkline_empty_series_or_zero_width_is_empty() {
        assert_eq!(sparkline(&[], 4), "");
        assert_eq!(sparkline(&[1.0, 2.0], 0), "");
    }

    #[test]
    fn sparkline_has_requested_width() {
        assert_eq!(sparkline(&[1.0, 2.0, 3.0, 4.0], 5).chars().count(), 5);
    }

    #[test]
    fn sparkline_flat_max_series_is_solid() {
        assert_eq!(sparkline(&[5.0, 5.0, 5.0, 5.0], 2), "⣿⣿");
    }

    #[test]
    fn sparkline_rises_left_to_right() {
        let s: Vec<char> = sparkline(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], 4)
            .chars()
            .collect();
        // Later cells carry at least as many lit dots as earlier ones.
        let lit = |c: char| (c as u32 - BRAILLE_BASE).count_ones();
        assert!(lit(s[3]) >= lit(s[0]));
        assert!(lit(*s.last().unwrap()) > 0);
    }

    #[test]
    fn sparkline_all_zero_is_blank() {
        assert_eq!(sparkline(&[0.0, 0.0, 0.0], 2), "⠀⠀");
    }
}
