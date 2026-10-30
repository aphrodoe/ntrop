//! Shard Stack: 8x16 falling-shard board with hard-drop placements, a heuristic rank per placement, and
//! policy predictors that bound how predictable the player's choices are.
//! The board, the move history and the hits are secret entropy material: never logged, zeroized on wipe.
//! The piece sequence is never credited: only the player's choice (the placement rank) is.

use zeroize::Zeroize;

pub const W: u8 = 8;
pub const H: u8 = 16;

/// Base shapes as (x, y) cells: straight and corner (3 cells), then P, U, T and S-like (5 cells).
pub const SHAPES: [&[(u8, u8)]; 6] = [
    &[(0, 0), (1, 0), (2, 0)],
    &[(0, 0), (0, 1), (1, 1)],
    &[(0, 0), (1, 0), (0, 1), (1, 1), (0, 2)],
    &[(0, 0), (2, 0), (0, 1), (1, 1), (2, 1)],
    &[(0, 0), (1, 0), (2, 0), (1, 1), (1, 2)],
    &[(1, 0), (2, 0), (1, 1), (0, 2), (1, 2)],
];

/// Distinct 90-degree rotations of a shape, each normalised to start at (0, 0) and sorted.
fn rotations(piece: u8) -> Vec<Vec<(u8, u8)>> {
    let mut cur: Vec<(i32, i32)> = SHAPES[piece as usize % SHAPES.len()].iter().map(|&(x, y)| (x as i32, y as i32)).collect();
    let mut out: Vec<Vec<(u8, u8)>> = Vec::new();
    for _ in 0..4 {
        let (mx, my) = (cur.iter().map(|c| c.0).min().unwrap(), cur.iter().map(|c| c.1).min().unwrap());
        let mut n: Vec<(u8, u8)> = cur.iter().map(|&(x, y)| ((x - mx) as u8, (y - my) as u8)).collect();
        n.sort_unstable();
        if !out.contains(&n) { out.push(n); }
        cur = cur.iter().map(|&(x, y)| (y, -x)).collect();
    }
    out
}

/// Cells of `piece` in rotation `rot` (wraps), as (x, y) offsets.
pub fn shape(piece: u8, rot: u8) -> Vec<(u8, u8)> {
    let r = rotations(piece);
    r[rot as usize % r.len()].clone()
}

/// Where a freshly spawned piece sits (centred), the placement "just press Drop" gives.
pub fn spawn_col(piece: u8, rot: u8) -> u8 {
    (W - shape(piece, rot).iter().map(|c| c.0).max().unwrap_or(0) - 1) / 2
}

pub fn rotation_count(piece: u8) -> u8 {
    rotations(piece).len() as u8
}

#[derive(Clone, Default, Zeroize)]
pub struct Board {
    /// Row 0 is the top; bit c of a row is column c.
    rows: [u8; H as usize],
}

fn fits(rows: &[u8; 16], cells: &[(u8, u8)], col: u8, r: u8) -> bool {
    cells.iter().all(|&(x, y)| col + x < W && r + y < H && rows[(r + y) as usize] >> (col + x) & 1 == 0)
}

impl Board {
    /// Top row where a hard drop of `cells` at `col` comes to rest; `None` if it does not fit at the top.
    fn land(&self, cells: &[(u8, u8)], col: u8) -> Option<u8> {
        if !fits(&self.rows, cells, col, 0) { return None; }
        let mut r = 0;
        while fits(&self.rows, cells, col, r + 1) { r += 1; }
        Some(r)
    }

    /// Board after the drop, plus lines cleared and the landing bottom row.
    fn sim(&self, piece: u8, rot: u8, col: u8) -> Option<([u8; 16], u8, u8)> {
        let cells = rotations(piece).into_iter().nth(rot as usize)?;
        let r = self.land(&cells, col)?;
        let mut rows = self.rows;
        for &(x, y) in &cells { rows[(r + y) as usize] |= 1 << (col + x); }
        let bottom = r + cells.iter().map(|c| c.1).max().unwrap();
        let kept: Vec<u8> = rows.iter().copied().filter(|&v| v != 0xFF).collect();
        let lines = (rows.len() - kept.len()) as u8;
        let mut out = [0u8; 16];
        out[lines as usize..].copy_from_slice(&kept);
        Some((out, lines, bottom))
    }

    /// Legal (rotation, column) hard-drop placements, rotation-major then column.
    pub fn placements(&self, piece: u8) -> Vec<(u8, u8)> {
        let mut v = Vec::new();
        for (rot, cells) in rotations(piece).iter().enumerate() {
            let w = cells.iter().map(|c| c.0).max().unwrap() + 1;
            for col in 0..=(W - w) {
                if self.land(cells, col).is_some() { v.push((rot as u8, col)); }
            }
        }
        v
    }

    /// Lines cleared, or `None` if the placement is illegal.
    pub fn place(&mut self, piece: u8, rot: u8, col: u8) -> Option<u8> {
        let (rows, lines, _) = self.sim(piece, rot, col)?;
        self.rows = rows;
        Some(lines)
    }

    /// Bottom row where the piece would come to rest (larger = lower).
    pub fn landing(&self, piece: u8, rot: u8, col: u8) -> Option<u8> {
        self.sim(piece, rot, col).map(|s| s.2)
    }

    pub fn score_after(&self, piece: u8, rot: u8, col: u8) -> f32 {
        let Some((rows, lines, _)) = self.sim(piece, rot, col) else { return f32::NEG_INFINITY };
        let (mut agg, mut holes, mut bump, mut prev) = (0i32, 0i32, 0i32, None::<i32>);
        for c in 0..W {
            let top = rows.iter().position(|&r| r >> c & 1 == 1);
            let h = top.map_or(0, |t| H as i32 - t as i32);
            agg += h;
            if let Some(t) = top { holes += rows[t..].iter().filter(|&&r| r >> c & 1 == 0).count() as i32; }
            if let Some(p) = prev { bump += (h - p).abs(); }
            prev = Some(h);
        }
        -0.51 * agg as f32 + 0.76 * lines as f32 - 0.36 * holes as f32 - 0.18 * bump as f32
    }

    /// Number of legal placements scoring strictly higher, capped at 15.
    pub fn rank(&self, piece: u8, rot: u8, col: u8) -> u8 {
        let s = self.score_after(piece, rot, col);
        self.placements(piece).iter().filter(|&&(r, c)| self.score_after(piece, r, c) > s).count().min(15) as u8
    }

    /// 8x16 row-major, 1 = filled.
    pub fn cells(&self) -> [u8; 128] {
        let mut o = [0u8; 128];
        for (r, row) in self.rows.iter().enumerate() {
            for c in 0..W as usize { o[r * 8 + c] = row >> c & 1; }
        }
        o
    }

    /// Preview of a piece at the top of the board (no gravity: it waits there until dropped).
    pub fn ghost(piece: u8, rot: u8, col: u8) -> [u8; 128] {
        let mut o = [0u8; 128];
        for (x, y) in shape(piece, rot) {
            if col + x < W { o[y as usize * 8 + (col + x) as usize] = 1; }
        }
        o
    }
}

/// Rank with the highest count (lowest rank on ties); `None` if nothing was counted yet.
fn top(counts: &[u8]) -> Option<u8> {
    let m = *counts.iter().max()?;
    (m > 0).then(|| counts.iter().position(|&c| c == m).unwrap() as u8)
}

const NPOL: usize = 47;
const WIN: usize = 64;
const SLACK_HITS: u32 = 4;
const HEALTH_MIN_N: u32 = 16;

/// Predictors of the player's next placement, committed before the drop: P0 heuristic best, P1 leftmost,
/// P2 rightmost, P3 centre-most, P4 lowest landing, P5 same column as last, P6 same placement as last,
/// P7..P14 lag-k repeats of the rank (k = 1..8), P15 most frequent successor of the previous rank, P16 of
/// the previous two ranks, P17..P23 lag-k repeats of the column (k = 2..8, best rotation there; the nearest legal column if it is blocked),
/// P24..P30 lag-k repeats of the full (rot, col) (k = 2..8), P31/P32 most frequent successor column of the previous 1/2 columns,
/// P40 spawn default (centred, rot 0), P41/P42 heuristic best / lowest landing restricted to rot 0, P43/P44 the same
/// restricted to the last-used rotation, P45 last (rot, col) for this piece type, P46 most frequent column for this piece type,
/// P33..P39 majority column of the moves one, two and three periods ago (period 2..8) (these two keep a mostly periodic rank pattern with occasional glitches from earning). MCV is IID-only, so this bounds the credit.
#[derive(Clone, Zeroize)]
pub struct ShardPolicies {
    /// Per predictor: hit bits over the last 64 pieces (bit 0 newest).
    hits: [u64; NPOL],
    scored: u32,
    /// [valid, rot, col] of the previous placement.
    last: [u8; 3],
    ranks: [u8; 8],
    nranks: u8,
    /// Previous placements [rot, col], newest first.
    moves: [[u8; 2]; 24],
    nmoves: u8,
    /// How often each rank followed rank a (index a * 16 + rank) / ranks (a, b) (index (a * 16 + b) * 16 + rank).
    /// Per piece type: [valid, rot, col] last used, and column counts.
    by_piece: [[u8; 3]; 6],
    piece_cols: [[u8; 8]; 6],
    colsucc1: [u8; 64],
    colsucc2: [u8; 512],
    succ1: [u8; 256],
    succ2: [u8; 4096],
}

impl Default for ShardPolicies {
    fn default() -> Self {
        Self { hits: [0; NPOL], scored: 0, last: [0; 3], ranks: [0; 8], nranks: 0, moves: [[0; 2]; 24], nmoves: 0, by_piece: [[0; 3]; 6], piece_cols: [[0; 8]; 6], colsucc1: [0; 64], colsucc2: [0; 512], succ1: [0; 256], succ2: [0; 4096] }
    }
}

impl ShardPolicies {
    /// Scores every predictor against the chosen move (`rank` is its rank). Call before placing the piece.
    pub fn observe(&mut self, board: &Board, piece: u8, rot: u8, col: u8, rank: u8) {
        let ps = board.placements(piece);
        let sc = |r: u8, c: u8| board.score_after(piece, r, c);
        let pick = |f: &dyn Fn(&(u8, u8)) -> i64| ps.iter().copied().min_by_key(|p| f(p));
        let mine = Some((rot, col));
        let best_at = |col: u8| ps.iter().copied().filter(|p| p.1 == col).fold(None::<(u8, u8)>, |b, p| match b { Some(q) if sc(q.0, q.1) >= sc(p.0, p.1) => b, _ => Some(p) });
        let mut pred: [Option<(u8, u8)>; 7] = [None; 7];
        pred[1] = pick(&|&(r, c)| (c as i64) * 16 + r as i64);
        pred[2] = pick(&|&(r, c)| -(c as i64) * 16 + r as i64);
        pred[3] = pick(&|&(r, c)| (2 * c as i64 - 7).abs() * 16 + r as i64);
        pred[4] = pick(&|&(r, c)| -(board.landing(piece, r, c).unwrap_or(0) as i64) * 256 + c as i64 * 16 + r as i64);
        if self.last[0] == 1 {
            let (lr, lc) = (self.last[1], self.last[2]);
            pred[5] = best_at(lc);
            pred[6] = ps.contains(&(lr, lc)).then_some((lr, lc));
        }
        let near = |mc: u8| ps.iter().copied().min_by_key(|&(r, c)| ((c as i32 - mc as i32).abs(), -(sc(r, c) * 1000.0) as i64, r));
        let pi = piece as usize % SHAPES.len();
        let restricted = |rr: u8, low: bool| ps.iter().copied().filter(|p| p.0 == rr).min_by_key(|&(r, c)| if low { -(board.landing(piece, r, c).unwrap_or(0) as i64) * 256 + c as i64 } else { -(sc(r, c) * 1000.0) as i64 * 256 + c as i64 });
        let mut hit = [false; NPOL];
        let sp = (0u8, spawn_col(piece, 0));
        hit[40] = ps.contains(&sp) && sp == (rot, col);
        hit[41] = restricted(0, false) == mine;
        hit[42] = restricted(0, true) == mine;
        if self.last[0] == 1 {
            hit[43] = restricted(self.last[1], false) == mine;
            hit[44] = restricted(self.last[1], true) == mine;
        }
        let bp = self.by_piece[pi];
        hit[45] = bp[0] == 1 && ps.contains(&(bp[1], bp[2])) && (bp[1], bp[2]) == (rot, col);
        hit[46] = top(&self.piece_cols[pi]).and_then(near).map(|p| p.1) == Some(col);
        self.by_piece[pi] = [1, rot, col];
        self.piece_cols[pi][col as usize] = self.piece_cols[pi][col as usize].saturating_add(1);
        hit[0] = rank == 0;
        for i in 1..7 { hit[i] = pred[i] == mine; }
        for k in 1..=8usize { hit[6 + k] = self.nranks as usize >= k && self.ranks[k - 1] == rank; }
        for k in 2..=8usize {
            if self.nmoves as usize >= k {
                let [mr, mc] = self.moves[k - 1];
                hit[15 + k] = near(mc).map(|p| p.1) == Some(col);
                hit[22 + k] = ps.contains(&(mr, mc)) && (mr, mc) == (rot, col);
            }
        }
        for per in 2..=8usize {
            let mut v: Vec<u8> = (1..=3).filter(|m| self.nmoves as usize >= m * per).map(|m| self.moves[m * per - 1][1]).collect();
            let maj = v.iter().copied().max_by_key(|&c| (v.iter().filter(|&&d| d == c).count(), c == v[0])).filter(|_| !v.is_empty());
            hit[31 + per] = maj.and_then(near).map(|p| p.1) == Some(col);
            v.zeroize();
        }
        if self.nmoves >= 1 {
            let c = self.moves[0][1] as usize * 8;
            hit[31] = top(&self.colsucc1[c..c + 8]).and_then(near).map(|p| p.1) == Some(col);
            self.colsucc1[c + col as usize] = self.colsucc1[c + col as usize].saturating_add(1);
        }
        if self.nmoves >= 2 {
            let c = (self.moves[1][1] as usize * 8 + self.moves[0][1] as usize) * 8;
            hit[32] = top(&self.colsucc2[c..c + 8]).and_then(near).map(|p| p.1) == Some(col);
            self.colsucc2[c + col as usize] = self.colsucc2[c + col as usize].saturating_add(1);
        }
        if self.nranks >= 1 {
            let c = self.ranks[0] as usize * 16;
            hit[15] = top(&self.succ1[c..c + 16]) == Some(rank);
            self.succ1[c + rank as usize] = self.succ1[c + rank as usize].saturating_add(1);
        }
        if self.nranks >= 2 {
            let c = (self.ranks[1] as usize * 16 + self.ranks[0] as usize) * 16;
            hit[16] = top(&self.succ2[c..c + 16]) == Some(rank);
            self.succ2[c + rank as usize] = self.succ2[c + rank as usize].saturating_add(1);
        }
        for (h, b) in self.hits.iter_mut().zip(hit) { *h = (*h << 1) | b as u64; }
        self.scored = (self.scored + 1).min(WIN as u32);
        self.ranks.rotate_right(1);
        self.ranks[0] = rank;
        self.nranks = (self.nranks + 1).min(8);
        self.moves.rotate_right(1);
        self.moves[0] = [rot, col];
        self.nmoves = (self.nmoves + 1).min(24);
        self.last = [1, rot, col];
    }

    /// (credit ceiling in mbits, healthy) after the latest move; `None` until one is scored.
    pub fn bound(&self) -> Option<(u32, bool)> {
        (self.scored > 0).then(|| {
            let best = self.hits.iter().map(|h| h.count_ones()).max().unwrap_or(0);
            let p = ((best + SLACK_HITS) as f64 / self.scored as f64).min(1.0);
            ((-p.log2() * 1000.0).floor() as u32, !(p > 0.5 && self.scored >= HEALTH_MIN_N))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, WordCount};
    use sha2::{Digest, Sha256};
    fn byte(i: u32, j: u32) -> u8 { Sha256::digest([i.to_le_bytes(), j.to_le_bytes()].concat())[0] }
    fn sess() -> Session { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); s }
    fn best(b: &Board, piece: u8) -> (u8, u8) { b.placements(piece).into_iter().find(|&(r, c)| b.rank(piece, r, c) == 0).unwrap() }
    /// Plays 120 pieces (piece i from `seed`), choosing with `f`; returns total credit in mbits.
    fn play(seed: u32, f: impl Fn(&Board, u8, &[(u8, u8)], u32) -> (u8, u8)) -> u32 {
        let mut s = sess(); let mut total = 0;
        for i in 0..120u32 {
            s.shard_spawn(byte(seed, i) % 6);
            let ps = s.shard.placements(s.shard_piece);
            let (r, c) = f(&s.shard, s.shard_piece, &ps, i);
            total += s.shard_place(r, c, i as u64).unwrap().credit_mbits;
        }
        total
    }
    fn near_rank(b: &Board, p: u8, ps: &[(u8, u8)], k: u8) -> (u8, u8) { *ps.iter().min_by_key(|&&(r, c)| (b.rank(p, r, c) as i32 - k as i32).abs()).unwrap() }
    #[test] fn shard_line_clears() {
        let mut b = Board::default(); b.rows[15] = 0b1111_1000; // gap at columns 0..=2
        assert_eq!(b.place(0, 0, 0), Some(1)); assert_eq!(b.cells()[15 * 8..].iter().sum::<u8>(), 0);
    }
    #[test] fn shard_rank_of_best_is_zero() { let b = Board::default(); let (r, c) = best(&b, 2); assert_eq!(b.rank(2, r, c), 0); }
    #[test] fn shard_illegal_placement_rejected() { let mut b = Board::default(); assert_eq!(b.place(0, 0, 6), None); assert_eq!(b.cells().iter().sum::<u8>(), 0); }
    #[test] fn shard_rotations_are_distinct() { assert_eq!(rotation_count(0), 2); assert_eq!(rotation_count(1), 4); assert_eq!(rotation_count(3), 4); assert!((0..6).all(|p| rotations(p).iter().all(|r| r.len() == SHAPES[p as usize].len()))); }
    #[test] fn shard_best_play_earns_nothing() { assert_eq!(play(1, |b, p, _, _| best(b, p)), 0); }
    #[test] fn shard_varied_play_earns() { let t = play(1, |b, p, ps, i| near_rank(b, p, ps, byte(i, 99) % 8)); assert!(t >= 100_000, "total {t}"); }
    #[test] fn shard_fixed_policies_earn_nothing() {
        let t = [
            play(2, |_, _, ps, _| *ps.iter().min_by_key(|&&(r, c)| (c, r)).unwrap()),
            play(2, |_, _, ps, _| *ps.iter().min_by_key(|&&(r, c)| (std::cmp::Reverse(c), r)).unwrap()),
            play(2, |_, _, ps, _| *ps.iter().min_by_key(|&&(r, c)| ((2 * c as i32 - 7).abs(), r)).unwrap()),
            play(2, |b, p, ps, _| *ps.iter().min_by_key(|&&(r, c)| (std::cmp::Reverse(b.landing(p, r, c).unwrap()), c, r)).unwrap()),
        ];
        assert!(t.iter().all(|&x| x <= 3_000), "totals {t:?}");
    }
    #[test] fn shard_rank_cycle_earns_little() { let t = play(3, |b, p, ps, i| near_rank(b, p, ps, (i % 4) as u8)); assert!(t <= 30_000, "total {t}"); }
    #[test] fn shard_column_cycle_earns_nothing() {
        let t = play(6, |b, p, ps, i| { let tc = [0i32, 2, 4, 6][i as usize % 4]; *ps.iter().min_by_key(|&&(r, c)| ((c as i32 - tc).abs(), b.rank(p, r, c), r)).unwrap() });
        assert!(t <= 3_000, "total {t}");
    }
    #[test] fn shard_unhealthy_earns_nothing() {
        let t = play(7, |_, p, ps, _| { let sc = spawn_col(p, 0); *ps.iter().min_by_key(|&&(r, c)| ((c as i32 - sc as i32).abs(), r)).unwrap() });
        assert!(t <= 3_000, "just press Drop {t}");
    }
    #[test] fn shard_piece_driven_habits_earn_nothing() {
        for seed in 10..14 {
            let a = play(seed, |b, p, ps, _| *ps.iter().min_by_key(|&&(r, c)| (r != 0, b.rank(p, r, c), c)).unwrap());
            let f = play(seed, |b, p, ps, _| { let t = [0i32, 5, 2, 7, 3, 1][p as usize]; *ps.iter().min_by_key(|&&(r, c)| ((c as i32 - t).abs(), b.rank(p, r, c), r)).unwrap() });
            let g = play(seed, |_, p, ps, _| { let t = [0i32, 5, 2, 7, 3, 1][p as usize]; *ps.iter().min_by_key(|&&(r, c)| (r != 0, (c as i32 - t).abs())).unwrap() });
            assert!(a <= 3_000 && f <= 3_000 && g <= 3_000, "seed {seed}: never-rotate-best {a}, column-per-type {f}, column-per-type rot0 {g}");
        }
    }
    #[test] fn shard_piece_sequence_is_not_credited() { let (a, b) = (play(4, |b, p, _, _| best(b, p)), play(5, |b, p, _, _| best(b, p))); assert_eq!((a, b), (0, 0)); }
    #[test] fn wipe_clears_board() { let mut s = sess(); for i in 0..5u64 { s.shard_spawn(2); let (r, c) = s.shard.placements(2)[0]; s.shard_place(r, c, i); } assert!(s.shard_cells().iter().any(|&v| v == 1)); s.wipe(); assert!(s.shard_cells().iter().all(|&v| v == 0)); }
}
