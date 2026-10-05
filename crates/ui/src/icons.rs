//! Eludite's own icon set (brief 0061; PLAN.md 8, "our own iconography"): 16 by 16 SVG drawings in Visual Studio's
//! metaphors (a yellow folder, a green C# project, a purple solution, the NuGet cube), drawn for Eludite and never
//! copied from Visual Studio, VS Code or Zed. The files are `crates/ui/icons/*.svg`, embedded in the binary and served
//! by [`Assets`], the shell's [`gpui::AssetSource`] (`Application::with_assets`).
//!
//! # Rules for a new icon
//!
//! - `viewBox="0 0 16 16"`, `width` and `height` 16, drawn on the pixel grid with 1 px strokes (`x.5` coordinates for
//!   a crisp line); under 2 KB.
//! - **Monochrome** (most of the set): every shape in `currentColor`, with depth from `fill-opacity` (a 0.3 fill inside
//!   a full-strength outline) or a mask that knocks a letter out. GPUI's `svg()` draws a file as an alpha mask in one
//!   color, so [`icon`] tints it with the theme's token for the icon ([`Icon::tint`]); each theme has its own values
//!   (`icon_folder`, `icon_csharp_file` and the rest in [`Theme`]).
//! - **Colored**: up to three fixed colors in the file, drawn as an image (`img()`), for an icon whose meaning needs a
//!   second color (today only `project-unavailable`: a muted project with the red error badge). Every color except
//!   white details inside a shape must read at 3:1 on all three themes' panels; an icon that cannot gets a `-light`
//!   variant for VS Light and VS Blue (none needs one today).
//! - Add the variant to [`Icon`] with what it means; the tests check that every icon has a file that parses, that the
//!   monochrome ones use `currentColor`, and the colored ones' contrast.
//!
//! # The set
//!
//! Tree roots: `solution` (a frame of four projects, purple), `workspace` (the opened folder: a folder with a tree),
//! `cargo-workspace` (two crates) and `cargo-package` (a crate), in the Rust orange. Folders: `folder`, `folder-open`
//! and the solution folders `solution-folder`, `solution-folder-open` (a folder with a mark), yellow. .NET projects:
//! `project-csharp` (a green square with C#), `project-csharp-test` (with a flask: references a test framework),
//! `project-web` (with a globe: a web project), `project-unavailable` (dashed and muted with the red error badge: it did
//! not load). The Dependencies node: `dependencies` (the blue stack), `frameworks` and `framework` (a reference
//! book), `packages` (the NuGet cube with its dot), `package` and `package-transitive` (a cube, dashed for one a package
//! brings in), `project-reference` (two linked projects). Cargo targets: `cargo-targets` (a target), `cargo-target-bin`
//! (a console window), `cargo-target-lib` (books on a shelf), `cargo-target-test` (a flask), `cargo-target-example`
//! (a light bulb), `cargo-target-bench` (a stopwatch). Files by type: `file-cs` (C#), `file-rs` (a page with a gear),
//! `file-json` (braces), `file-md` (M and a down arrow), `file-xml` (`</>`: MSBuild files, `.xml`, `.config`, `.resx`),
//! `file-sln` (a page of projects), `file-toml` (a page of keys), `file-ts` (TS), `file-js` (JS), `file-html` (a page
//! with `<>`), `file-css` (a page with `#`), `file-razor` (`@`), `file-cshtml` (a page with `@`), `file-aspx` (a page
//! with a globe), `file-image` (a picture), `file-text` (a page of lines), `file-shell` (a console with a prompt),
//! `file-yaml` (a page of indented keys), `file-lock` (a page with a pin: a lockfile), `file` (a page). The search
//! box: `search` (a magnifier) and `clear` (a cross).

use std::borrow::Cow;

use gpui::{AnyElement, AssetSource, Hsla, IntoElement, SharedString, Styled, img, px, svg};

use crate::Theme;

/// The prefix of every icon's asset path (`icons/folder.svg`).
pub const ICON_DIR: &str = "icons/";

macro_rules! icons {
    ($($(#[doc = $doc:literal])* $variant:ident => $file:literal,)*) => {
        /// An icon of the set; see the module docs for what each one means.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($(#[doc = $doc])* $variant,)*
        }

        impl Icon {
            /// Every icon.
            pub const ALL: &'static [Icon] = &[$(Icon::$variant,)*];

            /// The file's name without `.svg` (`folder-open`).
            pub fn name(self) -> &'static str {
                match self {
                    $(Icon::$variant => $file,)*
                }
            }

            /// The SVG file.
            pub fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$variant => include_bytes!(concat!("../icons/", $file, ".svg")),)*
                }
            }
        }
    };
}

icons! {
    /// The .NET solution.
    Solution => "solution",
    /// The opened folder at the root of the Workspace window.
    Workspace => "workspace",
    /// The Cargo workspace.
    CargoWorkspace => "cargo-workspace",
    /// A member package of the Cargo workspace.
    CargoPackage => "cargo-package",
    /// A solution folder (`.slnx` folders), collapsed.
    SolutionFolder => "solution-folder",
    /// A solution folder, expanded.
    SolutionFolderOpen => "solution-folder-open",
    /// A C# project.
    ProjectCSharp => "project-csharp",
    /// A C# project that references a test framework.
    ProjectCSharpTest => "project-csharp-test",
    /// A web project.
    ProjectWeb => "project-web",
    /// A project that did not load (colored: muted with the red error badge).
    ProjectUnavailable => "project-unavailable",
    /// A project's Dependencies node.
    Dependencies => "dependencies",
    /// The Frameworks folder of Dependencies.
    Frameworks => "frameworks",
    /// The Packages folder of Dependencies.
    Packages => "packages",
    /// A package the project references.
    Package => "package",
    /// A package another package brings in.
    PackageTransitive => "package-transitive",
    /// The Projects folder of Dependencies.
    ProjectReference => "project-reference",
    /// A shared framework (`Microsoft.NETCore.App`).
    Framework => "framework",
    /// A folder, collapsed.
    Folder => "folder",
    /// A folder, expanded.
    FolderOpen => "folder-open",
    /// A package's Targets folder.
    CargoTargets => "cargo-targets",
    /// A `bin` target (and a build script).
    CargoTargetBin => "cargo-target-bin",
    /// A `lib` or `proc-macro` target.
    CargoTargetLib => "cargo-target-lib",
    /// A `test` target.
    CargoTargetTest => "cargo-target-test",
    /// An `example` target.
    CargoTargetExample => "cargo-target-example",
    /// A `bench` target.
    CargoTargetBench => "cargo-target-bench",
    /// `.cs`.
    FileCs => "file-cs",
    /// `.rs`.
    FileRs => "file-rs",
    /// `.json`.
    FileJson => "file-json",
    /// `.md`.
    FileMd => "file-md",
    /// MSBuild and XML files: `.csproj`, `.props`, `.targets`, `.xml`, `.config`, `.resx`.
    FileXml => "file-xml",
    /// `.sln`, `.slnx`.
    FileSln => "file-sln",
    /// `.toml`.
    FileToml => "file-toml",
    /// `.ts`, `.tsx`.
    FileTs => "file-ts",
    /// `.js`.
    FileJs => "file-js",
    /// `.html`.
    FileHtml => "file-html",
    /// `.css`.
    FileCss => "file-css",
    /// `.razor`.
    FileRazor => "file-razor",
    /// `.cshtml`.
    FileCshtml => "file-cshtml",
    /// Web Forms: `.aspx`, `.ascx`, `.master`.
    FileAspx => "file-aspx",
    /// `.png`, `.jpg`, `.gif`, `.svg`, `.ico`.
    FileImage => "file-image",
    /// `.txt`, `.log`.
    FileText => "file-text",
    /// `.sh`, `.ps1`.
    FileShell => "file-shell",
    /// `.yml`, `.yaml`.
    FileYaml => "file-yaml",
    /// Lockfiles (`Cargo.lock`, `packages.lock.json`).
    FileLock => "file-lock",
    /// Any other file.
    File => "file",
    /// The search box's magnifier.
    Search => "search",
    /// The search box's clear button.
    Clear => "clear",
}

impl Icon {
    /// The asset path GPUI loads it by (`icons/folder.svg`).
    pub fn path(self) -> SharedString {
        SharedString::from(format!("{ICON_DIR}{}.svg", self.name()))
    }

    /// The icon named `name` (`folder-open`).
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|i| i.name() == name)
    }

    /// Whether the file carries its own colors (drawn as an image) rather than `currentColor` (tinted).
    pub fn colored(self) -> bool {
        matches!(self, Icon::ProjectUnavailable)
    }

    /// The theme's color for a monochrome icon (a colored one ignores it).
    pub fn tint(self, t: &Theme) -> gpui::Rgba {
        use Icon::*;
        match self {
            Solution | FileSln => t.icon_solution,
            Workspace | Folder | FolderOpen | SolutionFolder | SolutionFolderOpen => t.icon_folder,
            CargoWorkspace | CargoPackage | CargoTargetBin | CargoTargetLib | CargoTargetTest
            | CargoTargetExample | CargoTargetBench | FileRs => t.icon_rust,
            ProjectCSharp | ProjectCSharpTest | ProjectReference => t.icon_csharp_project,
            ProjectWeb | FileTs | FileCss | FileAspx => t.icon_web,
            Dependencies | Frameworks | Framework => t.icon_dependencies,
            Packages | Package | PackageTransitive => t.icon_package,
            FileCs | FileRazor | FileCshtml => t.icon_csharp_file,
            FileJson | FileJs | FileYaml => t.icon_json,
            FileMd => t.icon_markdown,
            FileXml | FileHtml => t.icon_xml,
            ProjectUnavailable | CargoTargets | FileToml | FileImage | FileText | FileShell
            | FileLock | File | Search | Clear => t.icon_muted,
        }
    }
}

/// The icons as GPUI assets (`icons/<name>.svg`), embedded in the binary. Register with
/// `Application::new().with_assets(Assets)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(path
            .strip_prefix(ICON_DIR)
            .and_then(|f| f.strip_suffix(".svg"))
            .and_then(Icon::by_name)
            .map(|i| Cow::Borrowed(i.bytes())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Icon::ALL
            .iter()
            .map(|i| i.path())
            .filter(|p| p.starts_with(path))
            .collect())
    }
}

/// A 16 by 16 icon: a monochrome one tinted `color` (GPUI's `svg()` with `text_color`), a colored one as drawn.
pub fn icon(icon: Icon, color: impl Into<Hsla>) -> AnyElement {
    if icon.colored() {
        img(icon.path())
            .flex_none()
            .w(px(16.))
            .h(px(16.))
            .into_any_element()
    } else {
        svg()
            .path(icon.path())
            .flex_none()
            .w(px(16.))
            .h(px(16.))
            .text_color(color.into())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn text(i: Icon) -> &'static str {
        std::str::from_utf8(i.bytes()).expect("icons are UTF-8")
    }

    /// Every `#RRGGBB` (or `#RGB`) color a file names.
    fn colors(svg: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (i, _) in svg.match_indices('#') {
            let hex: String = svg[i + 1..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .collect();
            let hex = match hex.len() {
                3 => hex.chars().flat_map(|c| [c, c]).collect(),
                6 => hex,
                _ => continue,
            };
            out.push(hex.to_ascii_uppercase());
        }
        out
    }

    #[test]
    fn every_icon_has_a_16_by_16_file_that_parses_and_the_asset_source_serves_it() {
        let renderer = gpui::SvgRenderer::new(Arc::new(Assets));
        let mut names = std::collections::HashSet::new();
        for &i in Icon::ALL {
            assert!(names.insert(i.name()), "{} twice", i.name());
            let svg = text(i);
            assert!(svg.len() < 2048, "{}: {} bytes", i.name(), svg.len());
            assert!(
                svg.contains(r#"viewBox="0 0 16 16""#)
                    && svg.contains(r#"width="16""#)
                    && svg.contains(r#"height="16""#),
                "{}",
                i.name()
            );
            let served = Assets.load(&i.path()).unwrap().expect("served");
            assert_eq!(&*served, i.bytes());
            renderer
                .parse_svg(&served)
                .unwrap_or_else(|e| panic!("{}: {e}", i.name()));
            let image = renderer.render_single_frame(&served, 1.0).unwrap();
            let size = image.size(0);
            let scale = gpui::SMOOTH_SVG_SCALE_FACTOR as i32;
            assert_eq!((size.width.0, size.height.0), (16 * scale, 16 * scale));
            let bgra = image.as_bytes(0).unwrap();
            assert!(
                bgra.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "{} draws nothing",
                i.name()
            );
            assert_eq!(Icon::by_name(i.name()), Some(i));
        }
        assert_eq!(Assets.load("icons/nope.svg").unwrap(), None);
        assert_eq!(Assets.load("folder.svg").unwrap(), None);
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
        // The icons the brief names.
        assert_eq!(Icon::ALL.len(), 47);
        assert_eq!(Icon::FolderOpen.path(), "icons/folder-open.svg");
    }

    #[test]
    fn monochrome_icons_use_current_color_and_colored_ones_read_on_every_panel() {
        for &i in Icon::ALL {
            let svg = text(i);
            if i.colored() {
                assert!(!svg.contains("currentColor"), "{}", i.name());
                let mut distinct: Vec<String> =
                    colors(svg).into_iter().filter(|c| c != "FFFFFF").collect();
                distinct.sort();
                distinct.dedup();
                assert!(
                    !distinct.is_empty() && distinct.len() <= 3,
                    "{}: {distinct:?}",
                    i.name()
                );
                for t in Theme::all() {
                    for c in &distinct {
                        let rgb = gpui::rgb(u32::from_str_radix(c, 16).unwrap());
                        let ratio = crate::theme::contrast(rgb, t.panel);
                        assert!(ratio >= 3.0, "{} #{c} on {}: {ratio}", i.name(), t.name);
                    }
                }
            } else {
                assert!(svg.contains("currentColor"), "{}", i.name());
                // Only the mask's black and white besides it: no fixed color to clash with the theme.
                assert!(
                    colors(svg).iter().all(|c| c == "FFFFFF" || c == "000000"),
                    "{}: {:?}",
                    i.name(),
                    colors(svg)
                );
            }
        }
    }

    #[test]
    fn every_icon_has_a_tint_in_every_theme() {
        for t in Theme::all() {
            for &i in Icon::ALL {
                let c = i.tint(&t);
                assert!(
                    crate::theme::contrast(c, t.panel) >= 3.0,
                    "{} on {}",
                    i.name(),
                    t.name
                );
            }
        }
    }
}
