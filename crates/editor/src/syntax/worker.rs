//! The syntax thread: one OS thread that runs every highlight step in the
//! process.
//!
//! Why not GPUI's background pool: a parse tree for a large file is
//! hundreds of thousands of small allocations, and each incremental re-parse
//! frees some and allocates others. Spread over a pool's threads, glibc's
//! per-thread malloc arenas fragment quickly (measured in brief 0009: RSS
//! grew 150 MB over 300 keystrokes in a 100k-line file, versus 14 MB with one
//! arena). One thread keeps that churn in one arena. Steps are short and
//! bounded ([`super::highlighter::ROWS_PER_STEP`]), so editors take turns.

use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

type Job = Box<dyn FnOnce() + Send>;

/// Handle to the process-wide syntax thread.
#[derive(Debug)]
pub struct SyntaxThread {
    jobs: std::sync::Mutex<Sender<Job>>,
}

impl SyntaxThread {
    /// The thread, started on first use.
    pub fn global() -> &'static SyntaxThread {
        static THREAD: OnceLock<SyntaxThread> = OnceLock::new();
        THREAD.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            std::thread::Builder::new()
                .name("niello-syntax".into())
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        job();
                    }
                })
                .expect("spawn syntax thread");
            SyntaxThread {
                jobs: std::sync::Mutex::new(tx),
            }
        })
    }

    /// Run `f` on the syntax thread. The future resolves to its result, or
    /// `None` if the thread is gone.
    pub fn run<R, F>(&self, f: F) -> impl std::future::Future<Output = Option<R>> + use<R, F>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        let (tx, rx) = futures::channel::oneshot::channel();
        let job: Job = Box::new(move || {
            let _ = tx.send(f());
        });
        let sent = self
            .jobs
            .lock()
            .map(|jobs| jobs.send(job).is_ok())
            .unwrap_or(false);
        async move { if sent { rx.await.ok() } else { None } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_jobs_off_the_calling_thread() {
        let caller = std::thread::current().id();
        let fut = SyntaxThread::global().run(move || std::thread::current().id() != caller);
        assert_eq!(futures::executor::block_on(fut), Some(true));
    }
}
