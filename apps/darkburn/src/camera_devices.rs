//! Where the Camera Control panel finds and opens cameras [CAMP-01, CAMP-02, CAM-01, SET-37].
//!
//! The panel and the `camera.find`, `camera.start` and `camera.stop` commands go through a
//! [`CameraProvider`], never straight to the camera library. The system provider opens real
//! cameras (only in a build with live capture); the simulated one hands out made-up cameras that
//! show a moving test picture, for trying the panel without a camera and for the tests. Under
//! `cargo test` the system provider refuses: a test can never open a real camera.

use darkburn_vision::capture::{CaptureSystem, FrameSource, OpenRequest, TestPattern};
use serde_json::{Value, json};

use crate::shell::DarkBurn;

/// Lists and opens cameras.
pub trait CameraProvider {
    fn list(&mut self, system: CaptureSystem) -> Result<Vec<(u32, String)>, String>;
    fn open(&mut self, index: u32, req: &OpenRequest) -> Result<Box<dyn FrameSource>, String>;
}

/// Two made-up cameras that show [`TestPattern`] at the requested size.
pub struct SimulatedCameras;

pub const SIMULATED: [(u32, &str); 2] = [(0, "Simulated overhead camera"), (1, "Simulated head camera")];

impl CameraProvider for SimulatedCameras {
    fn list(&mut self, _system: CaptureSystem) -> Result<Vec<(u32, String)>, String> {
        Ok(SIMULATED.iter().map(|(i, n)| (*i, (*n).to_string())).collect())
    }

    fn open(&mut self, index: u32, req: &OpenRequest) -> Result<Box<dyn FrameSource>, String> {
        if !SIMULATED.iter().any(|(i, _)| *i == index) {
            return Err(format!("There is no camera {index}."));
        }
        Ok(Box::new(TestPattern::new(req.width, req.height)))
    }
}

/// The cameras the operating system reports.
pub struct SystemCameras;

#[cfg(all(feature = "live-camera", not(test)))]
impl CameraProvider for SystemCameras {
    fn list(&mut self, system: CaptureSystem) -> Result<Vec<(u32, String)>, String> {
        darkburn_vision::capture::live::list_cameras_with(system).map_err(|e| e.to_string())
    }

    fn open(&mut self, index: u32, req: &OpenRequest) -> Result<Box<dyn FrameSource>, String> {
        darkburn_vision::capture::live::LiveCamera::open_with(index, req).map(|c| Box::new(c) as Box<dyn FrameSource>).map_err(|e| e.to_string())
    }
}

#[cfg(any(not(feature = "live-camera"), test))]
impl CameraProvider for SystemCameras {
    fn list(&mut self, _system: CaptureSystem) -> Result<Vec<(u32, String)>, String> {
        Err(NO_LIVE.into())
    }

    fn open(&mut self, _index: u32, _req: &OpenRequest) -> Result<Box<dyn FrameSource>, String> {
        Err(NO_LIVE.into())
    }
}

#[cfg(any(not(feature = "live-camera"), test))]
const NO_LIVE: &str = "This build was made without live camera support. Choose the simulated camera in Settings, or use a picture file.";

/// The capture system chosen in Settings.
pub fn system(app: &DarkBurn) -> CaptureSystem {
    CaptureSystem::from_key(&app.prefs.opts.capture_system)
}

/// What a camera is opened with: the capture system and size from Settings [SET-37].
pub fn request(app: &DarkBurn) -> OpenRequest {
    OpenRequest { system: system(app), width: app.prefs.opts.capture_width, height: app.prefs.opts.capture_height }
}

/// Whether the panel can offer cameras at all: a build with live capture, or the simulated camera.
pub fn available(app: &DarkBurn) -> bool {
    crate::vision_cmds::live_built() || system(app) == CaptureSystem::Simulated
}

fn with_provider<R>(app: &mut DarkBurn, f: impl FnOnce(&mut dyn CameraProvider, &OpenRequest) -> R) -> R {
    let req = request(app);
    if req.system == CaptureSystem::Simulated { f(&mut SimulatedCameras, &req) } else { f(&mut SystemCameras, &req) }
}

/// Find Cameras [CAMP-01]: ask the capture system which cameras there are.
pub fn find(app: &mut DarkBurn) -> Result<Value, String> {
    let found = with_provider(app, |p, req| p.list(req.system));
    match found {
        Ok(v) => {
            app.vision.message = if v.is_empty() { Some("No camera found.".into()) } else { None };
            app.vision.cameras = v;
            Ok(json!({
                "system": system(app).key(),
                "cameras": app.vision.cameras.iter().map(|(i, n)| json!({ "index": i, "name": n })).collect::<Vec<_>>(),
            }))
        }
        Err(e) => {
            app.vision.message = Some(e.clone());
            Err(e)
        }
    }
}

/// Start [CAMP-02, CAM-01]: open the chosen camera (or `index`), give it the saved exposure and
/// brightness and show its pictures in the viewfinder.
pub fn start(app: &mut DarkBurn, args: &Value) -> Result<Value, String> {
    if !app.vision.prefs.live_enabled {
        return Err("Allow live camera first.".into());
    }
    let index = match args.get("index") {
        Some(v) => Some(v.as_u64().and_then(|i| u32::try_from(i).ok()).ok_or("index must be a whole number")?),
        None => app.vision.state.camera.device_index,
    };
    let Some(index) = index else {
        app.vision.message = Some("Pick a camera first.".into());
        return Err("Pick a camera first.".into());
    };
    if app.vision.cameras.is_empty() {
        let _ = find(app);
    }
    let opened = with_provider(app, |p, req| p.open(index, req));
    let mut src = match opened {
        Ok(s) => s,
        Err(e) => {
            app.vision.message = Some(e.clone());
            return Err(e);
        }
    };
    let cam = app.vision.state.camera.clone();
    let mut controls = Vec::new();
    if let Some(e) = cam.exposure {
        controls.push(("exposure", src.set_exposure(e).map_err(|e| e.to_string())));
    }
    if let Some(b) = cam.brightness {
        controls.push(("brightness", src.set_brightness(b).map_err(|e| e.to_string())));
    }
    let first = src.grab().map_err(|e| e.to_string())?;
    let name = app.vision.cameras.iter().find(|(i, _)| *i == index).map(|(_, n)| n.clone()).unwrap_or_else(|| format!("Camera {index}"));
    if cam.device_index != Some(index) || cam.camera_name != name {
        app.vision.state.camera.device_index = Some(index);
        app.vision.state.camera.camera_name = name.clone();
        crate::vision_cmds::commit(app, "Camera choice", false);
    }
    let (w, h) = first.dimensions();
    app.vision.raw = Some(first);
    app.vision.frame_gen += 1;
    app.vision.source = Some(src);
    app.vision.source_name = name.clone();
    app.vision.live = true;
    app.vision.message = controls.iter().find_map(|(what, r)| r.as_ref().err().map(|e| format!("This camera did not take the {what}: {e}")));
    Ok(json!({
        "live": true, "index": index, "name": name, "width": w, "height": h, "system": system(app).key(),
        "controls": controls.iter().map(|(what, r)| json!({ "control": what, "ok": r.is_ok() })).collect::<Vec<_>>(),
    }))
}

/// Stop: close the camera; the last picture stays in the viewfinder.
pub fn stop(app: &mut DarkBurn) -> Value {
    let was = app.vision.live;
    app.vision.live = false;
    app.vision.source = None;
    json!({ "live": false, "was_live": was })
}

/// The viewfinder picture: as captured, or in black and white when Settings says so [SET-37].
pub fn viewfinder(raw: &image::RgbaImage, colour: bool) -> std::borrow::Cow<'_, image::RgbaImage> {
    if colour {
        return std::borrow::Cow::Borrowed(raw);
    }
    let mut g = raw.clone();
    for p in g.pixels_mut() {
        let l = (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])).round() as u8;
        *p = image::Rgba([l, l, l, p[3]]);
    }
    std::borrow::Cow::Owned(g)
}
