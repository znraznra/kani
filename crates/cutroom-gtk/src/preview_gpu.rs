//! Puts the preview picture together on the graphics chip.
//!
//! The chip's drawing context lives on a thread of its own: making an OpenGL
//! context current on the window's thread would upset the toolkit that draws
//! the window. The preview sends the layers over and waits for the picture.

use crate::gpu_compose::{GpuCompositor, UNSUPPORTED};
use cutroom_compose::{Frame, Placement};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

struct Job {
    size: (u32, u32),
    layers: Vec<(Arc<Frame>, Placement)>,
}

enum Reply {
    Picture(Vec<u8>),
    /// One layer the chip cannot place: the processor does this picture.
    Unsupported,
    Failed(String),
}

pub struct PreviewGpu {
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
}

fn work(jobs: Receiver<Job>, replies: Sender<Reply>) {
    let mut card: Option<GpuCompositor> = None;
    let mut size = (0, 0);
    while let Ok(job) = jobs.recv() {
        if card.is_none() || size != job.size {
            card = None; // the old one goes before the new one starts
            match GpuCompositor::new(job.size.0, job.size.1) {
                Ok(c) => {
                    eprintln!("kani: preview put together on the graphics chip ({})", c.renderer);
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
        c.begin_picture();
        let mut outcome: Result<(), String> = Ok(());
        for (frame, placement) in &job.layers {
            if let Err(why) = c.draw(frame, placement) {
                outcome = Err(why);
                break;
            }
        }
        let reply = match outcome
            .and_then(|()| c.start_picture_read())
            .and_then(|()| c.take_picture())
        {
            Ok(rgba) => Reply::Picture(rgba),
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
        PreviewGpu { jobs, replies }
    }

    /// The picture, `Ok(None)` when the chip cannot do this one (use the
    /// processor), or why the chip is no use.
    pub fn compose(
        &self,
        size: (u32, u32),
        layers: Vec<(Arc<Frame>, Placement)>,
    ) -> Result<Option<Vec<u8>>, String> {
        self.jobs
            .send(Job { size, layers })
            .map_err(|_| "the graphics thread stopped".to_string())?;
        match self.replies.recv_timeout(Duration::from_secs(5)) {
            Ok(Reply::Picture(rgba)) => Ok(Some(rgba)),
            Ok(Reply::Unsupported) => Ok(None),
            Ok(Reply::Failed(why)) => Err(why),
            Err(_) => Err("the graphics chip took too long".into()),
        }
    }
}
