//! Tab expansion and column conversion for one line of text.
//!
//! Tabs are shown as spaces up to the next multiple of [`TAB_SIZE`]. The
//! shaped (display) string replaces each tab with that many spaces, so a
//! byte offset into the display string differs from one into the buffer
//! line only by the extra width of earlier tabs.

use std::borrow::Cow;

/// Columns per tab stop (Visual Studio's default for C#).
pub const TAB_SIZE: u32 = 4;

/// The line with tabs expanded to spaces.
pub fn expand_tabs(line: &str) -> Cow<'_, str> {
    if !line.contains('\t') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0u32;
    for ch in line.chars() {
        if ch == '\t' {
            let n = TAB_SIZE - col % TAB_SIZE;
            out.extend(std::iter::repeat_n(' ', n as usize));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    Cow::Owned(out)
}

/// Visual column (characters, tabs expanded) of byte offset `byte_col`.
pub fn visual_column(line: &str, byte_col: usize) -> u32 {
    let mut col = 0u32;
    for (ix, ch) in line.char_indices() {
        if ix >= byte_col {
            break;
        }
        col += if ch == '\t' {
            TAB_SIZE - col % TAB_SIZE
        } else {
            1
        };
    }
    col
}

/// Byte offset of the character at visual column `target`, or the line
/// length if the line is shorter. A column inside a tab maps to the tab.
pub fn byte_for_visual_column(line: &str, target: u32) -> usize {
    let mut col = 0u32;
    for (ix, ch) in line.char_indices() {
        let w = if ch == '\t' {
            TAB_SIZE - col % TAB_SIZE
        } else {
            1
        };
        if col + w > target {
            return if target - col > w / 2 && ch == '\t' {
                ix + 1
            } else {
                ix
            };
        }
        col += w;
    }
    line.len()
}

/// Byte offset in the display string for byte offset `byte_col` in `line`.
pub fn to_display(line: &str, byte_col: usize) -> usize {
    if !line.contains('\t') {
        return byte_col.min(line.len());
    }
    let mut col = 0u32;
    let mut display = 0usize;
    for (ix, ch) in line.char_indices() {
        if ix >= byte_col {
            return display;
        }
        if ch == '\t' {
            let n = TAB_SIZE - col % TAB_SIZE;
            display += n as usize;
            col += n;
        } else {
            display += ch.len_utf8();
            col += 1;
        }
    }
    display
}

/// Byte offset in `line` for byte offset `display_col` in its display string.
pub fn from_display(line: &str, display_col: usize) -> usize {
    if !line.contains('\t') {
        return floor_char_boundary(line, display_col.min(line.len()));
    }
    let mut col = 0u32;
    let mut display = 0usize;
    for (ix, ch) in line.char_indices() {
        let w = if ch == '\t' {
            (TAB_SIZE - col % TAB_SIZE) as usize
        } else {
            ch.len_utf8()
        };
        if display + w > display_col {
            return if ch == '\t' && display_col - display > w / 2 {
                ix + 1
            } else {
                ix
            };
        }
        display += w;
        col += if ch == '\t' { w as u32 } else { 1 };
    }
    line.len()
}

pub fn floor_char_boundary(s: &str, mut ix: usize) -> usize {
    ix = ix.min(s.len());
    while !s.is_char_boundary(ix) {
        ix -= 1;
    }
    ix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
        assert_eq!(visual_column("ab\tc", 3), 4);
        assert_eq!(to_display("ab\tc", 3), 4);
        assert_eq!(from_display("ab\tc", 4), 3);
        assert_eq!(from_display("ab\tc", 2), 2);
        assert_eq!(byte_for_visual_column("ab\tc", 4), 3);
        assert_eq!(byte_for_visual_column("ab", 9), 2);
    }

    #[test]
    fn multibyte_columns() {
        let line = "añb";
        assert_eq!(visual_column(line, 3), 2);
        assert_eq!(byte_for_visual_column(line, 2), 3);
        assert_eq!(from_display(line, 2), 1);
    }
}
