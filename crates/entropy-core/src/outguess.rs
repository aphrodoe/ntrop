//! Outguess the Machine: a Shannon-style predictor reads the player's door history.
//! Credit is earned only while the machine guesses worse than a conservative bound.
//! The history is secret entropy material: never logged, zeroized on wipe.

use zeroize::Zeroize;

pub const DOORS: u8 = 4;
pub const WINDOW: usize = 32;
pub const WARMUP: usize = 16;
pub const MARGIN_HITS: u32 = 4;
pub const MAX_MBITS: u32 = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub predicted: u8,
    pub hit: bool,
    pub credit_mbits: u32,
    pub read_pct: u8,
    pub healthy: bool,
}

/// Three experts, each predicting from history alone: E1 doors, E2 offset from the previous
/// revealed guess, E3 win/lose state. The shown guess is the recently best expert.
#[derive(Default, Clone, Zeroize)]
pub struct Oracle {
    doors: Vec<u8>,
    /// The guess shown (committed) for each move.
    shown: Vec<u8>,
    /// Per expert: 1 where its own prediction hit that move.
    ehits: [Vec<u8>; 3],
}

/// Context counting over `seq`: longest context in `ks` seen before wins; ties to the lowest value.
fn ctx_pred(seq: &[u8], ks: &[usize]) -> Option<u8> {
    for &k in ks {
        if seq.len() > k {
            let ctx = &seq[seq.len() - k..];
            let mut counts = [0u32; DOORS as usize];
            for i in 0..seq.len() - k {
                if &seq[i..i + k] == ctx {
                    counts[seq[i + k] as usize] += 1;
                }
            }
            if counts.iter().any(|&c| c > 0) {
                return Some(argmax(&counts));
            }
        }
    }
    None
}

fn window_hits(h: &[u8]) -> u32 {
    h[h.len().saturating_sub(WINDOW)..].iter().map(|&b| b as u32).sum()
}

impl Oracle {
    fn experts(&self) -> [u8; 3] {
        let (d, s) = (&self.doors, &self.shown);
        let n = d.len();
        let e1 = ctx_pred(d, &[3, 2, 1, 0]).unwrap_or(0);
        let e2 = if n >= 1 {
            let r: Vec<u8> = (1..n).map(|t| (d[t] + 4 - s[t - 1]) % 4).collect();
            ctx_pred(&r, &[2, 1, 0]).map_or(e1, |r| (s[n - 1] + r) % 4)
        } else {
            e1
        };
        let e3 = if n >= 2 {
            let delta = |t: usize| (d[t] + 4 - d[t - 1]) % 4;
            let state = |t: usize| (s[t] == d[t]) as u8 * 4 + delta(t);
            let cur = state(n - 1);
            let mut counts = [0u32; DOORS as usize];
            for t in 1..n - 1 {
                if state(t) == cur {
                    counts[delta(t + 1) as usize] += 1;
                }
            }
            if counts.iter().any(|&c| c > 0) { (d[n - 1] + argmax(&counts)) % 4 } else { e1 }
        } else {
            e1
        };
        [e1, e2, e3]
    }

    /// Index of the expert with the most window hits (ties: E1, E2, E3).
    fn best(&self) -> usize {
        let h = [0, 1, 2].map(|i| window_hits(&self.ehits[i]));
        let mut b = 0;
        for i in 1..3 {
            if h[i] > h[b] {
                b = i;
            }
        }
        b
    }

    /// Pure function of the history.
    pub fn predict(&self) -> u8 {
        self.experts()[self.best()]
    }

    /// The prediction is taken before the door is pushed, so it is committed by construction.
    pub fn record(&mut self, door: u8) -> Move {
        let door = door % DOORS;
        let e = self.experts();
        let predicted = e[self.best()];
        let hit = predicted == door;
        self.doors.push(door);
        self.shown.push(predicted);
        for i in 0..3 {
            self.ehits[i].push((e[i] == door) as u8);
        }
        let moves = self.doors.len();
        let n = moves.min(WINDOW);
        // Best expert in hindsight, not just the shown guess.
        let hits = (0..3).map(|i| window_hits(&self.ehits[i])).max().unwrap();
        let read_pct = (100.0 * hits as f64 / n as f64).round() as u8;
        let (credit_mbits, healthy) = credit_bound(moves, hits);
        Move { predicted, hit, credit_mbits, read_pct, healthy }
    }

    pub fn wipe(&mut self) {
        self.zeroize();
    }
}


fn credit_bound(moves: usize, hits: u32) -> (u32, bool) {
    if moves <= WARMUP {
        return (0, true);
    }
    let n = moves.min(WINDOW) as f64;
    let p_ub = ((hits + MARGIN_HITS) as f64 / n).min(1.0);
    if p_ub > 0.5 {
        (0, false)
    } else {
        (((-p_ub.log2() * 1000.0).floor() as u32).min(MAX_MBITS), true)
    }
}

pub const DUEL_SWAP_ROUNDS: u32 = 5;
pub const DUEL_WIN_SCORE: u8 = 10;
pub const DUEL_LOCK_US: u64 = 3_000_000;
pub type DuelTap = (i32, i32, u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuelRound {
    pub seeker_hit: bool,
    pub credit_mbits: [u32; 2],
    pub healthy: [bool; 2],
    pub scores: [u8; 2],
    pub roles_swapped: bool,
    pub winner: Option<u8>,
}

/// Player 0 is the device owner. Predictors belong to players, regardless of role.
/// Matches share learning and credit history; only wipe starts a new entropy run.
#[derive(Default, Zeroize, zeroize::ZeroizeOnDrop)]
pub struct Duel {
    oracles: [Oracle; 2],
    // A matching pair is an opponent read in either direction. The same window
    // bounds both streams so copying the other player cannot earn twice.
    reads: [u8; WINDOW],
    read_cursor: usize,
    read_count: usize,
    hider: u8,
    scores: [u8; 2],
    round_number: u32,
    winner: Option<u8>,
}

impl Duel {
    pub fn moves(&self) -> usize { self.oracles[0].doors.len() }
    pub fn hider(&self) -> u8 { self.hider }
    pub fn scores(&self) -> [u8; 2] { self.scores }
    pub fn round_number(&self) -> u32 { self.round_number }
    pub fn winner(&self) -> Option<u8> { self.winner }

    /// Inputs are already locked by both players. Taps are mixed by Session,
    /// never credited separately from the door decision.
    pub fn round(
        &mut self,
        hider_door: u8,
        seeker_guess: u8,
        _hider_tap: DuelTap,
        _seeker_tap: DuelTap,
    ) -> DuelRound {
        if self.winner.is_some() {
            return DuelRound {
                seeker_hit: false, credit_mbits: [0; 2], healthy: [false; 2],
                scores: self.scores, roles_swapped: false, winner: self.winner,
            };
        }
        let hider = self.hider as usize;
        let seeker = 1 - hider;
        let mut doors = [0; 2];
        doors[hider] = hider_door % DOORS;
        doors[seeker] = seeker_guess % DOORS;
        let seeker_hit = doors[hider] == doors[seeker];
        self.reads[self.read_cursor] = seeker_hit as u8;
        self.read_cursor = (self.read_cursor + 1) % WINDOW;
        self.read_count = (self.read_count + 1).min(WINDOW);
        let opponent_hits = window_hits(&self.reads);

        let mut credit_mbits = [0; 2];
        let mut healthy = [true; 2];
        for player in 0..2 {
            let oracle = &mut self.oracles[player];
            oracle.record(doors[player]);
            let machine_hits = oracle.ehits.iter().map(|h| window_hits(h)).max().unwrap_or(0);
            (credit_mbits[player], healthy[player]) =
                credit_bound(oracle.doors.len(), machine_hits.max(opponent_hits));
        }
        doors.zeroize();
        let scorer = if seeker_hit { seeker } else { hider };
        self.scores[scorer] += 1;
        self.round_number += 1;
        if self.scores[scorer] == DUEL_WIN_SCORE {
            self.winner = Some(scorer as u8);
        }
        let roles_swapped = self.round_number % DUEL_SWAP_ROUNDS == 0;
        if roles_swapped { self.hider ^= 1; }
        DuelRound {
            seeker_hit, credit_mbits, healthy, scores: self.scores,
            roles_swapped, winner: self.winner,
        }
    }

    /// Explicit replay after first-to-ten. Never resets the predictors or meter.
    pub fn next_match(&mut self) -> bool {
        if self.winner.is_none() { return false; }
        self.scores = [0; 2];
        self.round_number = 0;
        self.hider = 0;
        self.winner = None;
        true
    }

    pub fn wipe(&mut self) { self.zeroize(); }
}

#[derive(Clone, Copy, Zeroize)]
struct LockedPick {
    door: u8,
    tap: DuelTap,
}

/// Secret choices leave the lock only as a complete pair.
#[derive(Zeroize, zeroize::ZeroizeOnDrop)]
pub struct DuelChoices {
    pub doors: [u8; 2],
    pub taps: [DuelTap; 2],
}

/// One immutable choice per player; an incomplete lock is wiped at its deadline.
#[derive(Default, Zeroize, zeroize::ZeroizeOnDrop)]
pub struct DuelLock {
    picks: [Option<LockedPick>; 2],
    deadline_us: Option<u64>,
}

impl DuelLock {
    pub fn locked(&self, player: usize) -> bool {
        self.picks.get(player).is_some_and(Option::is_some)
    }

    pub fn pick(&mut self, player: usize, door: u8, tap: DuelTap, now_us: u64) -> bool {
        if self.expire(now_us) || player >= 2 || door >= DOORS || self.locked(player) {
            return false;
        }
        self.deadline_us.get_or_insert(now_us.saturating_add(DUEL_LOCK_US));
        self.picks[player] = Some(LockedPick { door, tap });
        true
    }

    pub fn seconds_left(&self, now_us: u64) -> u32 {
        self.deadline_us.map_or(0, |deadline| {
            deadline.saturating_sub(now_us).div_ceil(1_000_000) as u32
        })
    }

    pub fn expire(&mut self, now_us: u64) -> bool {
        if self.deadline_us.is_some_and(|deadline| now_us >= deadline) {
            self.wipe();
            true
        } else {
            false
        }
    }

    pub fn take(&mut self) -> Option<DuelChoices> {
        let (Some(a), Some(b)) = (self.picks[0].as_ref(), self.picks[1].as_ref()) else {
            return None;
        };
        let choices = DuelChoices { doors: [a.door, b.door], taps: [a.tap, b.tap] };
        self.wipe();
        Some(choices)
    }

    pub fn wipe(&mut self) { self.zeroize(); }
}

fn argmax(c: &[u32; DOORS as usize]) -> u8 {
    let mut best = 0;
    for i in 1..c.len() {
        if c[i] > c[best] {
            best = i;
        }
    }
    best as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn prng_doors(n: u32) -> Vec<u8> {
        (0..n).map(|i| Sha256::digest(i.to_le_bytes())[0] & 3).collect()
    }

    #[test] fn oracle_reads_constant_player() { let mut o = Oracle::default(); let moves: Vec<Move> = (0..64).map(|_| o.record(2)).collect(); assert_eq!(moves.iter().map(|m| m.credit_mbits).sum::<u32>(), 0); assert!(!moves.last().unwrap().healthy); }
    #[test] fn oracle_reads_cycle() { let mut o = Oracle::default(); let moves: Vec<Move> = (0..64).map(|i| o.record((i % 4) as u8)).collect(); assert_eq!(moves[32..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0); }
    #[test] fn oracle_warmup_earns_nothing() { let mut o = Oracle::default(); for d in prng_doors(16) { assert_eq!(o.record(d).credit_mbits, 0); } }
    #[test] fn oracle_credits_random_player() { let mut o = Oracle::default(); let ms: Vec<Move> = prng_doors(256).into_iter().map(|d| o.record(d)).collect(); assert!(ms.iter().map(|m| m.credit_mbits).sum::<u32>() >= 150_000); assert!(ms.iter().all(|m| m.credit_mbits <= 2000)); }
    #[test] fn oracle_reads_offset_from_last_guess() { for off in [1u8, 3] { let mut o = Oracle::default(); let mut last = 0u8; let ms: Vec<Move> = (0..400).map(|_| { let m = o.record((last + off) % 4); last = m.predicted; m }).collect(); assert_eq!(ms[64..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0, "offset {off}"); } }
    #[test] fn oracle_reads_win_stay_lose_shift() { let mut o = Oracle::default(); let mut d = 0u8; let ms: Vec<Move> = (0..400).map(|_| { let m = o.record(d); if m.hit { d = (d + 1) % 4; } m }).collect(); assert_eq!(ms[64..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0); }
    #[test] fn oracle_prediction_is_fixed_before_pick() { let mut o = Oracle::default(); for d in prng_doors(20) { o.record(d); } let p = o.predict(); for door in 0..4 { assert_eq!(o.clone().record(door).predicted, p); } }
    #[test] fn oracle_wipe_forgets_history() { let mut o = Oracle::default(); for _ in 0..40 { o.record(1); } o.wipe(); assert_eq!(o.predict(), 0); assert_eq!(o.record(3).credit_mbits, 0); }

    fn duel_pick(d: &mut Duel, doors: [u8; 2]) -> DuelRound {
        if d.winner().is_some() {
            assert!(d.next_match());
        }
        let h = d.hider() as usize;
        d.round(doors[h], doors[1 - h], (10, 20, 100), (30, 40, 200))
    }

    fn random_pair(i: u32) -> [u8; 2] {
        let hash = Sha256::digest(i.to_le_bytes());
        [hash[0] & 3, hash[1] & 3]
    }

    #[test]
    fn duel_seeker_mirroring_hider_is_read() {
        let mut d = Duel::default();
        for _ in 0..128 {
            let r = duel_pick(&mut d, [2, 2]);
            assert_eq!(r.credit_mbits, [0, 0]);
        }
    }

    #[test]
    fn duel_opponent_reads_override_random_machine_history() {
        let mut d = Duel::default();
        for i in 0..128 {
            let door = random_pair(i)[0];
            let r = duel_pick(&mut d, [door, door]);
            assert_eq!(r.credit_mbits, [0, 0]);
            if i >= WARMUP as u32 {
                assert_eq!(r.healthy, [false, false]);
            }
        }
    }

    #[test]
    fn duel_both_random_both_earn() {
        let mut d = Duel::default();
        let mut total = [0; 2];
        for i in 0..128 {
            let r = duel_pick(&mut d, random_pair(i));
            for player in 0..2 {
                assert!(r.credit_mbits[player] <= MAX_MBITS);
                if i < WARMUP as u32 {
                    assert_eq!(r.credit_mbits[player], 0);
                }
                total[player] += r.credit_mbits[player];
            }
        }
        assert!(total.into_iter().all(|n| n >= 50_000), "{total:?}");
    }

    #[test]
    fn duel_constant_player_earns_nothing() {
        let mut d = Duel::default();
        let mut total = [0; 2];
        for i in 0..128 {
            let r = duel_pick(&mut d, [2, random_pair(i)[1]]);
            for player in 0..2 {
                total[player] += r.credit_mbits[player];
            }
        }
        assert_eq!(total[0], 0);
        assert!(total[1] >= 50_000);
    }

    #[test]
    fn duel_roles_swap_and_winner() {
        let mut d = Duel::default();
        for round in 1..=10 {
            let h = d.hider();
            // Player 0 scores every round, first as hider, then as seeker.
            let r = if h == 0 {
                d.round(1, 2, (0, 0, 0), (0, 0, 0))
            } else {
                d.round(1, 1, (0, 0, 0), (0, 0, 0))
            };
            assert_eq!(r.roles_swapped, round % 5 == 0);
            if round == 5 {
                assert_eq!(d.hider(), 1);
            }
            if round == 10 {
                assert_eq!(r.scores, [10, 0]);
                assert_eq!(r.winner, Some(0));
            } else {
                assert_eq!(r.winner, None);
            }
        }
        let before = d.oracles[0].doors.len();
        let r = d.round(3, 3, (1, 2, 3), (4, 5, 6));
        assert_eq!(r.credit_mbits, [0, 0]);
        assert_eq!(r.scores, [10, 0]);
        assert_eq!(d.oracles[0].doors.len(), before);
    }

    #[test]
    fn duel_next_match_preserves_learning() {
        let mut d = Duel::default();
        assert!(!d.next_match());
        while d.winner().is_none() {
            duel_pick(&mut d, [1, 2]);
        }
        let history = d.oracles[0].doors.clone();
        let reads = d.reads;
        assert!(d.next_match());
        assert_eq!(d.scores(), [0, 0]);
        assert_eq!(d.round_number(), 0);
        assert_eq!(d.hider(), 0);
        assert_eq!(d.winner(), None);
        assert_eq!(d.oracles[0].doors, history);
        assert_eq!(d.reads, reads);
    }

    #[test]
    fn duel_wipe_clears_state() {
        let mut d = Duel::default();
        for i in 0..64 {
            duel_pick(&mut d, random_pair(i));
        }
        d.wipe();
        assert!(d.oracles.iter().all(|o| o.doors.is_empty() && o.shown.is_empty()
            && o.ehits.iter().all(Vec::is_empty)));
        assert!(d.reads.iter().all(|&n| n == 0));
        assert_eq!((d.read_cursor, d.read_count), (0, 0));
        assert_eq!(d.scores(), [0, 0]);
        assert_eq!((d.hider(), d.round_number(), d.winner()), (0, 0, None));
        assert_eq!(duel_pick(&mut d, [3, 1]).credit_mbits, [0, 0]);
    }

    #[test]
    fn duel_lock_hides_until_both_and_rejects_replacement() {
        let mut lock = DuelLock::default();
        assert!(lock.pick(0, 3, (1, 2, 100), 100));
        assert!(lock.locked(0));
        assert!(!lock.locked(1));
        assert!(lock.take().is_none());
        assert!(!lock.pick(0, 1, (9, 9, 200), 200));
        assert!(lock.pick(1, 2, (3, 4, 300), 300));
        let picks = lock.take().unwrap();
        assert_eq!(picks.doors, [3, 2]);
        assert_eq!(picks.taps, [(1, 2, 100), (3, 4, 300)]);
        assert!(!lock.locked(0));
        assert!(lock.take().is_none());
    }

    #[test]
    fn duel_lock_timeout_wipes_without_a_round() {
        let mut lock = DuelLock::default();
        assert!(!lock.pick(2, 0, (0, 0, 0), 0));
        assert!(!lock.pick(0, DOORS, (0, 0, 0), 0));
        assert!(lock.pick(1, 2, (10, 20, 0), 0));
        assert_eq!(lock.seconds_left(0), 3);
        assert_eq!(lock.seconds_left(2_000_001), 1);
        assert!(!lock.expire(2_999_999));
        assert!(lock.expire(3_000_000));
        assert!(lock.picks.iter().all(Option::is_none));
        assert!(lock.take().is_none());
        assert_eq!(lock.seconds_left(3_000_000), 0);
        assert!(lock.pick(0, 1, (0, 0, 4_000_000), 4_000_000));
        // A late second pick is discarded, never paired with the old choice.
        assert!(!lock.pick(1, 2, (0, 0, 7_000_000), 7_000_000));
        assert!(lock.picks.iter().all(Option::is_none));
    }
}
