use std::{collections::VecDeque, time::Duration};

use indicatif::{style::ProgressTracker, HumanBytes, ProgressBar, ProgressStyle};
use serde::Deserialize;

/// The asynchronous version of the API
#[cfg(feature = "tokio")]
pub mod tokio;

/// The synchronous version of the API
#[cfg(feature = "ureq")]
pub mod sync;

const HF_ENDPOINT: &str = "HF_ENDPOINT";

/// This trait is used by users of the lib
/// to implement custom behavior during file downloads
pub trait Progress {
    /// At the start of the download
    /// The size is the total size in bytes of the file.
    fn init(&mut self, size: usize, filename: &str);
    /// This function is called whenever `size` bytes have been
    /// downloaded in the temporary file
    fn update(&mut self, size: usize);
    /// This is called at the end of the download
    fn finish(&mut self);
}

impl Progress for () {
    fn init(&mut self, _size: usize, _filename: &str) {}
    fn update(&mut self, _size: usize) {}
    fn finish(&mut self) {}
}

impl Progress for ProgressBar {
    fn init(&mut self, size: usize, filename: &str) {
        self.set_length(size as u64);
        self.set_style(
                ProgressStyle::with_template(
                    "{msg} [{elapsed_precise}] [{wide_bar}] {bytes}/{total_bytes} {bytes_per_sec_smoothed} ({eta})",
                ).unwrap().with_key("bytes_per_sec_smoothed", MovingAvgRate::default())
                    ,
            );
        let maxlength = 30;
        let message = if filename.len() > maxlength {
            format!("..{}", &filename[filename.len() - maxlength..])
        } else {
            filename.to_string()
        };
        self.set_message(message);
    }

    fn update(&mut self, size: usize) {
        self.inc(size as u64)
    }

    fn finish(&mut self) {
        ProgressBar::finish(self);
    }
}

/// Siblings are simplified file descriptions of remote files on the hub
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Siblings {
    /// The path within the repo.
    pub rfilename: String,
}

/// The description of the repo given by the hub
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RepoInfo {
    /// See [`Siblings`]
    pub siblings: Vec<Siblings>,

    /// The commit sha of the repo.
    pub sha: String,
}

#[derive(Clone, Default)]
struct MovingAvgRate {
    samples: VecDeque<(std::time::Instant, u64)>,
}

impl ProgressTracker for MovingAvgRate {
    fn clone_box(&self) -> Box<dyn ProgressTracker> {
        Box::new(self.clone())
    }

    fn tick(&mut self, state: &indicatif::ProgressState, now: std::time::Instant) {
        // sample at most every 20ms
        if self
            .samples
            .back()
            .is_none_or(|(prev, _)| (now - *prev) > Duration::from_millis(20))
        {
            self.samples.push_back((now, state.pos()));
        }

        while let Some(first) = self.samples.front() {
            if now - first.0 > Duration::from_secs(1) {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    fn reset(&mut self, _state: &indicatif::ProgressState, _now: std::time::Instant) {
        self.samples = Default::default();
    }

    fn write(&self, _state: &indicatif::ProgressState, w: &mut dyn std::fmt::Write) {
        match (self.samples.front(), self.samples.back()) {
            (Some((t0, p0)), Some((t1, p1))) if self.samples.len() > 1 => {
                let elapsed_ms = (*t1 - *t0).as_millis();
                let rate = ((p1 - p0) as f64 * 1000f64 / elapsed_ms as f64) as u64;
                write!(w, "{}/s", HumanBytes(rate)).unwrap()
            }
            _ => write!(w, "-").unwrap(),
        }
    }
}
