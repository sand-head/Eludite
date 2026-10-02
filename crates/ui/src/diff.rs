//! The pending-change review view's pieces (brief 0016): a line diff ([`diff_lines`], Myers' algorithm after the
//! common prefix and suffix are set aside, with a bound on its work so a wholesale rewrite degrades to "all removed,
//! all added" instead of a slow diff), its hunks (where the gutter marks a pending change), and the inline diff row.

use gpui::{Div, ParentElement, Rgba, SharedString, Styled, div, px, rgba};

use crate::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

/// One row of an inline diff. Line numbers are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// A run of changed lines: `old_start..old_start + old_len` (0-based rows of the old text) became
/// `new_start..new_start + new_len`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
}

/// Work bound for the middle of a diff: `d * (n + m)` entries of Myers' trace.
const MAX_TRACE: usize = 8_000_000;

fn split(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let t = text.strip_suffix('\n').unwrap_or(text);
    t.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// The inline diff of `old` and `new`, line by line.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a = split(old);
    let b = split(new);
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 8);
    let same = |out: &mut Vec<DiffLine>, i: usize, j: usize, text: &str| {
        out.push(DiffLine {
            kind: DiffKind::Same,
            old: Some(i as u32 + 1),
            new: Some(j as u32 + 1),
            text: text.to_owned(),
        })
    };
    for (i, line) in a.iter().take(prefix).enumerate() {
        same(&mut out, i, i, line);
    }
    for op in middle(am, bm) {
        match op {
            Op::Same(i, j) => same(&mut out, prefix + i, prefix + j, am[i]),
            Op::Removed(i) => out.push(DiffLine {
                kind: DiffKind::Removed,
                old: Some((prefix + i) as u32 + 1),
                new: None,
                text: am[i].to_owned(),
            }),
            Op::Added(j) => out.push(DiffLine {
                kind: DiffKind::Added,
                old: None,
                new: Some((prefix + j) as u32 + 1),
                text: bm[j].to_owned(),
            }),
        }
    }
    for k in 0..suffix {
        let i = a.len() - suffix + k;
        let j = b.len() - suffix + k;
        same(&mut out, i, j, a[i]);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same(usize, usize),
    Removed(usize),
    Added(usize),
}

/// Myers' O((n + m) d) diff of the middle, or every line removed then added when it would cost too much.
fn middle(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let wholesale = || {
        (0..a.len())
            .map(Op::Removed)
            .chain((0..b.len()).map(Op::Added))
            .collect::<Vec<_>>()
    };
    if n == 0 || m == 0 {
        return wholesale();
    }
    let max = (n + m) as usize;
    let max_d = (MAX_TRACE / (2 * max + 1)).clamp(1, max);
    let off = max as isize;
    let mut v = vec![0isize; 2 * max + 1];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=max_d as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let Some(dmax) = found else {
        return wholesale();
    };
    // Walk back through the trace.
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=dmax).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[(k - 1 + off) as usize] < v[(k + 1 + off) as usize])
        {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[(prev_k + off) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            ops.push(Op::Same(x as usize, y as usize));
        }
        if x == prev_x {
            y -= 1;
            ops.push(Op::Added(y as usize));
        } else {
            x -= 1;
            ops.push(Op::Removed(x as usize));
        }
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        ops.push(Op::Same(x as usize, y as usize));
    }
    ops.reverse();
    ops
}

/// The runs of changed lines.
pub fn hunks(lines: &[DiffLine]) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let (mut old_row, mut new_row) = (0u32, 0u32);
    let mut open = false;
    for l in lines {
        match l.kind {
            DiffKind::Same => {
                open = false;
                old_row += 1;
                new_row += 1;
            }
            DiffKind::Removed | DiffKind::Added => {
                if !open {
                    out.push(Hunk {
                        old_start: old_row,
                        old_len: 0,
                        new_start: new_row,
                        new_len: 0,
                    });
                    open = true;
                }
                let h = out.last_mut().expect("opened");
                if l.kind == DiffKind::Removed {
                    h.old_len += 1;
                    old_row += 1;
                } else {
                    h.new_len += 1;
                    new_row += 1;
                }
            }
        }
    }
    out
}

/// Lines added and removed.
pub fn counts(lines: &[DiffLine]) -> (usize, usize) {
    lines.iter().fold((0, 0), |(a, r), l| match l.kind {
        DiffKind::Added => (a + 1, r),
        DiffKind::Removed => (a, r + 1),
        DiffKind::Same => (a, r),
    })
}

/// Row height of the review view.
pub const DIFF_ROW_HEIGHT: f32 = 18.;

/// The colors Visual Studio's diff viewer uses for added and removed lines (translucent, over the editor background).
pub fn diff_colors(kind: DiffKind) -> Option<Rgba> {
    match kind {
        DiffKind::Added => Some(rgba(0x37_8B_37_40)),
        DiffKind::Removed => Some(rgba(0xC8_3C_3C_40)),
        DiffKind::Same => None,
    }
}

/// One inline diff row: old and new line numbers, the sign, and the text in `mono`.
pub fn diff_row(line: &DiffLine, theme: &Theme, mono: SharedString) -> Div {
    let num = |n: Option<u32>| {
        div()
            .w(px(44.))
            .flex_none()
            .pr_1()
            .text_color(theme.text_muted)
            .child(SharedString::from(
                n.map(|n| n.to_string()).unwrap_or_default(),
            ))
    };
    let sign = match line.kind {
        DiffKind::Added => "+",
        DiffKind::Removed => "-",
        DiffKind::Same => " ",
    };
    let mut row = div()
        .flex()
        .w_full()
        .h(px(DIFF_ROW_HEIGHT))
        .items_center()
        .font_family(mono)
        .whitespace_nowrap()
        .child(num(line.old))
        .child(num(line.new))
        .child(div().w(px(14.)).flex_none().child(sign))
        .child(
            div()
                .text_color(theme.text)
                .child(SharedString::from(line.text.replace('\t', "    "))),
        );
    if let Some(bg) = diff_colors(line.kind) {
        row = row.bg(bg);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lines: &[DiffLine]) -> String {
        lines
            .iter()
            .map(|l| match l.kind {
                DiffKind::Same => '=',
                DiffKind::Added => '+',
                DiffKind::Removed => '-',
            })
            .collect()
    }

    #[test]
    fn diffs_insertions_deletions_and_replacements() {
        let old = "a\nb\nc\nd\n";
        let new = "// header\na\nb\nX\nd\n";
        let d = diff_lines(old, new);
        assert_eq!(kinds(&d), "+==-+=");
        assert_eq!(d[0].new, Some(1));
        assert_eq!(d[0].old, None);
        assert_eq!(d[3].old, Some(3));
        assert_eq!(d[4].new, Some(4));
        assert_eq!(
            hunks(&d),
            [
                Hunk {
                    old_start: 0,
                    old_len: 0,
                    new_start: 0,
                    new_len: 1
                },
                Hunk {
                    old_start: 2,
                    old_len: 1,
                    new_start: 3,
                    new_len: 1
                }
            ]
        );
        assert_eq!(counts(&d), (2, 1));
        // A new file, a deleted file, CRLF, no change.
        assert_eq!(kinds(&diff_lines("", "x\ny\n")), "++");
        assert_eq!(kinds(&diff_lines("x\r\ny\r\n", "")), "--");
        assert_eq!(kinds(&diff_lines("x\r\ny\r\n", "x\ny\n")), "==");
        assert!(hunks(&diff_lines("same\n", "same\n")).is_empty());
    }

    #[test]
    fn the_middle_is_minimal() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let new = "1\n2\nx\n4\n5\ny\n7\n8\n";
        let d = diff_lines(old, new);
        assert_eq!(kinds(&d), "==-+==-+==");
        // Reconstructs both sides.
        let old_side: Vec<_> = d
            .iter()
            .filter(|l| l.kind != DiffKind::Added)
            .map(|l| l.text.as_str())
            .collect();
        let new_side: Vec<_> = d
            .iter()
            .filter(|l| l.kind != DiffKind::Removed)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(old_side.join("\n") + "\n", old);
        assert_eq!(new_side.join("\n") + "\n", new);
    }

    #[test]
    fn a_2000_line_file_diffs_fast_and_a_rewrite_degrades() {
        let old: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        let new: String = (0..2000)
            .map(|i| {
                if i % 100 == 0 {
                    format!("changed {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        let t = std::time::Instant::now();
        let d = diff_lines(&old, &new);
        assert!(t.elapsed().as_millis() < 500, "{:?}", t.elapsed());
        assert_eq!(counts(&d), (20, 20));
        assert_eq!(hunks(&d).len(), 20);
        let rewrite: String = (0..2000).map(|i| format!("other {i}\n")).collect();
        let d = diff_lines(&old, &rewrite);
        assert_eq!(counts(&d), (2000, 2000));
    }
}
