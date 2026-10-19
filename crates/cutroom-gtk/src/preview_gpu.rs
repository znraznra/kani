//! Puts the preview picture together on the graphics chip.
//!
//! The chip's drawing context lives on a thread of its own: making an OpenGL
//! context current on the window's thread would upset the toolkit that draws
//! the window. The preview sends the layers over; the live player carries on
//! meanwhile and picks the picture up when it is ready ([`PreviewGpu::submit`]
//! and [`PreviewGpu::poll`]), so the window never waits for the chip.

use crate::gpu_compose::{GpuCompositor, UNSUPPORTED};
use cutroom_compose::{Frame, Placement};
use std::cell::Cell;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Job {
    size: (u32, u32),
    layers: Vec<(Arc<Frame>, Placement)>,
}

/// Where a picture's time went on the chip's thread.
#[derive(Debug, Clone, Copy, Default)]
pub struct Timing {
    /// Uploading new pictures and drawing every layer.
    pub draw: Duration,
    /// Reading the finished picture back.
    pub read: Duration,
}

pub enum Reply {
    Picture(Vec<u8>, Timing),
    /// One layer the chip cannot place: the processor does this picture.
    Unsupported,
    Failed(String),
}

pub struct PreviewGpu {
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
    /// When the picture being worked on was sent, while there is one.
    sent: Cell<Option<Instant>>,
}

/// How long the chip may take before it counts as stuck.
pub const STUCK_AFTER: Duration = Duration::from_secs(5);

fn work(jobs: Receiver<Job>, replies: Sender<Reply>) {
    let mut card: Option<GpuCompositor> = None;
    let mut size = (0, 0);
    while let Ok(job) = jobs.recv() {
        if card.is_none() || size != job.size {
            card = None; // the old one goes before the new one starts
            crate::startup::step("preview: opening the graphics chip (EGL)");
            match GpuCompositor::new(job.size.0, job.size.1) {
                Ok(c) => {
                    eprintln!(
                        "kani: preview put together on the graphics chip ({})",
                        c.renderer
                    );
                    card = Some(c);
                    size = job.size;
                }
                Err(why) => {
                    if replies.send(Reply::Failed(why)).is_err() {
                        return;
                    }
                    continue;
                }
            }
        }
        let Some(c) = card.as_mut() else { continue };
        let began = Instant::now();
        c.begin_picture();
        let mut outcome: Result<(), String> = Ok(());
        for (frame, placement) in &job.layers {
            if let Err(why) = c.draw(frame, placement) {
                outcome = Err(why);
                break;
            }
        }
        let draw = began.elapsed();
        let read_began = Instant::now();
        let reply = match outcome
            .and_then(|()| c.start_picture_read())
            .and_then(|()| c.take_picture())
        {
            Ok(rgba) => Reply::Picture(
                rgba,
                Timing {
                    draw,
                    read: read_began.elapsed(),
                },
            ),
            Err(why) if why.starts_with(UNSUPPORTED) => {
                // A half-drawn picture may still be waiting to be read back.
                while c.pictures_waiting() > 0 {
                    let _ = c.take_picture();
                }
                Reply::Unsupported
            }
            Err(why) => Reply::Failed(why),
        };
        if replies.send(reply).is_err() {
            return;
        }
    }
}

impl PreviewGpu {
    pub fn start() -> PreviewGpu {
        let (jobs, job_rx) = channel();
        let (reply_tx, replies) = channel();
        std::thread::Builder::new()
            .name("kani-preview-gpu".into())
            .spawn(move || work(job_rx, reply_tx))
            .ok();
        PreviewGpu {
            jobs,
            replies,
            sent: Cell::new(None),
        }
    }

    /// Send the layers to be drawn; returns at once. One picture at a time:
    /// call [`PreviewGpu::poll`] for the answer.
    pub fn submit(
        &self,
        size: (u32, u32),
        layers: Vec<(Arc<Frame>, Placement)>,
    ) -> Result<(), String> {
        self.jobs
            .send(Job { size, layers })
            .map_err(|_| "the graphics thread stopped".to_string())?;
        self.sent.set(Some(Instant::now()));
        Ok(())
    }

    /// Is a picture being drawn? For how long?
    pub fn busy_for(&self) -> Option<Duration> {
        self.sent.get().map(|t| t.elapsed())
    }

    /// The finished picture, if it is ready (never waits).
    pub fn poll(&self) -> Option<Reply> {
        let reply = self.replies.try_recv().ok()?;
        self.sent.set(None);
        Some(reply)
    }

    /// The picture, waiting for it: `Ok(None)` when the chip cannot do this
    /// one (use the processor), or why the chip is no use.
    pub fn compose(
        &self,
        size: (u32, u32),
        layers: Vec<(Arc<Frame>, Placement)>,
    ) -> Result<Option<Vec<u8>>, String> {
        self.submit(size, layers)?;
        let reply = self.replies.recv_timeout(STUCK_AFTER);
        self.sent.set(None);
        match reply {
            Ok(Reply::Picture(rgba, _)) => Ok(Some(rgba)),
            Ok(Reply::Unsupported) => Ok(None),
            Ok(Reply::Failed(why)) => Err(why),
            Err(_) => Err("the graphics chip took too long".into()),
        }
    }
}
