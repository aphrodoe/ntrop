// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: MIT

mod camera;
mod duel;
mod haptics;
mod hunt;
mod shard;
mod theme;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use entropy_core::{qr_matrix, QuizOutcome, SampleResult, Session, WordCount};
use entropy_core::feedback::{Feedback, Pulse};
use zeroize::Zeroizing;
use slint_keyos_platform::app_ui2;
use slint_keyos_platform::slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

app_ui2!("Entropy Arcade");

/// A press that moves more than this many px is a drag (look); otherwise its release is a shot.
const DRAG_PX: f32 = 8.0;
const FRAME_MS: u64 = 33;
const POP_MS: u32 = 600;
const MAX_TRAIL: usize = 200;
/// Trail dot lifetime: age advances 1/60 per 16 ms tick, so a dot lasts ~1 s.
const AGE_STEP: f32 = 1.0 / 60.0;

/// Camera off everywhere it must not stay on: toggle, Done, Home, new run.
fn camera_off(st: &AppState) {
    camera::stop();
    st.set_camera_on(false);
    st.set_camera_preview(Image::default());
}

fn hw_random() -> Result<[u8; 32], ()> {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).map_err(|_| ())?;
    Ok(b)
}

/// Releases gameplay display data before entering the backup flow.
fn clear_game_models(st: &AppState) {
    duel::clear_ui(st);
    st.set_landing_cells(ModelRc::from(Rc::new(VecModel::from(vec![0i32; 128]))));
    st.set_shard_placements(0);
    st.set_shard_lines(0);
    st.set_shard_score(0);
    st.set_shard_level(1);
    st.set_shard_solver(SharedString::new());
    st.set_next_cells(ModelRc::from(Rc::new(VecModel::from(vec![0i32; 16]))));
    st.set_last_pick(-1);
    st.set_outguess_round(0);
    st.set_hunt_note(SharedString::new());
    st.set_hunt_pop(SharedString::new());
    st.set_dice_note(SharedString::new());
    st.set_shard_cells(ModelRc::from(Rc::new(VecModel::from(vec![0i32; 128]))));
    st.set_ghost_cells(ModelRc::from(Rc::new(VecModel::from(vec![0i32; 128]))));
    st.set_shard_note(SharedString::new());
    st.set_last_guess(-1);
    st.set_last_hit(false);
    st.set_last_credit(SharedString::new());
    st.set_read_pct(0);
    st.set_hunt_frame(Image::default());
}

/// Clears every secret-bearing UI property. Slint's own image copy is freed, not zeroized (accepted).
fn clear_secrets(st: &AppState) {
    clear_game_models(st);
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
    st.set_rolls_text(SharedString::new());
    st.set_roll_count(0);
    st.set_dice_bits(0.0);
    st.set_dice_run(false);
}

/// Dice counters plus the masked rolls ("•••• 1 5": only the last two rolls show).
fn sync_dice(st: &AppState, s: &Session) {
    use std::fmt::Write;
    let r = s.rolls();
    // Maximum 16 UTF-8 bytes (four bullets, spaces, two digits); no temporary roll strings.
    let mut t = Zeroizing::new(String::with_capacity(16));
    if r.len() > 2 { t.push_str("•••• "); }
    for (i, roll) in r[r.len().saturating_sub(2)..].iter().enumerate() {
        if i > 0 { t.push(' '); }
        write!(t, "{roll}").unwrap();
    }
    st.set_rolls_text(SharedString::from(t.as_str()));
    st.set_roll_count(r.len() as i32);
    st.set_rolls_needed(s.rolls_needed() as i32);
    st.set_dice_bits(r.len() as f32 * 2.585);
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

/// Redraws the Shard Stack board, the falling piece, its landing outline, the next piece and the stats.
fn sync_shard(st: &AppState, s: &Session, g: &shard::Shard) {
    let model = |v: Vec<i32>| ModelRc::from(Rc::new(VecModel::from(v)));
    st.set_shard_cells(model(s.shard_cells().iter().map(|&v| v as i32).collect()));
    st.set_ghost_cells(model(g.cells().iter().map(|&v| v as i32).collect()));
    let preview = s.shard_preview(g.rot, g.col);
    st.set_landing_cells(model(preview.iter().map(|&v| v as i32).collect()));
    let mut next = vec![0i32; 16];
    for (x, y) in entropy_core::shard::shape(g.next, 0) { next[y as usize * 4 + x as usize] = 1; }
    st.set_next_cells(model(next));
    st.set_shard_lines(g.lines as i32);
    st.set_shard_score((g.lines * g.lines * 100) as i32);
    st.set_shard_level((g.lines / 5 + 1) as i32);
    let (n, _, pct) = s.shard_progress();
    st.set_shard_solver(SharedString::from(if n > 0 { format!("{pct}%") } else { "-".into() }.as_str()));
}

/// Locks the piece at its current (rot, col, row): the core ranks and credits the placement, then the next piece spawns.
fn lock_piece(ui: &AppWindow, s: &RefCell<Session>, sh: &RefCell<shard::Shard>, f: &RefCell<Feedback>, start: Instant) {
    let st = ui.global::<AppState>();
    let t_us = start.elapsed().as_micros() as u64;
    let (rot, col, row) = { let g = sh.borrow(); (g.rot, g.col, g.row) };
    debug_assert_eq!(s.borrow().shard_land_row(rot, col), Some(row));
    let filled_before = s.borrow().shard_cells().iter().filter(|&&v| v == 1).count();
    let piece_size = entropy_core::shard::shape(sh.borrow().piece, rot).len();
    let placed = s.borrow_mut().shard_place(rot, col, t_us);
    let filled_after = s.borrow().shard_cells().iter().filter(|&&v| v == 1).count();
    let lines = (filled_before + piece_size).saturating_sub(filled_after) / 8;
    sh.borrow_mut().lines += lines as u32;
    st.set_shard_placements(st.get_shard_placements().saturating_add(1));
    let (scored, warm, _) = s.borrow().shard_progress();
    let mut note = match placed {
        Some(_) if scored < warm => format!("Learning your style {scored}/{warm}"),
        Some(e) if !e.healthy => "Too predictable: surprise the solver".to_string(),
        Some(e) => format!("+{:.1} bits", e.credit_mbits as f32 / 1000.0),
        None => String::new(),
    };
    sh.borrow_mut().next_piece();
    s.borrow_mut().shard_spawn(sh.borrow().piece);
    sh.borrow_mut().settle_spawn(&|r, c, w| s.borrow().shard_fits(r, c, w));
    if filled_after > 0 && s.borrow().shard_cells().iter().all(|&v| v == 0) {
        note = "Board full, cleared, bits kept".into();
    }
    st.set_shard_note(SharedString::from(note.as_str()));
    sync_shard(&st, &s.borrow(), &sh.borrow());
    credit(ui, &s.borrow(), f);
}

/// 50 ms gravity timer for Shard Stack (shares the Static Hunt timer slot and its stop points).
fn start_gravity(timer: &slint::Timer, ui: &slint::Weak<AppWindow>, s: &Rc<RefCell<Session>>, sh: &Rc<RefCell<shard::Shard>>, f: &Rc<RefCell<Feedback>>, start: Instant) {
    let (ui, s, sh, f) = (ui.clone(), s.clone(), sh.clone(), f.clone());
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
        let Some(ui) = ui.upgrade() else { return };
        if ui.global::<AppState>().get_screen() != Screen::Shard { return; }
        {
            let mut g = sh.borrow_mut();
            g.acc += 50;
            if g.acc < g.interval_ms() { return; }
            g.acc = 0;
            let (r, c, w) = (g.rot, g.col, g.row);
            if s.borrow().shard_fits(r, c, w + 1) {
                g.row += 1;
                drop(g);
                sync_shard(&ui.global::<AppState>(), &s.borrow(), &sh.borrow());
                return;
            }
        }
        lock_piece(&ui, &s, &sh, &f, start);
    });
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

#[derive(Default)]
struct Gesture {
    start: (f32, f32),
    last: (f32, f32),
    drag: bool,
}

/// Syncs the meter and fires haptics after a credited sample.
fn credit(ui: &AppWindow, s: &Session, f: &RefCell<Feedback>) {
    sync_meter(ui, s);
    let events = f.borrow_mut().update(s, ui.global::<AppState>().get_camera_on());
    if events.alert { haptics::alert(); }
    match events.pulse {
        Some(Pulse::Tick) => haptics::tick(),
        Some(Pulse::Ramp) => haptics::ramp(),
        Some(Pulse::Full) => haptics::full(),
        None => {},
    }
}

/// 33 ms frame timer for Static Hunt; the caller stops it on every exit from the screen.
fn start_frames(timer: &slint::Timer, ui: &slint::Weak<AppWindow>, hunt: &Rc<RefCell<hunt::Hunt>>, pop: &Rc<Cell<u32>>) {
    let (ui, hunt, pop) = (ui.clone(), hunt.clone(), pop.clone());
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(hunt::W, hunt::H);
    let (mut frames, mut total) = (0u32, Duration::ZERO);
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(FRAME_MS), move || {
        let t0 = Instant::now();
        {
            let mut h = hunt.borrow_mut();
            h.tick(FRAME_MS as u32);
            h.render(&mut buf);
        }
        let Some(ui) = ui.upgrade() else { return };
        let st = ui.global::<AppState>();
        st.set_hunt_frame(Image::from_rgb8(buf.clone()));
        if pop.get() > 0 {
            pop.set(pop.get().saturating_sub(FRAME_MS as u32));
            if pop.get() == 0 {
                st.set_hunt_pop(SharedString::new());
            }
        }
        total += t0.elapsed();
        frames += 1;
        if frames == 100 {
            log::info!("screen hunt frame_ms={}", total.as_secs_f32() * 10.0);
            (frames, total) = (0, Duration::ZERO);
        }
    });
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

    let hunt = Rc::new(RefCell::new(hunt::Hunt::new(1)));
    let gesture = Rc::new(RefCell::new(Gesture::default()));
    let hunt_timer = Rc::new(slint::Timer::default());
    let shard = Rc::new(RefCell::new(shard::Shard::new(1)));
    let pop = Rc::new(Cell::new(0u32)); // ms left on the "+1 bit" pop
    let duel = duel::Controller::new(&ui, session.clone(), fb.clone(), start);

    let (u, s, t, f) = (ui.as_weak(), session.clone(), trail.clone(), fb.clone());
    let (h, g, ht, pop2) = (hunt.clone(), gesture.clone(), hunt_timer.clone(), pop.clone());
    let sh = shard.clone();
    let d = duel.clone();
    st.on_start_arcade(move |n, game, players| {
        d.stop();
        *f.borrow_mut() = Feedback::new();
        let ui = u.upgrade().unwrap();
        camera_off(&ui.global::<AppState>());
        let wc = if n == 24 { WordCount::TwentyFour } else { WordCount::Twelve };
        s.borrow_mut().start_arcade(wc);
        t.set_vec(Vec::new());
        sync_meter(&ui, &s.borrow());
        let st = ui.global::<AppState>();
        clear_secrets(&st);
        ht.stop();
        st.set_screen(match game {
            1 if players == 2 => { d.start(); Screen::Duel }
            1 => Screen::Outguess,
            2 => {
                let seed = hw_random().map_or(0x9e37_79b9 ^ start.elapsed().as_nanos() as u32, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                *h.borrow_mut() = hunt::Hunt::new(seed);
                *g.borrow_mut() = Gesture::default();
                start_frames(&ht, &u, &h, &pop2);
                Screen::Hunt
            }
            4 => {
                let seed = hw_random().map_or(0x9e37_79b9 ^ start.elapsed().as_nanos() as u32, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                *sh.borrow_mut() = shard::Shard::new(seed);
                s.borrow_mut().shard_spawn(sh.borrow().piece);
                st.set_shard_note(SharedString::from("Learning your style 0/16"));
                sync_shard(&st, &s.borrow(), &sh.borrow());
                start_gravity(&ht, &u, &s, &sh, &f, start);
                Screen::Shard
            }
            _ => Screen::Painter,
        });
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
        st.set_last_pick(door);
        st.set_outguess_round(st.get_outguess_round().saturating_add(1));
        st.set_last_hit(m.hit);
        st.set_last_credit(SharedString::from(format!("+{:.1} bits", m.credit_mbits as f32 / 1000.0).as_str()));
        st.set_read_pct(m.read_pct as i32);
        credit(&ui, &s.borrow(), &f);
    });

    let (u, s, sh) = (ui.as_weak(), session.clone(), shard.clone());
    st.on_shard_move(move |d| {
        s.borrow_mut().shard_move(0, d, start.elapsed().as_micros() as u64);
        sh.borrow_mut().shift(d, &|r, c, w| s.borrow().shard_fits(r, c, w));
        let ui = u.upgrade().unwrap();
        sync_shard(&ui.global::<AppState>(), &s.borrow(), &sh.borrow());
    });
    let (u, s, sh) = (ui.as_weak(), session.clone(), shard.clone());
    st.on_shard_rotate(move || {
        s.borrow_mut().shard_move(1, 0, start.elapsed().as_micros() as u64);
        sh.borrow_mut().rotate(&|r, c, w| s.borrow().shard_fits(r, c, w));
        let ui = u.upgrade().unwrap();
        sync_shard(&ui.global::<AppState>(), &s.borrow(), &sh.borrow());
    });
    // Soft drop: one row per swipe step, never locks (gravity does).
    let (u, s, sh) = (ui.as_weak(), session.clone(), shard.clone());
    st.on_shard_soft(move |n| {
        for _ in 0..n {
            let (r, c, w) = { let g = sh.borrow(); (g.rot, g.col, g.row) };
            if !s.borrow().shard_fits(r, c, w + 1) { break; }
            sh.borrow_mut().row += 1;
        }
        sh.borrow_mut().acc = 0;
        let ui = u.upgrade().unwrap();
        sync_shard(&ui.global::<AppState>(), &s.borrow(), &sh.borrow());
    });
    // Hard drop: fall to the core's landing row, then lock.
    let (u, s, f, sh) = (ui.as_weak(), session.clone(), fb.clone(), shard.clone());
    st.on_shard_drop(move || {
        let ui = u.upgrade().unwrap();
        let (rot, col) = { let g = sh.borrow(); (g.rot, g.col) };
        let Some(row) = s.borrow().shard_land_row(rot, col) else { return };
        sh.borrow_mut().row = row;
        lock_piece(&ui, &s, &sh, &f, start);
    });

    let (u, s) = (ui.as_weak(), session.clone());
    let d = duel.clone();
    st.on_start_dice(move |n| {
        d.stop();
        let wc = if n == 24 { WordCount::TwentyFour } else { WordCount::Twelve };
        s.borrow_mut().start_dice(wc);
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        clear_secrets(&st);
        sync_dice(&st, &s.borrow());
        st.set_dice_run(true);
        st.set_dice_words(if n == 24 { 24 } else { 12 });
        st.set_screen(Screen::Dice);
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_roll(move |r| {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        if (1..=6).contains(&r) && s.borrow_mut().push_roll(r as u8) {
            sync_dice(&st, &s.borrow());
            st.set_dice_note(SharedString::from("Roll entered"));
        }
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_undo_roll(move || {
        let had_roll = !s.borrow().rolls().is_empty();
        s.borrow_mut().pop_roll();
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        sync_dice(&st, &s.borrow());
        st.set_dice_note(SharedString::from(if had_roll { "Last roll removed" } else { "No rolls to undo" }));
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_dice_done(move || {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        let r = s.borrow_mut().generate_from_dice();
        sync_dice(&st, &s.borrow()); // rolls are cleared on success, so the masked text empties
        match r {
            Ok(()) => show_words(&st, &s.borrow()),
            Err(e) => {
                st.set_error_text(SharedString::from(e.as_str()));
                st.set_screen(Screen::Error);
            }
        }
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
            credit(&ui, &s.borrow(), &f);
        }
    });

    let (u, s, f) = (ui.as_weak(), session.clone(), fb.clone());
    st.on_toggle_camera(move || {
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        st.set_camera_note(SharedString::new());
        if st.get_camera_on() {
            camera_off(&st);
        } else if camera::start(u.clone(), s.clone(), start, f.clone()) {
            st.set_camera_on(true);
        } else {
            st.set_camera_note(SharedString::from("Camera unavailable"));
        }
        credit(&ui, &s.borrow(), &f);
    });

    let g = gesture.clone();
    st.on_hunt_press(move |x, y| *g.borrow_mut() = Gesture { start: (x, y), last: (x, y), drag: false });

    let (u, s, f, h, g) = (ui.as_weak(), session.clone(), fb.clone(), hunt.clone(), gesture.clone());
    st.on_hunt_move(move |x, y| {
        let ui = u.upgrade().unwrap();
        let (x, y) = (x, y);
        let mut gs = g.borrow_mut();
        if !gs.drag && (x - gs.start.0).hypot(y - gs.start.1) > DRAG_PX {
            gs.drag = true;
        }
        if !gs.drag {
            return;
        }
        h.borrow_mut().turn((x - gs.last.0) as i32);
        gs.last = (x, y);
        let t_us = start.elapsed().as_micros() as u64;
        let r = s.borrow_mut().on_touch(x as i32, y as i32, t_us);
        if r != SampleResult::Uncredited {
            credit(&ui, &s.borrow(), &f);
        }
    });

    let (u, s, f, h, g, pop) = (ui.as_weak(), session.clone(), fb.clone(), hunt.clone(), gesture.clone(), pop.clone());
    st.on_hunt_release(move |x, y| {
        let ui = u.upgrade().unwrap();
        let (x, y) = (x, y);
        let mut gs = g.borrow_mut();
        if gs.drag || (x - gs.start.0).hypot(y - gs.start.1) > DRAG_PX {
            gs.drag = false;
            return;
        }
        // View is 240x288 shown at 2x; the area is centred, so x is already relative to the image.
        let shot = h.borrow_mut().tap((x / 2.0) as i32, (y / 2.0) as i32);
        let st = ui.global::<AppState>();
        let Some((dx, dy)) = shot else {
            st.set_hunt_note(SharedString::from("No ghost in view. Drag to look around."));
            return;
        };
        let t_us = start.elapsed().as_micros() as u64;
        let r = s.borrow_mut().on_shot((x / 2.0) as i32, (y / 2.0) as i32, dx, dy, t_us);
        let hit = h.borrow().last_hit;
        let note = match (hit, r) {
            (true, SampleResult::Credited(_)) => "Ghost hit / +1 bit",
            (true, _) => "Ghost hit / +0 bits. Vary your aim.",
            (false, SampleResult::Credited(_)) => "Miss / +1 bit from varied aim",
            (false, _) => "Miss / +0 bits. Keep aiming.",
        };
        st.set_hunt_note(SharedString::from(note));
        if r != SampleResult::Uncredited {
            credit(&ui, &s.borrow(), &f);
        }
        if matches!(r, SampleResult::Credited(_)) {
            ui.global::<AppState>().set_hunt_pop(SharedString::from("+1 bit"));
            pop.set(POP_MS);
        }
    });

    let (u, s, ht, t) = (ui.as_weak(), session.clone(), hunt_timer.clone(), trail.clone());
    let d = duel.clone();
    st.on_painter_done(move || {
        let ui = u.upgrade().unwrap();
        if s.borrow().pool().is_some_and(|p| p.is_full()) {
            ht.stop();
            let st = ui.global::<AppState>();
            camera_off(&st);
            t.set_vec(Vec::new());
            let was_duel = st.get_screen() == Screen::Duel;
            d.stop();
            clear_game_models(&st);
            st.set_screen(if was_duel { Screen::Duelowner } else { Screen::Summary });
        }
    });

    let (u, s) = (ui.as_weak(), session.clone());
    st.on_duel_owner_ready(move || {
        let Some(ui) = u.upgrade() else { return };
        let st = ui.global::<AppState>();
        if st.get_screen() == Screen::Duelowner && s.borrow().pool().is_some_and(|p| p.is_full()) {
            st.set_screen(Screen::Summary);
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
                st.set_page_words(ModelRc::from(Rc::new(VecModel::<SharedString>::default())));
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
    let (u, s, t, ht, g, sh, hu) = (ui.as_weak(), session.clone(), trail.clone(), hunt_timer.clone(), gesture.clone(), shard.clone(), hunt.clone());
    let d = duel.clone();
    let leave = Rc::new(move || {
        d.stop();
        ht.stop();
        *sh.borrow_mut() = shard::Shard::new(1); // drops piece/rot/col/rng
        *g.borrow_mut() = Gesture::default();
        *hu.borrow_mut() = hunt::Hunt::new(1);
        s.borrow_mut().wipe();
        t.set_vec(Vec::new());
        let ui = u.upgrade().unwrap();
        let st = ui.global::<AppState>();
        camera_off(&st);
        st.set_camera_note(SharedString::new());
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
    duel.stop();
}
