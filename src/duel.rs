//! Duel interaction: secret locks live in Rust, revealed choices alone enter Slint.
//! No logging, persistence, or hardware access.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use entropy_core::{feedback::Feedback, DuelLock, Session};
use slint_keyos_platform::slint::{self, ComponentHandle, SharedString};

use crate::{AppState, AppWindow, Screen};

const REVEAL_US: u64 = 1_500_000;

pub fn clear_ui(st: &AppState) {
    st.set_duel_round(1);
    st.set_duel_moves(0);
    st.set_duel_p1_warm(0);
    st.set_duel_p2_warm(0);
    st.set_duel_hider(0);
    st.set_duel_p1_score(0);
    st.set_duel_p2_score(0);
    st.set_duel_p1_locked(false);
    st.set_duel_p2_locked(false);
    st.set_duel_p1_healthy(true);
    st.set_duel_p2_healthy(true);
    st.set_duel_countdown(0);
    st.set_duel_revealed(false);
    st.set_duel_p1_door(-1);
    st.set_duel_p2_door(-1);
    st.set_duel_hit(false);
    st.set_duel_winner(-1);
    st.set_duel_note(SharedString::new());
}

pub struct Controller {
    ui: slint::Weak<AppWindow>,
    session: Rc<RefCell<Session>>,
    feedback: Rc<RefCell<Feedback>>,
    clock: Instant,
    timer: slint::Timer,
    lock: RefCell<DuelLock>,
    reveal_until: Cell<u64>,
}

impl Controller {
    pub fn new(
        ui: &AppWindow,
        session: Rc<RefCell<Session>>,
        feedback: Rc<RefCell<Feedback>>,
        clock: Instant,
    ) -> Rc<Self> {
        let controller = Rc::new(Self {
            ui: ui.as_weak(), session, feedback, clock,
            timer: slint::Timer::default(),
            lock: RefCell::new(DuelLock::default()), reveal_until: Cell::new(0),
        });
        let weak = Rc::downgrade(&controller);
        ui.global::<AppState>().on_duel_pick(move |player, door, x, y| {
            if let Some(c) = weak.upgrade() { c.pick(player, door, x, y); }
        });
        let weak = Rc::downgrade(&controller);
        ui.global::<AppState>().on_duel_next_match(move || {
            if let Some(c) = weak.upgrade() { c.next_match(); }
        });
        controller
    }

    pub fn start(self: &Rc<Self>) {
        self.stop();
        let Some(ui) = self.ui.upgrade() else { return };
        let st = ui.global::<AppState>();
        clear_ui(&st);
        self.ready(&st);
        let weak = Rc::downgrade(self);
        self.timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
            if let Some(c) = weak.upgrade() { c.tick(); }
        });
    }

    pub fn stop(&self) {
        self.timer.stop();
        self.lock.borrow_mut().wipe();
        self.reveal_until.set(0);
    }

    fn now(&self) -> u64 { self.clock.elapsed().as_micros() as u64 }

    fn sync_locked(&self, st: &AppState, now: u64) {
        let lock = self.lock.borrow();
        st.set_duel_p1_locked(lock.locked(0));
        st.set_duel_p2_locked(lock.locked(1));
        st.set_duel_countdown(lock.seconds_left(now) as i32);
    }

    fn ready(&self, st: &AppState) {
        let s = self.session.borrow();
        let duel = s.duel();
        let scores = duel.scores();
        st.set_duel_round(duel.round_number().saturating_add(1) as i32);
        st.set_duel_moves(duel.moves().min(i32::MAX as usize) as i32);
        st.set_duel_p1_warm(duel.warmup(0) as i32);
        st.set_duel_p2_warm(duel.warmup(1) as i32);
        st.set_duel_hider(duel.hider() as i32);
        st.set_duel_p1_score(scores[0] as i32);
        st.set_duel_p2_score(scores[1] as i32);
        st.set_duel_winner(duel.winner().map_or(-1, |p| p as i32));
        st.set_duel_revealed(false);
        st.set_duel_p1_door(-1);
        st.set_duel_p2_door(-1);
        st.set_duel_hit(false);
        st.set_duel_p1_locked(false);
        st.set_duel_p2_locked(false);
        st.set_duel_countdown(0);
        let note = if duel.moves() < entropy_core::outguess::WARMUP {
            format!("Learning both players · {} / 16 choices", duel.moves())
        } else {
            "First to 10 · Roles swap every 5 rounds".to_string()
        };
        st.set_duel_note(SharedString::from(note.as_str()));
    }

    fn pick(&self, player: i32, door: i32, x: f32, y: f32) {
        let Some(ui) = self.ui.upgrade() else { return };
        let st = ui.global::<AppState>();
        if st.get_screen() != Screen::Duel || st.get_duel_revealed()
            || st.get_duel_winner() >= 0 || !(0..2).contains(&player) || !(0..4).contains(&door)
        {
            return;
        }
        let now = self.now();
        let accepted = self.lock.borrow_mut().pick(
            player as usize, door as u8, (x as i32, y as i32, now), now,
        );
        self.sync_locked(&st, now);
        if !accepted { return; }
        let choices = self.lock.borrow_mut().take();
        let Some(choices) = choices else { return };
        let hider = self.session.borrow().duel().hider() as usize;
        let result = self.session.borrow_mut().duel_round(
            choices.doors[hider], choices.doors[1 - hider],
            choices.taps[hider], choices.taps[1 - hider],
        );

        // Publication happens only after the complete pair has been committed.
        st.set_duel_p1_door(choices.doors[0] as i32);
        st.set_duel_p2_door(choices.doors[1] as i32);
        st.set_duel_p1_locked(false);
        st.set_duel_p2_locked(false);
        st.set_duel_countdown(0);
        st.set_duel_p1_score(result.scores[0] as i32);
        st.set_duel_p2_score(result.scores[1] as i32);
        st.set_duel_p1_healthy(result.healthy[0]);
        st.set_duel_p2_healthy(result.healthy[1]);
        st.set_duel_winner(result.winner.map_or(-1, |p| p as i32));
        st.set_duel_hit(result.seeker_hit);
        st.set_duel_revealed(true);
        self.reveal_until.set(now.saturating_add(REVEAL_US));
        let s = self.session.borrow();
        st.set_duel_moves(s.duel().moves().min(i32::MAX as usize) as i32);
        st.set_duel_p1_warm(s.duel().warmup(0) as i32);
        st.set_duel_p2_warm(s.duel().warmup(1) as i32);
        let note = if let Some(winner) = result.winner {
            format!("Player {} wins · Again keeps your entropy", winner + 1)
        } else if result.roles_swapped {
            "Roles swap next round · Watch your new role".to_string()
        } else {
            format!("P1 +{:.1} bits · P2 +{:.1} bits",
                result.credit_mbits[0] as f32 / 1000.0,
                result.credit_mbits[1] as f32 / 1000.0)
        };
        st.set_duel_note(SharedString::from(note.as_str()));
        crate::credit(&ui, &s, &self.feedback);
    }

    fn tick(&self) {
        let Some(ui) = self.ui.upgrade() else { self.stop(); return };
        let st = ui.global::<AppState>();
        if st.get_screen() != Screen::Duel { self.stop(); return; }
        let now = self.now();
        if st.get_duel_revealed() {
            if now >= self.reveal_until.get() && self.session.borrow().duel().winner().is_none() {
                self.ready(&st);
            }
            return;
        }
        let expired = self.lock.borrow_mut().expire(now);
        self.sync_locked(&st, now);
        if expired {
            st.set_duel_note(SharedString::from("Time out · No round scored. Try again."));
        }
    }

    fn next_match(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let st = ui.global::<AppState>();
        if st.get_screen() != Screen::Duel || !self.session.borrow_mut().duel_next_match() {
            return;
        }
        self.lock.borrow_mut().wipe();
        self.reveal_until.set(0);
        self.ready(&st);
    }
}
