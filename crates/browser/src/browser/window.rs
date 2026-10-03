//! The commands brief 0032 adds for the Web Browser window, and who drives: `record` (a GIF of the tab from the
//! engine's frames, [`super::record`]), `devtools` (DevTools as a tab of the window), `dialog` (answer the dialog a
//! page waits on), and the [`Interrupt`] the shell uses when the person takes over from an agent.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use eludite_commands::CommandError;
use eludite_commands::browser::{
    DevtoolsOutput, DialogOutput, DialogRow, RecordAction, RecordOutput,
};
use serde_json::json;

use super::record::Recording;
use super::{Browser, Tab, engine_err, failed, invalid};
use crate::engine::{DialogAnswer, PendingDialog};

/// The person's hand on the browser, shared between the shell and the [`Browser`] (brief 0032, proposal 0002
/// section 5): [`Interrupt::took_over`] ends an agent's `wait` at once with `interrupted_by: "user"` (the person
/// clicked or typed in the page, or used the window's toolbar); [`Interrupt::stop`] (the "Agent is driving" strip's
/// Stop) also fails whatever else the agent's call was waiting for.
#[derive(Debug, Default)]
pub struct Interrupt {
    waits: AtomicU64,
    stops: AtomicU64,
}

/// The [`Interrupt`] counters when a call began.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    waits: u64,
    stops: u64,
}

impl Interrupt {
    /// The person took over: waiting calls end.
    pub fn took_over(&self) {
        self.waits.fetch_add(1, Ordering::AcqRel);
    }

    /// Stop: waiting calls end and the rest fail.
    pub fn stop(&self) {
        self.stops.fetch_add(1, Ordering::AcqRel);
        self.waits.fetch_add(1, Ordering::AcqRel);
    }

    pub fn marks(&self) -> Marks {
        Marks {
            waits: self.waits.load(Ordering::Acquire),
            stops: self.stops.load(Ordering::Acquire),
        }
    }

    /// Since `m`: the person took over (or stopped).
    pub fn interrupted_since(&self, m: Marks) -> bool {
        self.waits.load(Ordering::Acquire) != m.waits
    }

    /// Since `m`: the person pressed Stop.
    pub fn stopped_since(&self, m: Marks) -> bool {
        self.stops.load(Ordering::Acquire) != m.stops
    }
}

/// The error of a call the person stopped.
pub fn stopped_error() -> CommandError {
    failed(
        "the person stopped this call (Stop on the Web Browser window's \"Agent is driving\" strip); read \
         eludite.browser.tabs before acting again",
    )
}

impl Browser {
    /// The dialog `tab`'s page waits on: the engine's own (the embedded engine), or CDP's.
    pub(super) fn dialog_of(&self, tab: &Tab) -> Option<PendingDialog> {
        self.engine
            .pending_dialog(&tab.target)
            .or_else(|| tab.state().dialog.clone())
    }

    /// The dialog as `input` reports it.
    pub(super) fn dialog_row(&self, tab: &Tab) -> Option<DialogRow> {
        self.dialog_of(tab).map(|d| DialogRow {
            kind: d.kind,
            message: d.message,
            default_text: d.default_text,
        })
    }

    pub(super) fn record(
        &mut self,
        id: Option<&str>,
        action: &RecordAction,
    ) -> Result<RecordOutput, CommandError> {
        match action {
            RecordAction::Start {
                path,
                fps,
                max_seconds,
            } => {
                if let Some(r) = &self.recording {
                    if !r.is_done() {
                        return Err(failed(format!(
                            "tab {} is being recorded into {}; stop it first (action: stop)",
                            r.tab,
                            r.path.display()
                        )));
                    }
                    // Ended by itself and never stopped: its GIF is written; start anew.
                    let _ = self.recording.take().map(Recording::finish);
                }
                let tab = self.resolve(id)?;
                let source = self.engine.frames(&tab.target).ok_or_else(|| {
                    failed(
                        "record takes the frames the embedded engine draws (the setting browser.engine: \
                         embedded, with CEF from tools/cef/fetch.sh); this browser is an external Chrome",
                    )
                })?;
                let path = match path {
                    Some(p) => {
                        let p = PathBuf::from(p);
                        if !p.is_absolute() {
                            return Err(invalid(format!(
                                "`path` must be absolute here, not `{}`",
                                p.display()
                            )));
                        }
                        p
                    }
                    None => std::env::temp_dir()
                        .join("eludite-recordings")
                        .join(default_name(&tab.id)),
                };
                let r =
                    Recording::start(&tab.id, source, path, *fps, *max_seconds).map_err(failed)?;
                let out = RecordOutput {
                    tab: tab.id.clone(),
                    recording: true,
                    path: r.path.display().to_string(),
                    fps: r.fps,
                    max_seconds: r.max_seconds,
                    ..RecordOutput::default()
                };
                (self.log)(&format!(
                    "Recording tab {} at {} fps into {}",
                    tab.id,
                    r.fps,
                    r.path.display()
                ));
                self.recording = Some(r);
                Ok(out)
            }
            RecordAction::Stop => {
                let r = self.recording.take().ok_or_else(|| {
                    failed("no recording is running; start one with action: start")
                })?;
                if let Some(id) = id
                    && id != r.tab
                {
                    let tab = r.tab.clone();
                    self.recording = Some(r);
                    return Err(failed(format!("the recording is of tab {tab}, not {id}")));
                }
                let (tab, path, fps, max_seconds) =
                    (r.tab.clone(), r.path.clone(), r.fps, r.max_seconds);
                let done = r.finish().map_err(failed)?;
                (self.log)(&format!(
                    "Recorded tab {tab}: {} frames, {:.1} s, {} bytes, {}",
                    done.frames,
                    done.duration_ms / 1000.,
                    done.bytes,
                    path.display()
                ));
                Ok(RecordOutput {
                    tab,
                    recording: false,
                    path: path.display().to_string(),
                    fps,
                    max_seconds,
                    frames: Some(done.frames),
                    duration_ms: Some(done.duration_ms),
                    width: Some(done.width),
                    height: Some(done.height),
                    bytes: Some(done.bytes),
                    stopped_by: Some(done.stopped_by.into()),
                    thumbnail: done.thumbnail.as_deref().map(super::base64_encode),
                })
            }
        }
    }

    pub(super) fn devtools(
        &mut self,
        id: Option<&str>,
        inspect: Option<(f64, f64)>,
    ) -> Result<DevtoolsOutput, CommandError> {
        let tab = self.resolve(id)?;
        let opened = self
            .engine
            .devtools(&tab.target, inspect)
            .map_err(engine_err)?;
        Ok(DevtoolsOutput {
            tab: tab.id.clone(),
            opened,
        })
    }

    pub(super) fn dialog(
        &mut self,
        id: Option<&str>,
        answer: DialogAnswer,
    ) -> Result<DialogOutput, CommandError> {
        let tab = self.resolve(id)?;
        let action = if answer.accept { "accept" } else { "dismiss" };
        let d = if self.engine.pending_dialog(&tab.target).is_some() {
            self.engine
                .answer_dialog(&tab.target, &answer)
                .map_err(engine_err)?
        } else {
            let d = tab.state().dialog.clone().ok_or_else(|| {
                failed(
                    "no dialog is open in the tab (it was answered, perhaps by the person, or the page went on)",
                )
            })?;
            let mut params = json!({"accept": answer.accept});
            if let Some(t) = &answer.text {
                params["promptText"] = json!(t);
            }
            self.send(&tab, "Page.handleJavaScriptDialog", params)?;
            tab.state().dialog = None;
            d
        };
        let text = (d.kind == "prompt" && answer.accept)
            .then(|| answer.text.clone().or(d.default_text.clone()))
            .flatten();
        (self.log)(&format!(
            "[{}] {} the {} \"{}\"",
            tab.id,
            if answer.accept {
                "Accepted"
            } else {
                "Dismissed"
            },
            d.kind,
            d.message
        ));
        Ok(DialogOutput {
            tab: tab.id.clone(),
            kind: d.kind,
            message: d.message,
            action: action.into(),
            text,
        })
    }
}

/// `<tab>-<seconds since 1970>.gif`.
pub fn default_name(tab: &str) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{tab}-{secs}.gif")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupts_count_from_a_call_start() {
        let i = Interrupt::default();
        let m = i.marks();
        assert!(!i.interrupted_since(m) && !i.stopped_since(m));
        i.took_over();
        assert!(i.interrupted_since(m) && !i.stopped_since(m));
        let m = i.marks();
        i.stop();
        assert!(i.interrupted_since(m) && i.stopped_since(m));
        assert!(default_name("t3").starts_with("t3-") && default_name("t3").ends_with(".gif"));
    }
}
