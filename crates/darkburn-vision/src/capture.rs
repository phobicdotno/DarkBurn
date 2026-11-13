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

/// The system library a camera is opened through (SET-37). `Default` lets the platform choose;
/// `Simulated` is DarkBurn's own made-up camera ([`TestPattern`]), which opens no device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureSystem {
    #[default]
    Default,
    MediaFoundation,
    Video4Linux,
    AvFoundation,
    Simulated,
}

impl CaptureSystem {
    pub const ALL: [Self; 5] = [Self::Default, Self::MediaFoundation, Self::Video4Linux, Self::AvFoundation, Self::Simulated];

    /// The name stored in the preferences.
    pub fn key(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::MediaFoundation => "media_foundation",
            Self::Video4Linux => "video4linux",
            Self::AvFoundation => "avfoundation",
            Self::Simulated => "simulated",
        }
    }

    /// The name shown to the user.
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default for this computer",
            Self::MediaFoundation => "Media Foundation (Windows)",
            Self::Video4Linux => "Video4Linux (Linux)",
            Self::AvFoundation => "AVFoundation (macOS)",
            Self::Simulated => "Simulated camera",
        }
    }

    /// Reads a stored name. Short forms are taken too; a name this build does not know (an
    /// older "dshow", say) means the default, so an old preference never stops the camera.
    pub fn from_key(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "media_foundation" | "mediafoundation" | "msmf" => Self::MediaFoundation,
            "video4linux" | "v4l" | "v4l2" => Self::Video4Linux,
            "avfoundation" | "avf" => Self::AvFoundation,
            "simulated" | "simulator" | "sim" => Self::Simulated,
            _ => Self::Default,
        }
    }
}

/// What to open a camera with: the capture system and the picture size wanted. A camera that
/// does not offer that size gives its largest one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub system: CaptureSystem,
    pub width: u32,
    pub height: u32,
}

impl Default for OpenRequest {
    fn default() -> Self {
        Self { system: CaptureSystem::Default, width: 1280, height: 720 }
    }
}

/// A made-up camera that never runs out: a left-to-right grey ramp with a white square that moves
/// on every frame. Brightness adds to every pixel and exposure scales it, the way the real
/// controls lighten and darken a picture, so tests can see the settings arrive.
#[derive(Clone, Debug)]
pub struct TestPattern {
    pub width: u32,
    pub height: u32,
    /// Frames delivered so far.
    pub frames: u64,
    pub exposure: Option<f64>,
    pub brightness: Option<f64>,
}

impl TestPattern {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width: width.max(16), height: height.max(16), frames: 0, exposure: None, brightness: None }
    }
}

impl FrameSource for TestPattern {
    fn grab(&mut self) -> Result<RgbaImage, CaptureError> {
        let (w, h) = (self.width, self.height);
        let side = (w.min(h) / 8).max(4);
        let x0 = (self.frames as u32).wrapping_mul(side / 2 + 1) % (w - side);
        let y0 = (h - side) / 2;
        let gain = 2f64.powf(self.exposure.unwrap_or(0.0) / 4.0);
        let offset = self.brightness.unwrap_or(0.0);
        self.frames += 1;
        Ok(RgbaImage::from_fn(w, h, |x, y| {
            let inside = (x0..x0 + side).contains(&x) && (y0..y0 + side).contains(&y);
            let base = if inside { 255.0 } else { 32.0 + 160.0 * f64::from(x) / f64::from(w) };
            let v = (base * gain + offset).round().clamp(0.0, 255.0) as u8;
            // A little colour, so a black and white view can be told apart from a colour one.
            image::Rgba([v, v.saturating_sub(40), v / 2, 255])
        }))
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
    use super::{CaptureError, CaptureSystem, FrameSource, OpenRequest};
    use image::RgbaImage;
    use nokhwa::Camera;
    use nokhwa::pixel_format::RgbAFormat;
    use nokhwa::utils::{ApiBackend, CameraIndex, ControlValueSetter, KnownCameraControl, RequestedFormat, RequestedFormatType, Resolution};

    fn backend(system: CaptureSystem) -> Result<ApiBackend, CaptureError> {
        Ok(match system {
            CaptureSystem::Default => ApiBackend::Auto,
            CaptureSystem::MediaFoundation => ApiBackend::MediaFoundation,
            CaptureSystem::Video4Linux => ApiBackend::Video4Linux,
            CaptureSystem::AvFoundation => ApiBackend::AVFoundation,
            CaptureSystem::Simulated => return Err(CaptureError::Unsupported),
        })
    }

    /// Names of the cameras the system reports, with their index.
    pub fn list_cameras() -> Result<Vec<(u32, String)>, CaptureError> {
        list_cameras_with(CaptureSystem::Default)
    }

    /// Names of the cameras `system` reports, with their index.
    pub fn list_cameras_with(system: CaptureSystem) -> Result<Vec<(u32, String)>, CaptureError> {
        let infos = nokhwa::query(backend(system)?).map_err(|e| CaptureError::Device(e.to_string()))?;
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
            Self::open_with(index, &OpenRequest { width: 0, height: 0, ..OpenRequest::default() })
        }

        /// Open camera `index` through the requested capture system at the requested size, or
        /// at its largest size when it does not offer that one (or the size is 0 x 0).
        pub fn open_with(index: u32, req: &OpenRequest) -> Result<Self, CaptureError> {
            let api = backend(req.system)?;
            let largest = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::AbsoluteHighestResolution);
            let cam = if req.width > 0 && req.height > 0 {
                let sized = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::HighestResolution(Resolution::new(req.width, req.height)));
                Camera::with_backend(CameraIndex::Index(index), sized, api).or_else(|_| Camera::with_backend(CameraIndex::Index(index), largest, api))
            } else {
                Camera::with_backend(CameraIndex::Index(index), largest, api)
            };
            let mut cam = cam.map_err(|e| CaptureError::Device(e.to_string()))?;
            cam.open_stream().map_err(|e| CaptureError::Device(e.to_string()))?;
            Ok(Self { cam })
        }

        fn set(&mut self, control: KnownCameraControl, value: f64) -> Result<(), CaptureError> {
            self.cam.set_camera_control(control, ControlValueSetter::Integer(value.round() as i64)).map_err(|e| CaptureError::Device(e.to_string()))
        }
    }

    impl FrameSource for LiveCamera {
        /// The camera's own exposure number (on Windows usually a power of two in seconds, -13 to -1).
        fn set_exposure(&mut self, value: f64) -> Result<(), CaptureError> {
            self.set(KnownCameraControl::Exposure, value)
        }

        /// The camera's own brightness number.
        fn set_brightness(&mut self, value: f64) -> Result<(), CaptureError> {
            self.set(KnownCameraControl::Brightness, value)
        }

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
    fn capture_systems_read_back_and_unknown_names_mean_the_default() {
        for s in CaptureSystem::ALL {
            assert_eq!(CaptureSystem::from_key(s.key()), s);
        }
        assert_eq!(CaptureSystem::from_key("MSMF"), CaptureSystem::MediaFoundation);
        assert_eq!(CaptureSystem::from_key("v4l2"), CaptureSystem::Video4Linux);
        assert_eq!(CaptureSystem::from_key("dshow"), CaptureSystem::Default);
        assert_eq!(CaptureSystem::from_key(""), CaptureSystem::Default);
    }

    #[test]
    fn the_test_pattern_never_runs_out_moves_and_follows_the_controls() {
        let mut s = TestPattern::new(320, 240);
        let a = s.grab().unwrap();
        let b = s.grab().unwrap();
        assert_eq!(a.dimensions(), (320, 240));
        assert_ne!(a, b, "the square moves between frames");
        for _ in 0..500 {
            s.grab().unwrap();
        }
        let plain = TestPattern::new(320, 240).grab().unwrap();
        let mut bright = TestPattern::new(320, 240);
        bright.set_brightness(40.0).unwrap();
        let mut dark = TestPattern::new(320, 240);
        dark.set_exposure(-4.0).unwrap();
        let px = |i: &RgbaImage| i.get_pixel(10, 10)[0];
        assert!(px(&bright.grab().unwrap()) > px(&plain), "brightness lightens the picture");
        assert!(px(&dark.grab().unwrap()) < px(&plain), "a shorter exposure darkens it");
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
