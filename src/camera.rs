// SPDX-License-Identifier: MIT

//! LavaRand: webcam frames as entropy. Frame bytes, hashes and preview pixels are never logged.
//! Only the previous preview's low bits are kept (for the XOR noise visual) and zeroized on stop.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use entropy_core::Session;
use slint_keyos_platform::slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, SharedString, Weak};
use slint_keyos_platform::TaskHandle;
use zeroize::Zeroize;

use crate::{AppState, AppWindow, Feedback};

camera::use_api!();

const PV: usize = 120;
const STEP: usize = camera::CAMERA_WIDTH / PV;
const BPP: usize = camera::CAMERA_BYTES_PER_PX;

thread_local! {
    static API: OnceCell<Option<CameraApi>> = const { OnceCell::new() };
    static TASK: RefCell<Option<TaskHandle<()>>> = const { RefCell::new(None) };
    static PREV: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn with_api<R>(f: impl FnOnce(&CameraApi) -> R) -> Option<R> {
    API.with(|c| c.get_or_init(|| CameraApi::try_new_with_timeout(Duration::from_millis(500))).as_ref().map(f))
}

/// Downscaled XOR of the low 2 bits against the previous frame, as magenta noise. The raw image is never shown.
fn preview(bytes: &[u8]) -> Option<Image> {
    if bytes.len() < camera::CAMERA_WIDTH * camera::CAMERA_HEIGHT * BPP {
        return None;
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(PV as u32, PV as u32);
    PREV.with(|p| {
        let mut prev = p.borrow_mut();
        prev.resize(PV * PV, 0);
        for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
            let (y, x) = (i / PV, i % PV);
            let v = bytes[(y * STEP * camera::CAMERA_WIDTH + x * STEP) * BPP] & 3;
            let n = (v ^ prev[i]) as u32 * 85;
            prev[i] = v;
            *px = Rgba8Pixel { r: n as u8, g: (n * 43 / 255) as u8, b: (n * 214 / 255) as u8, a: 255 };
        }
    });
    Some(Image::from_rgba8(buf))
}

fn fail(ui: &Weak<AppWindow>) {
    stop();
    if let Some(ui) = ui.upgrade() {
        let st = ui.global::<AppState>();
        st.set_camera_on(false);
        st.set_camera_preview(Image::default());
        st.set_camera_note(SharedString::from("Camera unavailable"));
    }
}

/// Starts the frame task. Returns false (camera stays off) if the API is unreachable;
/// a subscription failure turns the toggle back off with a note.
pub fn start(ui: Weak<AppWindow>, session: Rc<RefCell<Session>>, t0: Instant, fb: Rc<RefCell<Feedback>>) -> bool {
    stop();
    // SetEnabled/NotifyVisible are Foundation-only; the system owns camera power. Subscribe directly.
    // If the system camera reports off we keep the subscription and hint, since frames start when it is switched on.
    let Some(enabled) = with_api(|a| a.is_enabled()) else {
        log::warn!("camera: api unreachable");
        return false;
    };
    if !enabled {
        if let Some(ui) = ui.upgrade() {
            ui.global::<AppState>().set_camera_note(SharedString::from("Turn on the camera in Control Center"));
        }
    }
    let sub = slint_keyos_platform::try_subscribe_scalar::<camera_permissions::CameraPermissions, camera::messages::Subscribe>(
        camera::messages::Subscribe,
    );
    let task = slint_keyos_platform::spawn_local(async move {
        let mut frames = match sub.await {
            Ok(f) => f,
            Err(e) => {
                log::warn!("camera: subscribe failed: {e:?}");
                return fail(&ui);
            }
        };
        log::info!("camera: subscribed");
        while let Some(frame) = frames.next().await {
            let Some(w) = ui.upgrade() else { break };
            let st = w.global::<AppState>();
            st.set_camera_note(SharedString::new());
            if !st.get_camera_on() {
                continue;
            }
            // Same clock as touch: the 250 ms touch window compares these.
            let t_us = t0.elapsed().as_micros() as u64;
            let content = frame.content();
            let bytes: &[u8] = content.as_slice();
            let r = session.borrow_mut().on_camera_frame(bytes, t_us);
            if let Some(img) = preview(bytes) {
                st.set_camera_preview(img);
            }
            if r != entropy_core::SampleResult::Uncredited {
                crate::credit(&w, &session.borrow(), &fb);
            }
        }
    });
    TASK.with(|t| *t.borrow_mut() = Some(task));
    true
}

/// Stops feeding (system owns camera power), ends the frame task (dropping it cancels the subscription) and wipes the preview buffer.
pub fn stop() {
    TASK.with(|t| t.borrow_mut().take());
    PREV.with(|p| {
        let mut p = p.borrow_mut();
        p.zeroize();
        p.clear();
    });
}
