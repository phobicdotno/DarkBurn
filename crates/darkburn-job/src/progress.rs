//! Progress reporting and cancellation for long jobs.

use std::sync::atomic::{AtomicBool, Ordering};

/// The job was cancelled through [`Progress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Progress callback and cancel flag. Both are optional.
#[derive(Default)]
pub struct Progress<'a> {
    pub cancel: Option<&'a AtomicBool>,
    /// Called with a fraction 0..=1.
    pub on_progress: Option<&'a mut dyn FnMut(f32)>,
    /// Range of the overall progress this stage maps onto.
    pub(crate) span: (f32, f32),
}

impl<'a> Progress<'a> {
    pub fn none() -> Self {
        Self { cancel: None, on_progress: None, span: (0.0, 1.0) }
    }

    pub fn new(cancel: Option<&'a AtomicBool>, on_progress: Option<&'a mut dyn FnMut(f32)>) -> Self {
        Self { cancel, on_progress, span: (0.0, 1.0) }
    }

    /// Report `fraction` of the current stage and check for cancellation.
    pub fn report(&mut self, fraction: f32) -> Result<(), Cancelled> {
        let (a, b) = if self.span == (0.0, 0.0) { (0.0, 1.0) } else { self.span };
        if let Some(cb) = self.on_progress.as_mut() {
            cb(a + (b - a) * fraction.clamp(0.0, 1.0));
        }
        self.check()
    }

    /// Check for cancellation only.
    pub fn check(&self) -> Result<(), Cancelled> {
        match self.cancel {
            Some(c) if c.load(Ordering::Relaxed) => Err(Cancelled),
            _ => Ok(()),
        }
    }

    pub(crate) fn set_span(&mut self, a: f32, b: f32) {
        self.span = (a, b);
    }
}
