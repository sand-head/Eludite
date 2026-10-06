//! Eludite's own type, embedded so the look does not depend on the fonts a machine has and nothing is fetched at
//! startup: Instrument Sans for the UI ([`UI_FONT`]: Regular, Medium, SemiBold, Bold and Italic) and JetBrains Mono for
//! code ([`MONO_FONT`]: Regular, Medium and Bold). Both are under the SIL Open Font License 1.1 (`OFL-1.1`); the
//! license texts are beside the files in `crates/ui/fonts/` and ship in the packages' `licenses/fonts/`. The files are
//! Fontsource's Latin and Latin Extended subsets merged into one TrueType file per face; other scripts fall back to
//! the platform's fonts.
//!
//! Public API: [`register`], called before the main window draws (once per app; later calls do nothing), and the
//! family names.

use std::borrow::Cow;

use gpui::{App, Global};

/// The UI's family.
pub const UI_FONT: &str = "Instrument Sans";
/// The family for code: the editor, Output, Find Results, diffs and code spans.
pub const MONO_FONT: &str = "JetBrains Mono";

const FILES: [&[u8]; 8] = [
    include_bytes!("../fonts/InstrumentSans-Regular.ttf"),
    include_bytes!("../fonts/InstrumentSans-Medium.ttf"),
    include_bytes!("../fonts/InstrumentSans-SemiBold.ttf"),
    include_bytes!("../fonts/InstrumentSans-Bold.ttf"),
    include_bytes!("../fonts/InstrumentSans-Italic.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Medium.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Bold.ttf"),
];

/// Marks an app whose text system has the faces.
struct Registered;

impl Global for Registered {}

/// Adds the embedded faces to the app's text system, once per app. If the platform cannot load them its own fonts
/// draw the text, so a failure is reported, never fatal.
pub fn register(cx: &mut App) -> Result<(), String> {
    if cx.has_global::<Registered>() {
        return Ok(());
    }
    cx.set_global(Registered);
    cx.text_system()
        .add_fonts(FILES.iter().map(|f| Cow::Borrowed(*f)).collect())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_is_a_truetype_file() {
        for f in FILES {
            assert_eq!(&f[..4], &[0, 1, 0, 0], "a TrueType outline font");
        }
    }

    #[test]
    fn the_license_texts_ship_beside_the_faces() {
        for l in [
            include_str!("../fonts/InstrumentSans-OFL.txt"),
            include_str!("../fonts/JetBrainsMono-OFL.txt"),
        ] {
            assert!(l.contains("SIL Open Font License, Version 1.1"));
        }
    }
}
