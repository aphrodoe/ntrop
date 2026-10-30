//! Haptic decisions from session counters and independent source health; no device API.

use crate::Session;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pulse { Tick, Ramp, Full }

pub struct FeedbackEvents {
    pub alert: bool,
    pub pulse: Option<Pulse>,
}

pub struct Feedback {
    ticks: u32,
    ramped: bool,
    full: bool,
    healthy: [bool; 3],
}

impl Feedback {
    pub fn new() -> Self {
        Self { ticks: 0, ramped: false, full: false, healthy: [true; 3] }
    }

    /// Every input path uses the same three health flags. An inactive camera cannot alert.
    pub fn update(&mut self, session: &Session, camera_on: bool) -> FeedbackEvents {
        let Some(pool) = session.pool() else {
            return FeedbackEvents { alert: false, pulse: None };
        };
        self.update_state(pool.credited_bits(), session.target_bits(), [
            pool.touch_healthy(), !camera_on || pool.camera_healthy(), pool.game_healthy(),
        ])
    }

    fn update_state(&mut self, credited: u32, target: u32, healthy: [bool; 3]) -> FeedbackEvents {
        let alert = self.healthy.iter().zip(healthy).any(|(&was, now)| was && !now);
        self.healthy = healthy;
        let full = credited >= target;
        let near_full = credited as u64 * 10 >= target as u64 * 9;
        // Prefer one milestone pulse per update; ramp and full supersede a tick.
        let pulse = if full && !self.full {
            Some(Pulse::Full)
        } else if !full && near_full && !self.ramped {
            Some(Pulse::Ramp)
        } else if !full && credited / 8 > self.ticks {
            Some(Pulse::Tick)
        } else {
            None
        };
        self.ticks = credited / 8;
        self.ramped |= near_full;
        self.full |= full;
        FeedbackEvents { alert, pulse }
    }
}

impl Default for Feedback {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, WordCount};

    #[test]
    fn frozen_camera_alerts_once_between_healthy_touch_samples() {
        let mut s = Session::new();
        s.start_arcade(WordCount::TwentyFour);
        let mut f = Feedback::new();
        let frame = [0x42; 4096];
        let mut alerts = 0;
        for i in 0..20i32 {
            s.on_touch(i * i, i * i * i, i as u64 * 10_000);
            alerts += f.update(&s, true).alert as usize;
            s.on_camera_frame(&frame, i as u64 * 10_000 + 1);
            alerts += f.update(&s, true).alert as usize;
        }
        assert!(!s.pool().unwrap().camera_healthy());
        assert!(s.pool().unwrap().touch_healthy());
        assert_eq!(alerts, 1);
        assert!(!f.update(&s, false).alert, "turning the camera off clears its health state");
        assert!(f.update(&s, true).alert, "a new camera activation can report its failure");
        assert!(!f.update(&s, true).alert);
    }

    #[test]
    fn each_source_alerts_on_its_own_falling_edge() {
        let mut f = Feedback::new();
        assert!(f.update_state(0, 128, [true, false, true]).alert);
        assert!(f.update_state(0, 128, [false, false, true]).alert);
        assert!(f.update_state(0, 128, [false, false, false]).alert);
        assert!(!f.update_state(0, 128, [false, false, false]).alert);
        assert!(!f.update_state(0, 128, [true, true, true]).alert);
        assert!(f.update_state(0, 128, [false, true, true]).alert);
    }

    #[test]
    fn milestone_pulses_do_not_overlap_or_repeat() {
        let mut f = Feedback::new();
        let ok = [true; 3];
        assert_eq!(f.update_state(7, 128, ok).pulse, None);
        assert_eq!(f.update_state(8, 128, ok).pulse, Some(Pulse::Tick));
        assert_eq!(f.update_state(8, 128, ok).pulse, None);
        assert_eq!(f.update_state(120, 128, ok).pulse, Some(Pulse::Ramp));
        assert_eq!(f.update_state(120, 128, ok).pulse, None);
        assert_eq!(f.update_state(128, 128, ok).pulse, Some(Pulse::Full));
        assert_eq!(f.update_state(128, 128, ok).pulse, None);
        assert_eq!(Feedback::new().update_state(128, 128, ok).pulse, Some(Pulse::Full));
    }
}
