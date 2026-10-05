//! The Agents window's transcript pieces (brief 0016, polished by brief 0058): the person's prompt, a block of the
//! agent's Markdown, the collapsed thinking block, a tool call card (one line with the kind's glyph, the adapter's
//! title, the status badge and a chevron; the arguments and the result folded under it), a plan, notices, the status
//! badge, the usage strip above the prompt box ([`UsageStrip`]), and the status line of a running turn with its
//! spinner ([`spinner_frame`]).
//! Stateless: the shell owns the transcript, the expansion state, the clock and the click handlers. Every color comes
//! from the [`Theme`]; the only literals are the error red and the running blue in [`ToolStatus::color`], picked for
//! a dark or a light panel.

use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Div, ElementId, Font, FontWeight, InteractiveElement, ParentElement, Rgba, SharedString,
    Stateful, Styled, div, px, relative, rgb,
};

use crate::Theme;
use crate::markdown::{self, Block};

/// A tool call's state as the transcript shows it (brief 0016 Contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Announced, not started.
    Pending,
    Running,
    /// Permission granted by policy, without asking.
    AllowedWithoutPrompt,
    /// Waiting for the user's answer.
    AwaitingPermission,
    /// Waiting for the user's review of its edits.
    AwaitingReview,
    Denied,
    Failed,
    Completed,
}

/// Whether `theme`'s panels are dark (VS Dark), so text colors need to be light.
fn is_dark(theme: &Theme) -> bool {
    let p = theme.panel;
    0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b < 0.5
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            ToolStatus::Pending => "pending",
            ToolStatus::Running => "running",
            ToolStatus::AllowedWithoutPrompt => "allowed without prompt",
            ToolStatus::AwaitingPermission => "awaiting permission",
            ToolStatus::AwaitingReview => "awaiting review",
            ToolStatus::Denied => "denied",
            ToolStatus::Failed => "failed",
            ToolStatus::Completed => "completed",
        }
    }

    /// The badge's color in `theme`: `success`, `warning`, and a red and a blue readable on its panel. The Agents
    /// window draws its errors in [`ToolStatus::Failed`]'s color and a running state in [`ToolStatus::Running`]'s.
    pub fn color(self, theme: &Theme) -> Rgba {
        let dark = is_dark(theme);
        match self {
            ToolStatus::Completed | ToolStatus::AllowedWithoutPrompt => theme.success,
            ToolStatus::Denied | ToolStatus::Failed if dark => rgb(0xF48771),
            ToolStatus::Denied | ToolStatus::Failed => rgb(0xC4314B),
            ToolStatus::AwaitingPermission | ToolStatus::AwaitingReview => theme.warning,
            ToolStatus::Pending | ToolStatus::Running if dark => rgb(0x3794FF),
            ToolStatus::Pending | ToolStatus::Running => rgb(0x0E639C),
        }
    }
}

/// The ACP tool kinds (`ToolKind`), `other` last.
pub const TOOL_KINDS: [&str; 10] = [
    "read",
    "edit",
    "delete",
    "move",
    "search",
    "execute",
    "think",
    "fetch",
    "switch_mode",
    "other",
];

/// The glyph a tool call's line starts with, by its ACP `kind` (anything unknown is `other`'s).
pub fn kind_glyph(kind: &str) -> &'static str {
    match kind {
        "read" => "\u{25A4}",
        "edit" => "\u{270E}",
        "delete" => "\u{2715}",
        "move" => "\u{21C4}",
        "search" => "\u{2315}",
        "execute" => "\u{25B6}",
        "think" => "\u{2026}",
        "fetch" => "\u{21E3}",
        "switch_mode" => "\u{21C5}",
        _ => "\u{2022}",
    }
}

/// The spinner of a running tool call and a running turn: one frame per [`SPINNER_STEP`].
pub const SPINNER: [&str; 10] = [
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];

/// How long each spinner frame shows.
pub const SPINNER_STEP: Duration = Duration::from_millis(100);

/// The spinner's frame `elapsed` after it started.
pub fn spinner_frame(elapsed: Duration) -> &'static str {
    let step = (elapsed.as_millis() / SPINNER_STEP.as_millis()) as usize;
    SPINNER[step % SPINNER.len()]
}

/// A running turn's elapsed time: `0:12`, `1:05`, `72:00`.
pub fn elapsed_text(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// `1234` as `1.2k`, `259826` as `260k`, `1000000` as `1M`, `1500000` as `1.5M` (brief 0034; a trailing `.0` is
/// dropped).
pub fn tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    let n = n as f64;
    let s = if n < 1e4 {
        format!("{:.1}k", n / 1e3)
    } else if n < 1e6 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{:.1}M", n / 1e6)
    };
    s.replace(".0k", "k").replace(".0M", "M")
}

/// An amount of money as the transcript shows it: `$0.95` in US dollars, else `0.95 EUR`.
pub fn money(amount: f64, currency: &str) -> String {
    if currency == "USD" {
        format!("${amount:.2}")
    } else {
        format!("{amount:.2} {currency}")
    }
}

/// The context bar's width.
pub const USAGE_BAR_WIDTH: f32 = 80.;
/// The fraction of the context window from which the bar is drawn in `warning`.
pub const USAGE_WARNING: f32 = 0.8;
/// What the strip says before the agent's first `usage_update`.
pub const NO_USAGE: &str = "No usage yet";

/// What the usage strip shows (brief 0058): the session's context and its running cost, from the agent's last
/// `usage_update`.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageStrip {
    /// Tokens in context.
    pub used: u64,
    /// The context window; 0 when the agent does not know it (no bar).
    pub size: u64,
    /// The session's cost so far: (amount, currency).
    pub cost: Option<(f64, String)>,
}

impl UsageStrip {
    /// `61k of 1M · $0.95`; `61k tokens` without a window size; no cost part without a cost.
    pub fn text(&self) -> String {
        let mut s = if self.size > 0 {
            format!("{} of {}", tokens(self.used), tokens(self.size))
        } else {
            format!("{} tokens", tokens(self.used))
        };
        if let Some((amount, currency)) = &self.cost {
            s.push_str(" \u{B7} ");
            s.push_str(&money(*amount, currency));
        }
        s
    }

    /// How full the context window is (0 to 1), when its size is known.
    pub fn fraction(&self) -> Option<f32> {
        (self.size > 0).then(|| (self.used as f64 / self.size as f64).clamp(0., 1.) as f32)
    }

    /// The bar's fill: the accent color, `warning` from [`USAGE_WARNING`]; `None` draws no bar.
    pub fn bar_color(&self, theme: &Theme) -> Option<Rgba> {
        let f = self.fraction()?;
        Some(if f >= USAGE_WARNING {
            theme.warning
        } else {
            theme.accent
        })
    }
}

/// The usage strip: the context bar ([`USAGE_BAR_WIDTH`] by 4 px), then [`UsageStrip::text`], small and muted;
/// [`NO_USAGE`] without one. The caller adds the id and the tooltip.
pub fn usage_strip(strip: Option<&UsageStrip>, theme: &Theme) -> Div {
    let row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(20.))
        .text_size(theme.typography.small)
        .text_color(theme.text_muted);
    let Some(strip) = strip else {
        return row.child(NO_USAGE);
    };
    let bar = strip.bar_color(theme).map(|fill| {
        let width = USAGE_BAR_WIDTH * strip.fraction().unwrap_or(0.);
        div()
            .flex_none()
            .w(px(USAGE_BAR_WIDTH))
            .h(px(4.))
            .bg(theme.border)
            .child(div().w(px(width)).h_full().bg(fill))
    });
    row.children(bar).child(
        div()
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .child(SharedString::from(strip.text())),
    )
}

/// The line under the transcript while a turn runs: the spinner, then `text` (`Claude Code is working… 0:12 · Esc to
/// stop`), small and muted. The caller adds the id.
pub fn status_line(text: impl Into<SharedString>, spinner: &'static str, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(20.))
        .text_size(theme.typography.small)
        .text_color(theme.text_muted)
        .child(
            div()
                .flex_none()
                .w(px(12.))
                .text_color(ToolStatus::Running.color(theme))
                .child(spinner),
        )
        .child(div().flex_1().min_w(px(0.)).truncate().child(text.into()))
}

/// A small colored label; a running call's carries the spinner's frame.
pub fn status_badge(status: ToolStatus, spinner: Option<&'static str>, theme: &Theme) -> Div {
    let color = status.color(theme);
    div()
        .flex()
        .flex_none()
        .gap_1()
        .px_1()
        .border_1()
        .border_color(color)
        .text_color(color)
        .text_size(theme.typography.small)
        .children(spinner)
        .child(status.label())
}

/// The person's prompt: a block on `panel_raised` with a 3 px accent border on the left, the body type size, wrapped,
/// with `time` (`14:02`) small and muted at the right of its first line.
pub fn user_prompt(text: impl Into<SharedString>, time: &str, theme: &Theme) -> Div {
    div().w_full().px_3().py_1().child(
        div()
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_1()
            .bg(theme.panel_raised)
            .border_l(px(3.))
            .border_color(theme.accent)
            .text_color(theme.text)
            .text_size(theme.typography.body)
            .line_height(relative(1.4))
            .child(div().flex_1().min_w(px(0.)).child(text.into()))
            .child(
                div()
                    .flex_none()
                    .text_size(theme.typography.small)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(time.to_owned())),
            ),
    )
}

/// One top-level Markdown block of the agent's message, drawn by [`markdown::render_linked`]: text in `font` at the
/// body size with a line height of 1.4, code in `mono`, blocks 8 px apart; a click on a link calls `on_link` with its
/// target. `id` is unique among the transcript's rows.
pub fn agent_block(
    id: impl Into<ElementId>,
    blocks: &[Block],
    theme: &Theme,
    font: &Font,
    mono: &Font,
    on_link: markdown::OnLink,
) -> Div {
    let block = div()
        .w_full()
        .px_3()
        .text_color(theme.text)
        .text_size(theme.typography.body)
        .line_height(relative(1.4));
    if blocks.is_empty() {
        block
    } else {
        block.pb_2().child(markdown::render_linked(
            id, blocks, theme, font, mono, on_link,
        ))
    }
}

/// The agent's thinking: one muted line, `label` (`Thinking…` while it streams, `Thought for 4 s` after), with the
/// whole text under it when expanded. The caller adds the click that toggles it.
pub fn thought_block(
    id: impl Into<SharedString>,
    label: &str,
    text: &str,
    expanded: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let line = div()
        .flex()
        .gap_1()
        .child(
            div()
                .flex_none()
                .w(px(10.))
                .child(if expanded { "\u{25BE}" } else { "\u{25B8}" }),
        )
        .child(SharedString::from(label.to_owned()));
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .w_full()
        .px_3()
        .pb_2()
        .cursor_pointer()
        .text_color(theme.text_muted)
        .child(line)
        .when_some(expanded.then(|| text.to_owned()), |d, text| {
            d.child(
                div()
                    .pl(px(14.))
                    .pt_1()
                    .italic()
                    .child(SharedString::from(text)),
            )
        })
}

/// `text` cut to `max` lines, then `… N more lines`.
pub fn clip_lines(text: &str, max: usize) -> String {
    let total = text.lines().count();
    if total <= max {
        return text.to_owned();
    }
    let mut out: Vec<&str> = text.lines().take(max).collect();
    let more = format!("\u{2026} {} more lines", total - max);
    out.push(&more);
    out.join("\n")
}

/// A tool call card (brief 0058): one line, the kind's glyph, the adapter's `title`, the status badge at the right and
/// a chevron; expanded, the arguments (in the mono font) and the result under it. The note (how permission was
/// decided, the audit entry, the changes) shows whether or not it is expanded. `grouped` draws the 2 px rule on the
/// left that makes consecutive calls of a turn read as one group. The caller adds the click that toggles it and the
/// tooltip (the tool's name).
pub struct ToolCard<'a> {
    pub id: SharedString,
    pub title: &'a str,
    pub kind: &'a str,
    pub status: ToolStatus,
    /// The spinner's frame while the call runs.
    pub spinner: Option<&'static str>,
    pub expanded: bool,
    pub grouped: bool,
    pub arguments: &'a str,
    pub result: &'a str,
    pub note: Option<&'a str>,
}

pub fn tool_call_card(card: ToolCard<'_>, theme: &Theme, mono: SharedString) -> Stateful<Div> {
    let selector = card.id.clone();
    let line = div()
        .flex()
        .gap_2()
        .items_center()
        .h(px(22.))
        .child(
            div()
                .flex_none()
                .w(px(14.))
                .text_color(theme.text_muted)
                .child(kind_glyph(card.kind)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme.text)
                .child(SharedString::from(card.title.to_owned())),
        )
        .child(status_badge(card.status, card.spinner, theme))
        .child(
            div()
                .flex_none()
                .w(px(10.))
                .text_color(theme.text_muted)
                .child(if card.expanded {
                    "\u{25BE}"
                } else {
                    "\u{25B8}"
                }),
        );
    let mut body = div().flex().flex_col().gap_1().child(line);
    if card.expanded && !card.arguments.is_empty() {
        body = body.child(
            div()
                .ml(px(22.))
                .p_1()
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .font_family(mono.clone())
                .text_size(theme.typography.small)
                .text_color(theme.text_muted)
                .child(SharedString::from(card.arguments.to_owned())),
        );
    }
    if card.expanded && !card.result.is_empty() {
        body = body.child(
            div()
                .ml(px(22.))
                .font_family(mono)
                .text_size(theme.typography.small)
                .text_color(theme.text)
                .child(SharedString::from(card.result.to_owned())),
        );
    }
    if let Some(note) = card.note {
        body = body.child(
            div()
                .ml(px(22.))
                .text_size(theme.typography.small)
                .text_color(theme.text_muted)
                .child(SharedString::from(note.to_owned())),
        );
    }
    div()
        .id(card.id)
        .debug_selector(move || selector.to_string())
        .w_full()
        .px_3()
        .cursor_pointer()
        .child(
            div()
                .pl_2()
                .border_l_2()
                .border_color(if card.grouped {
                    theme.border
                } else {
                    gpui::transparent_black().into()
                })
                .child(body),
        )
}

/// `2 of 5 done`: the plan's completed entries of all of them.
pub fn plan_progress(entries: &[(String, String)]) -> String {
    let done = entries.iter().filter(|(_, s)| s == "completed").count();
    format!("{done} of {} done", entries.len())
}

/// A plan update: the `Plan` title with [`plan_progress`], then one line per entry with its status mark.
pub fn plan_card(entries: &[(String, String)], theme: &Theme) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .gap_px()
        .p_2()
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child("Plan"),
                )
                .child(
                    div()
                        .text_size(theme.typography.small)
                        .text_color(theme.text_muted)
                        .child(SharedString::from(plan_progress(entries))),
                ),
        );
    for (content, status) in entries {
        let mark = match status.as_str() {
            "completed" => "\u{2611}",
            "in_progress" => "\u{25B6}",
            _ => "\u{2610}",
        };
        col = col.child(
            div()
                .flex()
                .gap_1()
                .text_color(if status == "completed" {
                    theme.text_muted
                } else {
                    theme.text
                })
                .child(mark)
                .child(SharedString::from(content.clone())),
        );
    }
    div().w_full().px_3().pb_2().child(col)
}

/// A muted line: turn ends other than `end_turn`, state changes, login instructions; an error in
/// [`ToolStatus::Failed`]'s color.
pub fn notice(text: impl Into<SharedString>, error: bool, theme: &Theme) -> Div {
    div()
        .w_full()
        .px_3()
        .pb_2()
        .text_size(theme.typography.small)
        .text_color(if error {
            ToolStatus::Failed.color(theme)
        } else {
            theme.text_muted
        })
        .child(text.into())
}

/// A turn's usage line (brief 0034): right-aligned, small and muted.
pub fn usage_line(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .w_full()
        .flex()
        .justify_end()
        .px_3()
        .pb_2()
        .text_size(theme.typography.small)
        .text_color(theme.text_muted)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_have_labels_and_distinct_alarm_colors() {
        use ToolStatus::*;
        let all = [
            Pending,
            Running,
            AllowedWithoutPrompt,
            AwaitingPermission,
            AwaitingReview,
            Denied,
            Failed,
            Completed,
        ];
        let mut labels: Vec<_> = all.iter().map(|s| s.label()).collect();
        labels.dedup();
        assert_eq!(labels.len(), all.len());
        assert_eq!(AllowedWithoutPrompt.label(), "allowed without prompt");
        for t in Theme::all() {
            assert_ne!(Denied.color(&t), Completed.color(&t), "{}", t.name);
            assert_ne!(
                AwaitingPermission.color(&t),
                Running.color(&t),
                "{}",
                t.name
            );
            assert_eq!(Denied.color(&t), Failed.color(&t), "{}", t.name);
            assert_eq!(Completed.color(&t), t.success);
            assert_eq!(AwaitingReview.color(&t), t.warning);
        }
        // The red and the blue are lighter on the dark panel than on the light ones.
        assert_ne!(
            Failed.color(&Theme::vs_dark()),
            Failed.color(&Theme::vs_light())
        );
    }

    #[test]
    fn every_acp_tool_kind_has_its_own_glyph_and_unknown_kinds_are_other() {
        let glyphs: Vec<&str> = TOOL_KINDS.iter().map(|k| kind_glyph(k)).collect();
        let mut distinct = glyphs.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), TOOL_KINDS.len(), "{glyphs:?}");
        assert_eq!(kind_glyph("read"), "\u{25A4}");
        assert_eq!(kind_glyph("execute"), "\u{25B6}");
        assert_eq!(kind_glyph("switch_mode"), "\u{21C5}");
        assert_eq!(kind_glyph("other"), "\u{2022}");
        assert_eq!(kind_glyph("teleport"), kind_glyph("other"));
        assert_eq!(kind_glyph(""), kind_glyph("other"));
    }

    #[test]
    fn the_spinner_steps_every_100_ms_and_wraps() {
        assert_eq!(spinner_frame(Duration::ZERO), "\u{280B}");
        assert_eq!(spinner_frame(Duration::from_millis(99)), "\u{280B}");
        assert_eq!(spinner_frame(Duration::from_millis(100)), "\u{2819}");
        assert_eq!(spinner_frame(Duration::from_millis(950)), "\u{280F}");
        assert_eq!(spinner_frame(Duration::from_millis(1000)), "\u{280B}");
        assert_eq!(spinner_frame(Duration::from_millis(1234)), SPINNER[2]);
        assert_eq!(elapsed_text(Duration::from_millis(12_400)), "0:12");
        assert_eq!(elapsed_text(Duration::from_secs(65)), "1:05");
        assert_eq!(elapsed_text(Duration::from_secs(4320)), "72:00");
    }

    #[test]
    fn the_strip_says_the_context_and_the_cost_and_warns_from_80_percent() {
        let t = Theme::vs_dark();
        let strip = |used, size, cost: Option<(f64, &str)>| UsageStrip {
            used,
            size,
            cost: cost.map(|(a, c)| (a, c.to_owned())),
        };
        // The fake agent's `stream` turn: 61,204 of 1,000,000, $0.9512.
        let s = strip(61_204, 1_000_000, Some((0.9512, "USD")));
        assert_eq!(s.text(), "61k of 1M \u{B7} $0.95");
        assert_eq!(s.bar_color(&t), Some(t.accent));
        // Size 0: no bar.
        let s = strip(61_204, 0, None);
        assert_eq!(s.text(), "61k tokens");
        assert_eq!(s.fraction(), None);
        assert_eq!(s.bar_color(&t), None);
        // 20 percent: the accent.
        let s = strip(40_000, 200_000, None);
        assert_eq!(s.text(), "40k of 200k");
        assert_eq!(s.fraction(), Some(0.2));
        assert_eq!(s.bar_color(&t), Some(t.accent));
        // 85 percent: the warning color, in every theme.
        for th in Theme::all() {
            let s = strip(170_000, 200_000, None);
            assert_eq!(s.bar_color(&th), Some(th.warning), "{}", th.name);
        }
        assert_eq!(strip(160_000, 200_000, None).bar_color(&t), Some(t.warning));
        // Over the window clamps the bar.
        assert_eq!(strip(300_000, 200_000, None).fraction(), Some(1.0));
        // Another currency.
        assert_eq!(
            strip(1_500_000, 2_000_000, Some((12.5, "EUR"))).text(),
            "1.5M of 2M \u{B7} 12.50 EUR"
        );
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1k");
        assert_eq!(tokens(2_897), "2.9k");
        assert_eq!(tokens(259_826), "260k");
        assert_eq!(money(0.25, "USD"), "$0.25");
    }

    #[test]
    fn long_arguments_are_clipped_by_lines_and_plans_count_what_is_done() {
        let text: String = (1..=45)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let clipped = clip_lines(&text, 40);
        assert_eq!(clipped.lines().count(), 41);
        assert!(
            clipped.ends_with("line 40\n\u{2026} 5 more lines"),
            "{clipped}"
        );
        assert_eq!(clip_lines("a\nb", 40), "a\nb");
        let entries = [
            ("a".to_owned(), "completed".to_owned()),
            ("b".to_owned(), "completed".to_owned()),
            ("c".to_owned(), "in_progress".to_owned()),
            ("d".to_owned(), "pending".to_owned()),
            ("e".to_owned(), "pending".to_owned()),
        ];
        assert_eq!(plan_progress(&entries), "2 of 5 done");
    }
}
