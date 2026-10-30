// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: MIT

mod haptics;
mod theme;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use entropy_core::{qr_matrix, QuizOutcome, SampleResult, Session, WordCount};
use zeroize::Zeroizing;
use slint_keyos_platform::app_ui2;
use slint_keyos_platform::slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

app_ui2!("Entropy Arcade");

const MAX_TRAIL: usize = 200;
/// Trail dot lifetime: age advances 1/60 per 16 ms tick, so a dot lasts ~1 s.
const AGE_STEP: f32 = 1.0 / 60.0;

fn hw_random() -> Result<[u8; 32], ()> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|_| ())?;
    Ok(b)
}

/// Clears every secret-bearing UI property. Slint's own image copy is freed, not zeroized (accepted).
fn clear_secrets(st: &AppState) {
    let empty: Vec<SharedString> = Vec::new();
    st.set_page_words(ModelRc::from(Rc::new(VecModel::from(empty.clone()))));
    st.set_quiz_choices(ModelRc::from(Rc::new(VecModel::from(empty))));
    st.set_qr(Image::default());
    st.set_fingerprint(SharedString::new());
    st.set_art(SharedString::new());
    st.set_quiz_position(0);
    st.set_quiz_step(0);
    st.set_page(0);
    st.set_first_index(0);
    st.set_last_guess(-1);
    st.set_last_credit(SharedString::new());
    st.set_read_pct(0);
}

fn strs(v: &[String]) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(v.iter().map(|w| SharedString::from(w.as_str())).collect::<Vec<_>>())))
}

fn show_words(st: &AppState, s: &Session) {
    st.set_page_words(strs(s.page_words()));
    st.set_page(s.page() as i32);
    st.set_page_count(s.page_count() as i32);
    st.set_first_index((s.page() * entropy_core::WORDS_PER_PAGE + 1) as i32);
    st.set_screen(Screen::Words);
}

fn show_quiz(st: &AppState, s: &Session) {
    if let Some(q) = s.quiz_question() {
        st.set_quiz_position(q.position as i32);
        st.set_quiz_step(s.quiz_step() as i32);
        st.set_quiz_choices(strs(&q.choices));
    }
}

/// Renders the SeedQR (4-module quiet zone, integer scale to ~400 px). The Rust-side bytes are zeroized.
fn render_qr(digits: &str) -> Image {
    let (n, dark) = qr_matrix(digits);
    let total = n + 8;
    let scale = (400 / total).max(1);
    let px = total * scale;
    let mut rgba = Zeroizing::new(vec![255u8; px * px * 4]);
    for y in 0..px {
        for x in 0..px {
            let (mx, my) = (x / scale, y / scale);
            if (4..4 + n).contains(&mx) && (4..4 + n).contains(&my) && dark[(my - 4) * n + (mx - 4)] {
                let i = (y * px + x) * 4;
                rgba[i..i + 3].fill(0);
            }
        }
    }
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, px as u32, px as u32);
    Image::from_rgba8(buf)
}

/// Pushes pool counters (never secrets) into the UI.
fn sync_meter(ui: &AppWindow, s: &Session) {
    let st = ui.global::<AppState>();
    if let Some(p) = s.pool() {
        // Display-only cap so the shown total never exceeds the target (camera <= half, touch fills the rest).
        let target = s.target_bits();
        let cam = p.camera_bits().min(target / 2);
        let touch = p.touch_bits().min(target - cam);
        st.set_touch_bits(touch as i32);
        st.set_camera_bits(cam as i32);
        st.set_touch_healthy(p.touch_healthy());
        st.set_camera_healthy(p.camera_healthy());
        st.set_game_bits(p.game_bits().min(target - cam - touch) as i32);
        st.set_game_healthy(p.game_healthy());
    }
    st.set_target_bits(s.target_bits() as i32);
}

/// Fires haptics from pool counters (never secrets): tick per 8 credited bits, ramp at 90%, full, alert on unhealthy.
#[derive(Default)]
struct Feedback {
    ticks: u32,
    ramped: bool,
    full: bool,
    touch_ok: bool,
    game_ok: bool,
}

impl Feedback {
    fn new() -> Self {
        Self { touch_ok: true, game_ok: true, ..Default::default() }
    }

    fn update(&mut self, credited: u32, target: u32, full: bool, touch_ok: bool, game_ok: bool) {
        if (self.touch_ok && !touch_ok) || (self.game_ok && !game_ok) {
            haptics::alert();
        }
        self.touch_ok = touch_ok;
        self.game_ok = game_ok;
        if full {
            if !self.full {
                self.full = true;
                haptics::full();
            }
        } else if credited / 8 > self.ticks {
            haptics::tick();
        }
        self.ticks = credited / 8;
        if !self.ramped && credited * 10 >= target * 9 {
            self.ramped = true;
            if !full {
                haptics::ramp();
            }
        }
    }
}

fn app_main(_cx: AppContext, ui: AppWindow) {
    log_server::init_wait(env!("CARGO_CRATE_NAME")).unwrap();
    log::set_max_level(log::LevelFilter::Info);

    theme::init(&ui);

    let session = Rc::new(RefCell::new(Session::new()));
    let trail: Rc<VecModel<TrailDot>> = Rc::new(VecModel::default());
    let start = Instant::now();
    let fb = Rc::new(RefCell::new(Feedback::new()));
    let st = ui.global::<AppState>();
    st.set_trail(ModelRc::from(trail.clone()));

    let (u, s, t, f) = (ui.as_weak(), session.clone(), trail.clone(), fb.clone());
    st.on_start_arcade(move |n, game| {
        *f.borrow_mut() = Feedback::new();
        let ui = u.upgrade().unwrap();
        let wc = if n == 24 { WordCount::TwentyFour } else { WordCount::Twelve };
        s.borrow_mut().start_arcade(wc);
        t.set_vec(Vec::new());
        sync_meter(&ui, &s.borrow());
        let st = ui.global::<AppState>();
        st.set_last_guess(-1);
        st.set_last_hit(false);
        st.set_last_credit(SharedString::new());
        st.set_read_pct(0);
        // Game 2 (hunt) lands in Task 10; until then it uses the Painter.
        st.set_screen(if game == 1 { Screen::Outguess } else { Screen::Painter });
        log::info!("arcade started: {} words", n);
    });

    let (u, s, f) = (ui.as_weak(), session.clone(), fb.clone());
    st.on_pick_door(move |door, x, y| {
        let ui = u.upgrade().unwrap();
        let t_us = start.elapsed().as_micros() as u64;
        let m = s.borrow_mut().outguess_pick(door as u8, x as i32, y as i32, t_us);
        sync_meter(&ui, &s.borrow());
        // Only now, after the pick is committed, does the guess reach the UI.
        let st = ui.global::<AppState>();
        st.set_last_guess(m.predicted as i32);
        st.set_last_hit(m.hit);
        st.set_last_credit(SharedString::from(format!("+{:.1} bits", m.credit_mbits as f32 / 1000.0).as_str()));
        st.set_read_pct(m.read_pct as i32);
        let sb = s.borrow();
        if let Some(p) = sb.pool() {
            f.borrow_mut().update(p.credited_bits(), sb.target_bits(), p.is_full(), p.touch_healthy(), p.game_healthy());
        }
    });

    let u = ui.as_weak();
    st.on_start_dice(move |_| {
        // Dice Duel is built in Task 9.
        u.upgrade().unwrap().global::<AppState>().set_screen(Screen::Dice);
    });

    let (u, s, t, f) = (ui.as_weak(), session.clone(), trail.clone(), fb.clone());
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
            let sb = s.borrow();
            if let Some(p) = sb.pool() {
                f.borrow_mut().update(p.credited_bits(), sb.target_bits(), p.is_full(), p.touch_healthy(), p.game_healthy());
            }
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
        let r = s.borrow_mut().generate(hw_random());
        match r {
            Ok(()) => show_words(&st, &s.borrow()),
            Err(e) => {
                st.set_error_text(SharedString::from(e.as_str()));
                st.set_screen(Screen::Error);
            }
        }
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_next_page(move || {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        if s.borrow_mut().next_page() {
            show_words(&st, &s.borrow());
        } else if let Some(d) = s.borrow().qr_digits() {
            st.set_qr(render_qr(&d));
            st.set_screen(Screen::Seedqr);
        }
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_prev_page(move || {
        let ui = u.upgrade().unwrap();
        s.borrow_mut().prev_page();
        show_words(&ui.global::<AppState>(), &s.borrow());
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_start_quiz(move || {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        match hw_random() {
            Ok(r) => {
                let r = Zeroizing::new(r);
                s.borrow_mut().start_quiz(&r);
                st.set_qr(Image::default());
                show_quiz(&st, &s.borrow());
                st.set_screen(Screen::Quiz);
            }
            Err(()) => {
                s.borrow_mut().wipe();
                clear_secrets(&st);
                st.set_error_text(SharedString::from("Hardware random unavailable"));
                st.set_screen(Screen::Error);
            }
        }
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_answer(move |i| {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        let outcome = s.borrow_mut().answer(i as usize);
        match outcome {
            QuizOutcome::Next => show_quiz(&st, &s.borrow()),
            QuizOutcome::Restart => {
                st.set_quiz_choices(ModelRc::from(Rc::new(VecModel::<SharedString>::default())));
                show_words(&st, &s.borrow());
            }
            QuizOutcome::Done => {
                let fp = s.borrow().fingerprint();
                if let Some((fp, art)) = fp {
                    let hex: String = fp.iter().map(|b| format!("{b:02x}")).collect();
                    st.set_fingerprint(SharedString::from(hex.as_str()));
                    st.set_art(SharedString::from(art.as_str()));
                }
                // Derived display strings stay; entropy, words and pool are wiped before "App buffers wiped" shows.
                s.borrow_mut().wipe();
                st.set_quiz_choices(ModelRc::from(Rc::new(VecModel::<SharedString>::default())));
                st.set_page_words(ModelRc::from(Rc::new(VecModel::<SharedString>::default())));
                st.set_screen(Screen::Done);
            }
        }
    });

    // finish() and back-home() both wipe the session and every secret-bearing UI property.
    let (u, s, t) = (ui.as_weak(), session.clone(), trail.clone());
    let leave = Rc::new(move || {
        s.borrow_mut().wipe();
        t.set_vec(Vec::new());
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        clear_secrets(&st);
        st.set_screen(Screen::Home);
    });
    let l = leave.clone();
    st.on_back_home(move || l());
    st.on_finish(move || leave());

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
