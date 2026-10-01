//! Probes for the brief 0005 budgets: UI-thread time per frame while
//! streaming, chunk send to present latency, and ready time.
//!
//! Frame work is measured as in brief 0001: from the panel's `render` (start
//! of the frame's layout for this view) to a `cx.defer` callback, which GPUI
//! runs after the frame is drawn and presented. Event handling on the UI
//! thread (applying a batch of updates to the transcript) is measured
//! separately; both run on the UI thread.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

#[derive(Debug, Default)]
pub struct Probes {
    pub recording: bool,
    /// Render start to end of present, per frame that rendered the panel.
    pub frame_work_ms: Vec<f64>,
    /// UI-thread time applying one batch of agent events.
    pub apply_ms: Vec<f64>,
    pub batch_sizes: Vec<usize>,
    /// Send stamps of chunks applied but not yet presented.
    pub unpresented: Vec<u128>,
    /// Fake agent send to applied on the UI thread.
    pub chunk_to_apply_ms: Vec<f64>,
    /// Fake agent send to end of the present that first showed the chunk.
    pub chunk_to_present_ms: Vec<f64>,
    pub frames: usize,
}

pub fn wall_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

/// Percentile by nearest rank.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn r3(x: f64) -> f64 {
    (x * 1000.).round() / 1000.
}

pub fn summarize(samples: &[f64]) -> Value {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let mean = s.iter().sum::<f64>() / s.len().max(1) as f64;
    json!({
        "n": s.len(),
        "p50_ms": r3(percentile(&s, 50.)),
        "p95_ms": r3(percentile(&s, 95.)),
        "p99_ms": r3(percentile(&s, 99.)),
        "max_ms": r3(s.last().copied().unwrap_or(f64::NAN)),
        "mean_ms": r3(mean),
    })
}

/// Resident set size in MiB, Linux only.
pub fn rss_mib() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib: f64 = s
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(r3(kib / 1024.))
}

impl Probes {
    pub fn report(&self) -> Value {
        json!({
            "frames_rendered": self.frames,
            "frame_work": summarize(&self.frame_work_ms),
            "ui_apply_per_batch": summarize(&self.apply_ms),
            "batch_size_mean": self.batch_sizes.iter().sum::<usize>() as f64 / self.batch_sizes.len().max(1) as f64,
            "chunk_to_apply": summarize(&self.chunk_to_apply_ms),
            "chunk_to_present": summarize(&self.chunk_to_present_ms),
            "chunks_presented": self.chunk_to_present_ms.len(),
            "rss_mib": rss_mib(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let s: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&s, 50.), 50.);
        assert_eq!(percentile(&s, 99.), 99.);
        assert_eq!(summarize(&s)["p95_ms"], 95.);
    }
}
