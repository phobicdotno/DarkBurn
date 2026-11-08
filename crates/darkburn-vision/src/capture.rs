//! Frame sources for the camera panel. The trait and a canned source are always available; live capture through
//! `nokhwa` is behind the off-by-default `capture` feature.

use image::RgbaImage;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// The source cannot do this (for example exposure on a canned source).
    Unsupported,
    /// No more frames.
    Exhausted,
    Device(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "not supported by this camera"),
            Self::Exhausted => write!(f, "no more frames"),
            Self::Device(s) => write!(f, "camera error: {s}"),
        }
    }
}
impl std::error::Error for CaptureError {}

/// Anything that can deliver camera frames.
pub trait FrameSource {
    fn grab(&mut self) -> Result<RgbaImage, CaptureError>;

    fn set_exposure(&mut self, _value: f64) -> Result<(), CaptureError> {
        Err(CaptureError::Unsupported)
    }

    fn set_brightness(&mut self, _value: f64) -> Result<(), CaptureError> {
        Err(CaptureError::Unsupported)
    }
}

/// Plays back prepared frames; used by tests and demos.
#[derive(Default)]
pub struct CannedFrames {
    frames: VecDeque<RgbaImage>,
    pub exposure: Option<f64>,
    pub brightness: Option<f64>,
}

impl CannedFrames {
    pub fn new(frames: impl IntoIterator<Item = RgbaImage>) -> Self {
        Self { frames: frames.into_iter().collect(), exposure: None, brightness: None }
    }
}

impl FrameSource for CannedFrames {
    fn grab(&mut self) -> Result<RgbaImage, CaptureError> {
        self.frames.pop_front().ok_or(CaptureError::Exhausted)
    }

    fn set_exposure(&mut self, value: f64) -> Result<(), CaptureError> {
        self.exposure = Some(value);
        Ok(())
    }

    fn set_brightness(&mut self, value: f64) -> Result<(), CaptureError> {
        self.brightness = Some(value);
        Ok(())
    }
}

#[cfg(feature = "capture")]
pub mod live {
    //! Live webcam capture (nokhwa, MIT/Apache-2.0).
    use super::{CaptureError, FrameSource};
    use image::RgbaImage;
    use nokhwa::Camera;
    use nokhwa::pixel_format::RgbAFormat;
    use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType};

    /// Names of the cameras the system reports, with their index.
    pub fn list_cameras() -> Result<Vec<(u32, String)>, CaptureError> {
        let infos = nokhwa::query(ApiBackend::Auto).map_err(|e| CaptureError::Device(e.to_string()))?;
        Ok(infos
            .into_iter()
            .enumerate()
            .map(|(i, info)| (if let CameraIndex::Index(n) = info.index() { *n } else { i as u32 }, info.human_name()))
            .collect())
    }

    pub struct LiveCamera {
        cam: Camera,
    }

    impl LiveCamera {
        /// Open camera `index` at the highest resolution it offers and start streaming.
        pub fn open(index: u32) -> Result<Self, CaptureError> {
            let fmt = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::AbsoluteHighestResolution);
            let mut cam = Camera::new(CameraIndex::Index(index), fmt).map_err(|e| CaptureError::Device(e.to_string()))?;
            cam.open_stream().map_err(|e| CaptureError::Device(e.to_string()))?;
            Ok(Self { cam })
        }
    }

    impl FrameSource for LiveCamera {
        fn grab(&mut self) -> Result<RgbaImage, CaptureError> {
            let buf = self.cam.frame().map_err(|e| CaptureError::Device(e.to_string()))?;
            let img = buf.decode_image::<RgbAFormat>().map_err(|e| CaptureError::Device(e.to_string()))?;
            RgbaImage::from_raw(img.width(), img.height(), img.into_raw()).ok_or_else(|| CaptureError::Device("bad frame size".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canned_source_plays_back_and_runs_out() {
        let mut s = CannedFrames::new([RgbaImage::new(2, 2), RgbaImage::new(3, 3)]);
        s.set_exposure(-4.0).unwrap();
        assert_eq!(s.exposure, Some(-4.0));
        assert_eq!(s.grab().unwrap().width(), 2);
        assert_eq!(s.grab().unwrap().width(), 3);
        assert_eq!(s.grab().unwrap_err(), CaptureError::Exhausted);
    }

    #[test]
    fn default_trait_methods_are_unsupported() {
        struct Dummy;
        impl FrameSource for Dummy {
            fn grab(&mut self) -> Result<RgbaImage, CaptureError> {
                Ok(RgbaImage::new(1, 1))
            }
        }
        assert_eq!(Dummy.set_exposure(1.0).unwrap_err(), CaptureError::Unsupported);
        assert_eq!(Dummy.set_brightness(1.0).unwrap_err(), CaptureError::Unsupported);
    }
}
