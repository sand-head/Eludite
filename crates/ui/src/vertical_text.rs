//! Text rotated 90 degrees clockwise, reading top to bottom: Visual Studio's
//! auto-hide tabs on the left and right edges.
//!
//! GPUI at the pinned revision paints glyphs without a transform, and its
//! glyph rasterizer (`TextSystem::rasterize_glyph`, `raster_bounds`) is
//! crate-private, so a label cannot be rotated as glyph sprites. What is public
//! is `Window::paint_svg` with caller-supplied SVG bytes: GPUI's SVG renderer
//! (resvg) converts `<text>` to outlines with the system fonts, rasterizes an
//! alpha mask at the window's scale factor (supersampled) and caches it in the
//! sprite atlas, and the sprite is tinted with the current text color on the
//! GPU. [`vertical_label`] builds that SVG once per (text, font, size), sized
//! by GPUI's own shaping of the label, with the rotation inside the SVG.
//!
//! Public API: [`vertical_label`], [`VerticalLabel`], [`rotated_label`],
//! [`RotatedLabel`] and [`RotatedLabelCache`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui::{
    App, Bounds, Element, ElementId, Font, FontStyle, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, SharedString, Size, Style, TextRun, TransformationMatrix,
    Window, px, size,
};

/// A label laid out rotated: `size.width` is the line box, `size.height` the
/// shaped advance of the text.
#[derive(Debug)]
pub struct RotatedLabel {
    /// SVG document drawing the text rotated 90 degrees clockwise, in logical
    /// pixels.
    pub svg: Arc<[u8]>,
    /// The sprite atlas key GPUI caches the rasterized mask under.
    pub key: SharedString,
    /// Logical size of the rotated label.
    pub size: Size<Pixels>,
}

type Key = (SharedString, Font, u32);

/// Rotated labels by (text, font, font size). A GPUI global: each label is
/// shaped and written once; GPUI's sprite atlas then holds one raster per
/// device size, so a frame only paints a cached sprite.
#[derive(Default)]
pub struct RotatedLabelCache {
    labels: HashMap<Key, Arc<RotatedLabel>>,
}

impl gpui::Global for RotatedLabelCache {}

impl RotatedLabelCache {
    /// Number of distinct labels built so far.
    pub fn len(&self) -> usize {
        self.labels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    /// The cached label for these inputs, if one was built.
    pub fn get(&self, text: &str, font: &Font, font_size: Pixels) -> Option<Arc<RotatedLabel>> {
        self.labels
            .get(&(
                SharedString::from(text.to_owned()),
                font.clone(),
                font_size.as_f32().to_bits(),
            ))
            .cloned()
    }
}

/// The rotated label for `text` in `font` at `font_size`, built on first use
/// and cached in [`RotatedLabelCache`].
pub fn rotated_label(
    text: &SharedString,
    font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Arc<RotatedLabel> {
    let key = (text.clone(), font.clone(), font_size.as_f32().to_bits());
    if let Some(hit) = cx
        .try_global::<RotatedLabelCache>()
        .and_then(|c| c.labels.get(&key))
    {
        return hit.clone();
    }
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.clone(), font_size, &[run], None);
    let ascent = line.ascent.as_f32().abs();
    let descent = line.descent.as_f32().abs();
    let advance = line.width.as_f32().ceil();
    let thickness = (ascent + descent).ceil();
    let family = svg_family(font, cx);
    let cache = cx.default_global::<RotatedLabelCache>();
    let label = Arc::new(RotatedLabel {
        svg: rotated_svg(
            text,
            &family,
            font,
            font_size.as_f32(),
            advance,
            thickness,
            (ascent, descent),
        )
        .into_bytes()
        .into(),
        key: format!("eludite-ui/rotated-label/{}", cache.labels.len()).into(),
        size: size(px(thickness), px(advance)),
    });
    cache.labels.insert(key, label.clone());
    label
}

/// The CSS font-family list for the face GPUI actually resolved `font` to,
/// so the SVG renderer draws the same family the rest of the UI uses.
fn svg_family(font: &Font, cx: &App) -> String {
    let ts = cx.text_system();
    let id = ts.resolve_font(font);
    let resolved = ts
        .get_font_for_id(id)
        .map(|f| f.family)
        .unwrap_or_else(|| font.family.clone());
    let name = gpui::font_name_with_fallbacks(&resolved, "system-ui");
    if name == "system-ui" {
        "system-ui, sans-serif".into()
    } else {
        format!("'{}', sans-serif", xml_escape(name).replace('\'', "&apos;"))
    }
}

/// An SVG `thickness` wide and `advance` tall drawing `text` rotated 90
/// degrees clockwise (glyph tops face right), centered across its width.
/// `textLength` pins the run to GPUI's shaped advance so the layout and the
/// drawing agree even if the two shapers differ by a fraction of a pixel.
fn rotated_svg(
    text: &str,
    family: &str,
    font: &Font,
    font_size: f32,
    advance: f32,
    thickness: f32,
    (ascent, descent): (f32, f32),
) -> String {
    // Rotating by 90 degrees maps the baseline's normal onto -x: the glyph
    // box [baseline - ascent, baseline + descent] lands on [tx - descent,
    // tx + ascent]. Put the descent edge at x = 0.
    let tx = descent + (thickness - ascent - descent) / 2.;
    let style = match font.style {
        FontStyle::Normal => "normal",
        FontStyle::Italic => "italic",
        FontStyle::Oblique => "oblique",
    };
    let mut s = String::with_capacity(320 + text.len());
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{thickness}\" height=\"{advance}\" \
         viewBox=\"0 0 {thickness} {advance}\"><text x=\"0\" y=\"0\" \
         transform=\"translate({tx} 0) rotate(90)\" font-family=\"{family}\" \
         font-size=\"{font_size}\" font-weight=\"{}\" font-style=\"{style}\" \
         textLength=\"{advance}\" lengthAdjust=\"spacing\" fill=\"#000\">{}</text></svg>",
        font.weight.0,
        xml_escape(text),
    );
    s
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// A label drawn rotated 90 degrees clockwise in the inherited text style
/// (font, size and color, so a parent's hover color applies). It has no
/// hitbox of its own: the parent keeps hover and click handling.
pub struct VerticalLabel {
    text: SharedString,
}

/// See [`VerticalLabel`].
pub fn vertical_label(text: impl Into<SharedString>) -> VerticalLabel {
    VerticalLabel { text: text.into() }
}

impl IntoElement for VerticalLabel {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for VerticalLabel {
    type RequestLayoutState = Arc<RotatedLabel>;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let label = rotated_label(&self.text, &style.font(), font_size, window, cx);
        let layout = Style {
            size: size(label.size.width.into(), label.size.height.into()),
            flex_shrink: 0.,
            ..Style::default()
        };
        (window.request_layout(layout, [], cx), label)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        label: &mut Self::RequestLayoutState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if label.size.width <= px(0.) || label.size.height <= px(0.) {
            return;
        }
        let color = window.text_style().color;
        let bounds = Bounds::new(bounds.origin, label.size);
        let _ = window.paint_svg(
            bounds,
            label.key.clone(),
            Some(&label.svg),
            TransformationMatrix::unit(),
            color,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_is_tall_rotated_and_escaped() {
        let svg = rotated_svg(
            "A&B <x>",
            "'Noto Sans', sans-serif",
            &Font::default(),
            11.,
            60.,
            15.,
            (11., 3.),
        );
        assert!(svg.contains("width=\"15\" height=\"60\""));
        assert!(svg.contains("rotate(90)"));
        assert!(svg.contains("A&amp;B &lt;x&gt;"));
        assert!(svg.contains("textLength=\"60\""));
    }
}
