// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: MIT

mod theme;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use entropy_core::{SampleResult, Session, WordCount};
use slint_keyos_platform::app_ui2;
use slint_keyos_platform::slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

app_ui2!("Entropy Arcade");

const MAX_TRAIL: usize = 200;
/// Trail dot lifetime: age advances 1/60 per 16 ms tick, so a dot lasts ~1 s.
const AGE_STEP: f32 = 1.0 / 60.0;

fn hw_random() -> Result<[u8; 32], ()> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|_| ())?;
    Ok(b)
}

/// Pushes pool counters (never secrets) into the UI.
fn sync_meter(ui: &AppWindow, s: &Session) {
    let st = ui.global::<AppState>();
    if let Some(p) = s.pool() {
        // Display-only cap so the shown total never exceeds the target (camera <= half, touch fills the rest).
        let target = s.target_bits();
        let cam = p.camera_bits().min(target / 2);
        st.set_touch_bits(p.touch_bits().min(target - cam) as i32);
        st.set_camera_bits(cam as i32);
        st.set_touch_healthy(p.touch_healthy());
        st.set_camera_healthy(p.camera_healthy());
    }
    st.set_target_bits(s.target_bits() as i32);
}

fn app_main(_cx: AppContext, ui: AppWindow) {
    log_server::init_wait(env!("CARGO_CRATE_NAME")).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    theme::init(&ui);

    let session = Rc::new(RefCell::new(Session::new()));
    let trail: Rc<VecModel<TrailDot>> = Rc::new(VecModel::default());
    let start = Instant::now();
    let st = ui.global::<AppState>();
    st.set_trail(ModelRc::from(trail.clone()));

    let (u, s, t) = (ui.as_weak(), session.clone(), trail.clone());
    st.on_start_arcade(move |n| {
        let ui = u.upgrade().unwrap();
        let wc = if n == 24 { WordCount::TwentyFour } else { WordCount::Twelve };
        s.borrow_mut().start_arcade(wc);
        t.set_vec(Vec::new());
        sync_meter(&ui, &s.borrow());
        ui.global::<AppState>().set_screen(Screen::Painter);
        log::info!("arcade started: {} words", n);
    });

    let u = ui.as_weak();
    st.on_start_dice(move |_| {
        // Dice Duel is built in Task 9.
        u.upgrade().unwrap().global::<AppState>().set_screen(Screen::Dice);
    });

    let (u, s, t) = (ui.as_weak(), session.clone(), trail.clone());
    st.on_touch(move |x, y| {
        let ui = u.upgrade().unwrap();
        let t_us = start.elapsed().as_micros() as u64;
        let r = s.borrow_mut().on_touch(x as i32, y as i32, t_us);
        if t.row_count() >= MAX_TRAIL {
            t.remove(0);
        }
        t.push(TrailDot { x, y, age: 0.0 });
        if r != SampleResult::Uncredited {
            sync_meter(&ui, &s.borrow());
        }
    });

    let u = ui.as_weak();
    st.on_toggle_camera(move || {
        // Camera sampling is wired in Task 7; the flag only drives the UI for now.
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        st.set_camera_on(!st.get_camera_on());
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_painter_done(move || {
        let ui = u.upgrade().unwrap();
        if s.borrow().pool().is_some_and(|p| p.is_full()) {
            ui.global::<AppState>().set_screen(Screen::Summary);
        }
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_generate(move || {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        match s.borrow_mut().generate(hw_random()) {
            Ok(()) => st.set_screen(Screen::Words),
            Err(e) => {
                st.set_error_text(SharedString::from(e.as_str()));
                st.set_screen(Screen::Error);
            }
        }
    });

    let (u, s, t) = (ui.as_weak(), session.clone(), trail.clone());
    st.on_back_home(move || {
        s.borrow_mut().wipe();
        t.set_vec(Vec::new());
        u.upgrade().unwrap().global::<AppState>().set_screen(Screen::Home);
    });

    // Fade the trail; dots past age 1 are dropped.
    let timer = slint::Timer::default();
    let t = trail.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        let n = t.row_count();
        if n == 0 {
            return;
        }
        let mut dots: Vec<TrailDot> = (0..n).filter_map(|i| t.row_data(i)).collect();
        for d in &mut dots {
            d.age += AGE_STEP;
        }
        dots.retain(|d| d.age < 1.0);
        t.set_vec(dots);
    });

    ui.run().expect("UI running");
}
