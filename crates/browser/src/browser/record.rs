//! `record` (brief 0032): a tab as an animated GIF, from the frames the engine renders into memory
//! ([`FrameSource`]), taken at `fps` on a thread of its own until `stop` or `max_seconds`.
//!
//! Each tick reads the newest frame without consuming it (the Web Browser window keeps drawing), converts BGRA to
//! RGBA scaled to at most [`MAX_WIDTH`] pixels wide, and encodes it with `image`'s GIF encoder (already in the build
//! through GPUI; no new dependency) at its fastest quantization. A tick where the page did not change (the same frame
//! sequence) lengthens the previous frame instead of adding one, so a still page makes a small file. The number of
//! ticks is `fps * max_seconds` whatever the timing, so a slow encoder makes the recording late, not short.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, RgbaImage};

use crate::embedded::{Frame, FrameSource};

/// Frames are scaled down to this width.
pub const MAX_WIDTH: u32 = 960;
/// The thumbnail of the first frame.
pub const THUMB_WIDTH: u32 = 320;

/// A recording in progress.
pub struct Recording {
    pub tab: String,
    pub path: PathBuf,
    pub fps: u32,
    pub max_seconds: u32,
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<Result<Recorded, String>>,
}

/// What a finished recording made.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    pub frames: u64,
    pub duration_ms: f64,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// `request`, `max_seconds` or `tab_closed`.
    pub stopped_by: &'static str,
    /// The first frame, PNG.
    pub thumbnail: Option<Vec<u8>>,
}

impl Recording {
    /// Start recording `source` into `path` (its folder is created).
    pub fn start(
        tab: &str,
        source: Arc<dyn FrameSource>,
        path: PathBuf,
        fps: u32,
        max_seconds: u32,
    ) -> Result<Recording, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (s, p) = (stop.clone(), path.clone());
        let fps = fps.max(1);
        let thread = std::thread::Builder::new()
            .name("browser-record".into())
            .spawn(move || record(source.as_ref(), &p, fps, max_seconds, &s))
            .map_err(|e| e.to_string())?;
        Ok(Recording {
            tab: tab.to_owned(),
            path,
            fps,
            max_seconds,
            stop,
            thread,
        })
    }

    /// The recording ended by itself (`max_seconds`, or the tab closed).
    pub fn is_done(&self) -> bool {
        self.thread.is_finished()
    }

    /// Stop and wait for the GIF.
    pub fn finish(self) -> Result<Recorded, String> {
        self.stop.store(true, Ordering::Release);
        self.thread
            .join()
            .map_err(|_| "the recording thread panicked".to_owned())?
    }
}

/// BGRA (premultiplied, opaque pages) to RGBA, scaled to at most `max_width` wide.
fn to_rgba(f: &Frame<'_>, max_width: u32) -> Option<RgbaImage> {
    let (w, h) = (f.width, f.height);
    let mut raw = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let o = y * f.stride as usize;
        raw.extend_from_slice(f.pixels.get(o..o + w as usize * 4)?);
    }
    for px in raw.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let img = RgbaImage::from_raw(w, h, raw)?;
    if w <= max_width {
        return Some(img);
    }
    let nh = ((u64::from(h) * u64::from(max_width)) / u64::from(w)).max(1) as u32;
    Some(image::imageops::resize(
        &img,
        max_width,
        nh,
        image::imageops::FilterType::Triangle,
    ))
}

fn png(img: &RgbaImage) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(out)
}

fn record(
    source: &dyn FrameSource,
    path: &Path,
    fps: u32,
    max_seconds: u32,
    stop: &AtomicBool,
) -> Result<Recorded, String> {
    let tmp = path.with_extension("gif.part");
    let file =
        std::fs::File::create(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    let mut out = std::io::BufWriter::new(file);
    let period = Duration::from_secs_f64(1. / f64::from(fps));
    let ticks = u64::from(fps) * u64::from(max_seconds);
    let started = Instant::now();
    let mut stopped_by = "max_seconds";
    let mut last_sequence = 0;
    // One frame is held back: a tick without a change lengthens it.
    let mut held: Option<(RgbaImage, u64)> = None;
    let mut size = (0, 0);
    let mut thumbnail = None;
    let mut frames = 0u64;
    let mut total_ms = 0u64;
    let period_ms = 1000 / u64::from(fps);
    {
        let mut encoder = GifEncoder::new_with_speed(&mut out, 30);
        encoder
            .set_repeat(Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        let mut emit =
            |img: RgbaImage, ms: u64, encoder: &mut GifEncoder<_>| -> Result<(), String> {
                frames += 1;
                total_ms += ms;
                encoder
                    .encode_frame(image::Frame::from_parts(
                        img,
                        0,
                        0,
                        Delay::from_numer_denom_ms(ms as u32, 1),
                    ))
                    .map_err(|e| e.to_string())
            };
        for tick in 0..ticks {
            let at = started + period * tick as u32;
            while Instant::now() < at {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep((at - Instant::now()).min(Duration::from_millis(10)));
            }
            if stop.load(Ordering::Acquire) {
                stopped_by = "request";
                break;
            }
            if source.is_closed() {
                stopped_by = "tab_closed";
                break;
            }
            let seq = source.sequence();
            let mut fresh: Option<RgbaImage> = None;
            if seq != last_sequence || held.is_none() {
                source.read(false, &mut |f| {
                    fresh = to_rgba(f, MAX_WIDTH);
                    last_sequence = f.sequence.max(seq);
                });
            }
            match (fresh, held.take()) {
                (Some(img), prev) => {
                    if thumbnail.is_none() {
                        let thumb = if img.width() > THUMB_WIDTH {
                            let th = (img.height() * THUMB_WIDTH / img.width()).max(1);
                            image::imageops::resize(
                                &img,
                                THUMB_WIDTH,
                                th,
                                image::imageops::FilterType::Triangle,
                            )
                        } else {
                            img.clone()
                        };
                        thumbnail = png(&thumb);
                        size = (img.width(), img.height());
                    }
                    if let Some((p, ms)) = prev {
                        emit(p, ms, &mut encoder)?;
                    }
                    held = Some((img, period_ms));
                }
                (None, Some((p, ms))) => held = Some((p, ms + period_ms)),
                (None, None) => {}
            }
        }
        if let Some((p, ms)) = held.take() {
            emit(p, ms, &mut encoder)?;
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(Recorded {
        frames,
        duration_ms: total_ms as f64,
        width: size.0,
        height: size.1,
        bytes,
        stopped_by,
        thumbnail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU64;

    /// A source whose page changes on every look (or never), 8 by 4 pixels of one gray.
    struct Fake {
        sequence: AtomicU64,
        moving: bool,
        reads: Mutex<u64>,
    }

    impl FrameSource for Fake {
        fn sequence(&self) -> u64 {
            if self.moving {
                self.sequence.fetch_add(1, Ordering::SeqCst) + 1
            } else {
                1
            }
        }

        fn read(&self, consume: bool, f: &mut dyn FnMut(&Frame<'_>)) -> bool {
            assert!(!consume, "a recording never takes the window's frames");
            *self.reads.lock().unwrap() += 1;
            let seq = self.sequence.load(Ordering::SeqCst);
            let pixels = vec![(seq * 40 % 255) as u8; 8 * 4 * 4];
            f(&Frame {
                sequence: seq,
                width: 8,
                height: 4,
                stride: 32,
                pixels: &pixels,
                dirty: &[],
                paint_ns: 0,
                copy_ns: 0,
            });
            true
        }

        fn set_listener(&self, _f: Option<Box<dyn Fn() + Send + Sync>>) {}
    }

    fn gif_frames(path: &Path) -> usize {
        use image::AnimationDecoder;
        let d = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(
            std::fs::File::open(path).unwrap(),
        ))
        .unwrap();
        d.into_frames().count()
    }

    #[test]
    fn a_moving_page_gives_one_frame_per_tick_and_a_still_one_a_single_long_frame() {
        let dir = tempfile::tempdir().unwrap();
        let moving = Arc::new(Fake {
            sequence: AtomicU64::new(0),
            moving: true,
            reads: Mutex::new(0),
        });
        let path = dir.path().join("rec").join("moving.gif");
        let r = Recording::start("t1", moving, path.clone(), 10, 1).unwrap();
        while !r.is_done() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let done = r.finish().unwrap();
        assert_eq!(done.frames, 10, "{done:?}");
        assert_eq!(done.duration_ms, 1000.);
        assert_eq!((done.width, done.height), (8, 4));
        assert_eq!(done.stopped_by, "max_seconds");
        assert_eq!(gif_frames(&path), 10);
        assert!(
            done.thumbnail
                .as_ref()
                .is_some_and(|t| t.starts_with(b"\x89PNG"))
        );
        assert_eq!(done.bytes, std::fs::metadata(&path).unwrap().len());

        let still = Arc::new(Fake {
            sequence: AtomicU64::new(1),
            moving: false,
            reads: Mutex::new(0),
        });
        let path = dir.path().join("still.gif");
        let r = Recording::start("t1", still.clone(), path.clone(), 20, 1).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let done = r.finish().unwrap();
        assert_eq!(done.frames, 1, "one frame, lengthened");
        assert_eq!(done.stopped_by, "request");
        assert!(done.duration_ms >= 50., "{done:?}");
        assert_eq!(
            *still.reads.lock().unwrap(),
            1,
            "an unchanged page is read once"
        );
        assert_eq!(gif_frames(&path), 1);
    }

    #[test]
    fn wide_frames_are_scaled_to_the_maximum_width() {
        let pixels = vec![200u8; 2000 * 10 * 4];
        let f = Frame {
            sequence: 1,
            width: 2000,
            height: 10,
            stride: 8000,
            pixels: &pixels,
            dirty: &[],
            paint_ns: 0,
            copy_ns: 0,
        };
        let img = to_rgba(&f, MAX_WIDTH).unwrap();
        assert_eq!((img.width(), img.height()), (MAX_WIDTH, 4));
    }
}
