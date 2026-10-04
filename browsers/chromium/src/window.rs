//! What the Web Browser window's methods need that is not CEF itself (brief 0032), kept here so it is tested without
//! CEF: the `<select>` popup drawn over the view, the names the protocol uses for CEF's cursor types, permissions and
//! window dispositions, where a download goes, and base64 for favicons; and the remote debugging port's choice and
//! readiness probe (brief 0038).

use std::path::{Path, PathBuf};

/// Copy `src` (`sw` by `sh` BGRA pixels) into `dst` (`dw` by `dh`) at (`x`, `y`), clipped to `dst`.
#[allow(clippy::too_many_arguments)]
pub fn overlay(dst: &mut [u8], dw: u32, dh: u32, src: &[u8], sw: u32, sh: u32, x: i32, y: i32) {
    let (dw, dh, sw, sh) = (dw as i64, dh as i64, sw as i64, sh as i64);
    let (x, y) = (x as i64, y as i64);
    let x0 = x.max(0);
    let x1 = (x + sw).min(dw);
    if x0 >= x1 {
        return;
    }
    for row in y.max(0)..(y + sh).min(dh) {
        let sr = row - y;
        let s = ((sr * sw + (x0 - x)) * 4) as usize;
        let d = ((row * dw + x0) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        if s + n <= src.len() && d + n <= dst.len() {
            dst[d..d + n].copy_from_slice(&src[s..s + n]);
        }
    }
}

/// CSS's cursor keyword for CEF's `cef_cursor_type_t`, by its value (`tab/cursor`).
pub fn cursor_name(cef_type: u32) -> &'static str {
    const NAMES: [&str; 50] = [
        "default",       // CT_POINTER
        "crosshair",     // CT_CROSS
        "pointer",       // CT_HAND
        "text",          // CT_IBEAM
        "wait",          // CT_WAIT
        "help",          // CT_HELP
        "e-resize",      // CT_EASTRESIZE
        "n-resize",      // CT_NORTHRESIZE
        "ne-resize",     // CT_NORTHEASTRESIZE
        "nw-resize",     // CT_NORTHWESTRESIZE
        "s-resize",      // CT_SOUTHRESIZE
        "se-resize",     // CT_SOUTHEASTRESIZE
        "sw-resize",     // CT_SOUTHWESTRESIZE
        "w-resize",      // CT_WESTRESIZE
        "ns-resize",     // CT_NORTHSOUTHRESIZE
        "ew-resize",     // CT_EASTWESTRESIZE
        "nesw-resize",   // CT_NORTHEASTSOUTHWESTRESIZE
        "nwse-resize",   // CT_NORTHWESTSOUTHEASTRESIZE
        "col-resize",    // CT_COLUMNRESIZE
        "row-resize",    // CT_ROWRESIZE
        "all-scroll",    // CT_MIDDLEPANNING
        "all-scroll",    // CT_EASTPANNING
        "all-scroll",    // CT_NORTHPANNING
        "all-scroll",    // CT_NORTHEASTPANNING
        "all-scroll",    // CT_NORTHWESTPANNING
        "all-scroll",    // CT_SOUTHPANNING
        "all-scroll",    // CT_SOUTHEASTPANNING
        "all-scroll",    // CT_SOUTHWESTPANNING
        "all-scroll",    // CT_WESTPANNING
        "move",          // CT_MOVE
        "vertical-text", // CT_VERTICALTEXT
        "cell",          // CT_CELL
        "context-menu",  // CT_CONTEXTMENU
        "alias",         // CT_ALIAS
        "progress",      // CT_PROGRESS
        "no-drop",       // CT_NODROP
        "copy",          // CT_COPY
        "none",          // CT_NONE
        "not-allowed",   // CT_NOTALLOWED
        "zoom-in",       // CT_ZOOMIN
        "zoom-out",      // CT_ZOOMOUT
        "grab",          // CT_GRAB
        "grabbing",      // CT_GRABBING
        "all-scroll",    // CT_MIDDLE_PANNING_VERTICAL
        "all-scroll",    // CT_MIDDLE_PANNING_HORIZONTAL
        "custom",        // CT_CUSTOM
        "no-drop",       // CT_DND_NONE
        "move",          // CT_DND_MOVE
        "copy",          // CT_DND_COPY
        "alias",         // CT_DND_LINK
    ];
    NAMES.get(cef_type as usize).copied().unwrap_or("default")
}

/// The protocol's permission names for CEF's `cef_permission_request_types_t` bits (`tab/permission`).
pub fn permission_names(bits: u32) -> Vec<&'static str> {
    const MAP: [(u32, &str); 14] = [
        (1 << 8, "geolocation"),
        (1 << 15, "notifications"),
        (1 << 2, "camera"),
        (1 << 1, "camera"),
        (1 << 12, "microphone"),
        (1 << 4, "clipboard"),
        (1 << 13, "midi"),
        (1 << 5, "storage"),
        (1 << 6, "storage"),
        (1 << 20, "storage"),
        (1 << 28, "sensors"),
        (1 << 23, "windowManagement"),
        (1 << 7, "localFonts"),
        (1 << 3, "screen"),
    ];
    let mut out: Vec<&str> = Vec::new();
    let mut known = 0;
    for (bit, name) in MAP {
        known |= bit;
        if bits & bit != 0 && !out.contains(&name) {
            out.push(name);
        }
    }
    if bits & !known != 0 || out.is_empty() {
        out.push("other");
    }
    out
}

/// The protocol's names for CEF's `cef_media_access_permission_types_t` bits (camera, microphone, screen capture).
pub fn media_permission_names(bits: u32) -> Vec<&'static str> {
    let mut out = Vec::new();
    if bits & 2 != 0 {
        out.push("camera");
    }
    if bits & 1 != 0 {
        out.push("microphone");
    }
    if bits & (4 | 8) != 0 {
        out.push("screen");
    }
    if out.is_empty() {
        out.push("other");
    }
    out
}

/// `tab/popup`'s `disposition` for CEF's `cef_window_open_disposition_t`.
pub fn disposition_name(raw: u32) -> &'static str {
    match raw {
        3 => "newForegroundTab",
        4 => "newBackgroundTab",
        5 => "newPopup",
        6 => "newWindow",
        _ => "other",
    }
}

/// A file name a page suggested, made safe: no folders, no leading dots, nothing empty.
pub fn safe_file_name(suggested: &str, url: &str) -> String {
    let base = if suggested.trim().is_empty() {
        url.split(['?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("download")
    } else {
        suggested
    };
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' | ':' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    if cleaned.is_empty() || cleaned.contains("://") {
        "download".into()
    } else {
        cleaned
    }
}

/// `dir/name`, or `dir/stem (1).ext`, `(2)`, ... when that exists.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    (1..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .expect("an unused name")
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= c.len() {
                A[(n >> (18 - 6 * i)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// The default download folder for a profile: `downloads` beside it (`<workspace>/.eludite/browser/downloads`).
pub fn default_download_dir(profile: &Path) -> PathBuf {
    profile
        .parent()
        .map(|p| p.join("downloads"))
        .unwrap_or_else(|| profile.join("downloads"))
}

/// Downloads larger than this are refused unless `initialize` says otherwise: 100 MB.
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

// ---- the remote debugging port (brief 0038) ----

/// A free port on 127.0.0.1 for CEF's remote debugging (CEF takes 1024 to 65534): the system's choice for a socket
/// bound and closed at once. Another process could take it before CEF binds it; `engine/ready` is only sent once the
/// port answers as DevTools does.
pub fn free_debug_port() -> Option<u16> {
    (0..20).find_map(|_| {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = l.local_addr().ok()?.port();
        (1024..65535).contains(&port).then_some(port)
    })
}

/// Whether DevTools answers on `127.0.0.1:port`: `GET /json/version` with a status line `HTTP/1.1 200`.
pub fn devtools_answers(port: u16, timeout: std::time::Duration) -> bool {
    use std::io::{Read as _, Write as _};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(timeout));
    if s.write_all(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut head = [0u8; 12];
    s.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.1 200")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_popup_is_drawn_over_the_view_and_clipped() {
        let mut dst = vec![0u8; 4 * 4 * 4];
        let src = vec![9u8; 2 * 2 * 4];
        overlay(&mut dst, 4, 4, &src, 2, 2, 1, 1);
        let px = |x: usize, y: usize| dst[(y * 4 + x) * 4];
        assert_eq!((px(0, 0), px(1, 1), px(2, 2), px(3, 3)), (0, 9, 9, 0));
        // Partly outside: clipped, no panic.
        let mut dst = vec![0u8; 4 * 4 * 4];
        overlay(&mut dst, 4, 4, &src, 2, 2, 3, -1);
        assert_eq!(dst[(3) * 4], 9, "row 0, column 3");
        assert_eq!(dst[(4 + 3) * 4], 0, "row 1 is below the popup");
        overlay(&mut dst, 4, 4, &src, 2, 2, 10, 10);
    }

    #[test]
    fn cursor_permission_and_disposition_names() {
        assert_eq!(cursor_name(0), "default");
        assert_eq!(cursor_name(2), "pointer");
        assert_eq!(cursor_name(3), "text");
        assert_eq!(cursor_name(45), "custom");
        assert_eq!(cursor_name(999), "default");
        assert_eq!(permission_names(1 << 8), ["geolocation"]);
        assert_eq!(
            permission_names((1 << 2) | (1 << 1) | (1 << 12)),
            ["camera", "microphone"]
        );
        assert_eq!(permission_names(1 << 30), ["other"]);
        assert_eq!(media_permission_names(3), ["camera", "microphone"]);
        assert_eq!(disposition_name(3), "newForegroundTab");
    }

    #[test]
    fn download_names_are_safe_and_unique() {
        assert_eq!(safe_file_name("../../etc/passwd", "x"), "_.._etc_passwd");
        assert_eq!(
            safe_file_name("", "http://h/a/report.pdf?x=1"),
            "report.pdf"
        );
        assert_eq!(safe_file_name("", "http://h/"), "h");
        assert_eq!(safe_file_name(".hidden", "x"), "hidden");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a.txt"), dir.path().join("a.txt"));
        std::fs::write(dir.path().join("a.txt"), b"").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (1).txt")
        );
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(
            default_download_dir(Path::new("/w/.eludite/browser/profile")),
            Path::new("/w/.eludite/browser/downloads")
        );
    }

    /// Brief 0038: a free port to give CEF, and the probe that says DevTools answers there.
    #[test]
    fn a_free_debug_port_and_the_devtools_probe() {
        let port = free_debug_port().unwrap();
        assert!(port >= 1024);
        // Nothing listens there yet.
        assert!(!devtools_answers(
            port,
            std::time::Duration::from_millis(200)
        ));
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            for (i, s) in l.incoming().enumerate() {
                let mut s = s.unwrap();
                let mut req = [0u8; 256];
                let n = s.read(&mut req).unwrap();
                assert!(
                    std::str::from_utf8(&req[..n])
                        .unwrap()
                        .starts_with("GET /json/version")
                );
                let status: &[u8] = if i == 0 {
                    b"HTTP/1.1 404 Not Found\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\n\r\n{}"
                };
                s.write_all(status).unwrap();
            }
        });
        assert!(!devtools_answers(port, std::time::Duration::from_secs(1)));
        assert!(devtools_answers(port, std::time::Duration::from_secs(1)));
    }
}
