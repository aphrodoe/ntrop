# Entropy Arcade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A KeyOS app for Passport Prime that generates BIP39 seeds from playful user entropy (touch scribbling and camera noise) mixed with the hardware RNG, with honest accounting, live health tests, and a verifiable dice mode.

**Architecture:** All security logic lives in a pure-Rust, host-testable crate `entropy-core` (no KeyOS deps; it takes hardware randomness as an argument). The KeyOS app is a thin shell. Rust owns a `Session` (pool, secrets, current screen) and pushes plain display data into one Slint global. Slint renders a screen per `AppState.screen` value with `if` blocks, so there's no SDK router. All flow decisions and secret handling stay in Rust.

**Tech Stack:** KeyOS SDK 1.1.0 (Rust std + Slint, `slint-keyos-platform`), `sha2`, `bip39`, `zeroize`, `qrcode` (no default features), `k256`, `hmac`, `ripemd`, `getrandom` 0.2 (SDK-patched to the TRNG).

**Spec:** `/home/aphrodoe/Documents/bitmela/docs/superpowers/specs/2026-10-30-entropy-arcade-design.md`

## Global Constraints

- The app lives at `/home/aphrodoe/Documents/bitmela/entropy-arcade`. Every command runs inside `nix develop ~/.foundation/sdk/current` (or a `foundation develop` shell).
- No secret (entropy, pool digest, words, seed, fingerprint inputs) may reach `log::*`, files, settings or panics. Log only counts and states.
- Mixing must be exactly `SHA-256("entropy-arcade/v1" ‖ hw_random[32] ‖ pool_digest[32])`, truncated to 32 bytes (24 words) or 16 bytes (12 words).
- Dice mode must be exactly `SHA-256(ASCII digits of rolls)`, truncated the same way. Minimum 50 rolls for 12 words and 99 for 24. **No hardware RNG in dice mode.**
- Credits: 1 bit per healthy touch sample. 4 bits per healthy camera frame, only if a touch sample arrived ≤250 ms earlier, with camera total capped at `target_bits / 2`. Outguess: `−log2(p_ub)` per move (≤ 2 bits), `p_ub = (hits + 4) / n` over the last `n ≤ 32` moves, 0 for the first 16 moves and while `p_ub > 0.5`. Static Hunt: 1 bit per shot whose miss offset passes the touch health tests. Targets are 256 bits (24 words) or 128 bits (12 words).
- Palette "Neon on ink": ink `#0B0A12`, surface `#15131F`, raised `#1E1B2C`, line `#2C2840`, text `#F2F0FF`, muted `#9A94B8`, touch `#00E5FF`, camera `#FF2BD6`, game `#8B5CFF`, primary/success `#C6FF3D`, warning `#FFB020`, error `#FF4D6D`.
- Game-only randomness (ghost spawns, static texture) never comes from the pool.
- UI copy: the wipe message reads **"App buffers wiped"**, never "all memory". Credits are labelled as **conservative estimates**.
- Stay on `getrandom = "0.2"` (the SDK patch covers only 0.2).
- Screen 480×800, portrait. Use `@ui/*` kit components and `Theme.*` tokens where they exist.

## Review Focus

1. **Scribbling in a straight line or tapping one spot.** Repeated identical deltas must earn 0 credit and eventually flag the source unhealthy, never crash or stall the meter forever once real scribbling resumes. Test: `touch_recovers_after_unhealthy` (Task 2).
2. **Camera on while not touching.** The meter must not move. Test: `camera_needs_recent_touch` (Task 1).
3. **Leaving a screen mid-flow (back or exit) after words were shown.** Secrets must be wiped. Test: `session_wipe_clears_secrets` (Task 5).
4. **The hardware RNG failing (`getrandom` returns Err).** Generation must stop with an error screen, never fall back to user-only entropy in Arcade mode. Test: `finish_requires_hw_random` (Task 5).
5. **A dice roll outside 1..=6, or too few rolls.** These must be rejected and never produce a seed. Test: `dice_rejects_bad_input` (Task 4).
6. **A predictable Outguess player, or a guess shown before the pick.** Patterned play must earn 0, and the guess must be committed before the tap. Tests: `oracle_reads_cycle`, `oracle_prediction_is_fixed_before_pick` (Task 9).

---

## File Structure

```
entropy-arcade/
  Cargo.toml                    # app; adds entropy-core + zeroize + getrandom 0.2
  app-config.toml               # friendly name "Entropy Arcade"; permission template list
  permission_templates.toml     # + [entropy-arcade] template: camera + haptics messages
  crates/entropy-core/
    Cargo.toml
    src/lib.rs                  # pub mod + re-exports; WordCount
    src/pool.rs                 # EntropyPool: absorb, credit accounting, finish()
    src/health.rs               # TouchHealth, CameraHealth
    src/seed.rs                 # words, Standard SeedQR digits, QR matrix, master fingerprint
    src/randomart.rs            # drunken-bishop art from fingerprint
    src/dice.rs                 # dice_entropy()
    src/quiz.rs                 # make_quiz()
    src/session.rs              # Session (moved here from the app crate: host-testable)
    src/outguess.rs             # Oracle predictor + live credit
  src/main.rs                   # app_main: wiring callbacks -> Session -> AppState
  src/camera.rs                 # subscribe frames -> pool + preview image
  src/haptics.rs                # tiny wrapper: tick/ramp/full/alert
  src/hunt.rs                   # Static Hunt game state + software renderer (no secrets)
  src/theme.rs                  # (scaffolded, unchanged)
  ui/app.slint                  # AppWindow: switch on AppState.screen
  ui/state.slint                # global AppState (props + callbacks), Screen enum
  ui/screens/home.slint, painter.slint, summary.slint, words.slint,
             seedqr.slint, quiz.slint, done.slint, dice.slint, error.slint,
             outguess.slint, hunt.slint
  ui/widgets/meter.slint        # entropy meter with per-source segments
```

---

### Task 1: Scaffold + `entropy-core` pool and mixing

**Files:**
- Create: the app via `foundation new`, plus `crates/entropy-core/{Cargo.toml,src/lib.rs,src/pool.rs,src/health.rs}`
- Modify: `entropy-arcade/Cargo.toml`

**Interfaces:**
- Produces:
  - `pub enum WordCount { Twelve, TwentyFour }` with `fn target_bits(self) -> u32` (128/256) and `fn entropy_len(self) -> usize` (16/32)
  - `pub enum SampleResult { Credited(u32), Uncredited, SourceUnhealthy }`
  - `pub struct EntropyPool` with:
    - `new(wc: WordCount) -> Self`
    - `add_touch(&mut self, x: i32, y: i32, t_us: u64) -> SampleResult`
    - `add_camera_frame(&mut self, frame: &[u8], t_us: u64) -> SampleResult`
    - `touch_bits(&self) -> u32`, `camera_bits(&self) -> u32`, `credited_bits(&self) -> u32` (sum, clamped to the target), `is_full(&self) -> bool`, `touch_healthy(&self) -> bool`, `camera_healthy(&self) -> bool`
    - `finish(self, hw_random: &[u8; 32]) -> zeroize::Zeroizing<Vec<u8>>` (length = `entropy_len`)
  - In `health.rs` (stubbed here, filled in Task 2): `pub enum Verdict { Good, Bad, Unhealthy }`, `TouchHealth::check(&mut self, dx: i32, dy: i32) -> Verdict`, `CameraHealth::check(&mut self, frame: &[u8]) -> Verdict`.

- [ ] **Step 1: Scaffold.** In `/home/aphrodoe/Documents/bitmela`, run `foundation new entropy-arcade --friendly-name "Entropy Arcade" --launcher-name "Entropy Arcade" --description "Play your way to a seed phrase" --publisher-name "Bitmela" < /dev/null`. Copy `hello-prime/.mcp.json` into it (it has the `passport-sim` server). Add `CLAUDE.md` containing just `@AGENTS.md`, because Claude Code doesn't read `AGENTS.md` on its own. Add `.claude/settings.json` that denies the hardware-flashing MCP tools: `mcp__passport-drive__samba_*`, `mcp__passport-drive__reboot_to_samba`, `mcp__passport-drive__install_certificate`, `mcp__passport-sim__samba_*`, `mcp__passport-sim__reboot_to_samba`. From here on, launch the coding agent **from inside `entropy-arcade/`, in a `foundation develop` shell**. Outside that shell, `foundation-passport-drive` fails with `libudev.so.1: cannot open shared object file`, and the project skills and `.mcp.json` only load when the agent starts in the app directory.
- [ ] **Step 2: Create `crates/entropy-core`** (edition 2021). Deps: `sha2 = "0.10"`, `zeroize = "1"`, `bip39 = { version = "2", default-features = false, features = ["std"] }`, `qrcode = { version = "0.14", default-features = false }`, `k256 = "0.13"`, `hmac = "0.12"`, `ripemd = "0.1"`. Add `entropy-core = { path = "crates/entropy-core" }` to the app's `Cargo.toml`.
- [ ] **Step 3: Write failing tests** in `pool.rs`:

```rust
#[test] fn finish_is_spec_mix() {
    let p = EntropyPool::new(WordCount::TwentyFour);
    let hw = [7u8; 32];
    let empty_digest = Sha256::new().finalize();
    let mut h = Sha256::new(); h.update(b"entropy-arcade/v1"); h.update(hw); h.update(empty_digest);
    assert_eq!(&p.finish(&hw)[..], &h.finalize()[..]);
}
#[test] fn twelve_words_truncates_to_16() { assert_eq!(EntropyPool::new(WordCount::Twelve).finish(&[1;32]).len(), 16); }
#[test] fn touch_credits_one_bit() { /* first sample Uncredited; next 10 distinct moves -> touch_bits()==10 */ }
#[test] fn different_input_different_output() { /* same hw, one extra touch -> different finish() */ }
#[test] fn camera_needs_recent_touch() { /* camera frames with no touch -> camera_bits()==0; touch at t=0 then a frame at t=200_000 -> 4; a frame at t=300_000 after no touch -> 0 */ }
#[test] fn camera_capped_at_half() { /* interleave touches + 200 distinct frames on Twelve -> camera_bits()==64 */ }
#[test] fn full_at_target() { /* 128 credited touches on Twelve -> is_full() */ }
```

- [ ] **Step 4: Run** `cargo test -p entropy-core`. Expected: FAIL (not implemented).
- [ ] **Step 5: Implement `pool.rs`.** Absorb every sample (healthy or not) as `tag(u8) ‖ len(u32 LE) ‖ bytes`, with tags `0x01` touch (`x,y,t_us` as LE i32,i32,u64) and `0x02` camera. Keep the delta from the previous touch for health. Health stubs in `health.rs` return `Good` unless the delta is (0,0) (Task 2 makes them real).
- [ ] **Step 6: Run** `cargo test -p entropy-core`. Expected: PASS.
- [ ] **Step 7: Commit:** `git add -A && git commit -m "feat: scaffold app and entropy pool with spec mixing"`

### Task 2: Health tests

**Files:** Modify `crates/entropy-core/src/health.rs`

**Interfaces:**
- Consumes: `Verdict` and the two `check` signatures from Task 1.
- Produces: the constants `TOUCH_RCT_LIMIT = 8`, `TOUCH_APT_WINDOW = 64`, `TOUCH_APT_MAX = 32`, `CAM_STUCK_LIMIT = 5`, `CAM_MIN_MEAN_DIFF: f32 = 0.5` (all `pub const` in one block).

- [ ] **Step 1: Failing tests:**

```rust
#[test] fn touch_zero_delta_bad() { assert_eq!(TouchHealth::default().check(0,0), Verdict::Bad); }
#[test] fn touch_repeat_delta_bad() { let mut h = TouchHealth::default(); h.check(3,4); assert_eq!(h.check(3,4), Verdict::Bad); }
#[test] fn touch_unhealthy_after_8_bad() { /* 8 x check(0,0): 8th returns Unhealthy */ }
#[test] fn touch_recovers_after_unhealthy() { /* after Unhealthy, a new distinct delta returns Good */ }
#[test] fn touch_apt_flags_dominant_delta() { /* in 64 samples alternate (1,0),(0,1) — 32 each, ok; add (1,0) so it exceeds 32 in window -> Unhealthy */ }
#[test] fn camera_identical_frame_bad() { /* same frame twice -> Bad */ }
#[test] fn camera_unhealthy_after_5_stuck() {}
#[test] fn camera_noisy_frames_good() { /* frames with pseudo-random bytes (simple LCG) -> Good */ }
```

- [ ] **Step 2: Run** `cargo test -p entropy-core health`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - **Touch:** a delta is bad if it is (0,0) or equals the previous delta. `TOUCH_RCT_LIMIT` consecutive bad deltas make the source Unhealthy; any good delta clears this. The APT keeps a `VecDeque` of the last 64 deltas, and any delta occurring more than `TOUCH_APT_MAX` times makes the verdict Unhealthy.
  - **Camera:** compute the SHA-256 of the frame and compare it with the previous frame's hash. Mean diff = mean |a−b| of the low nibble of every 16th byte compared with the previous frame. A frame is Bad if the hash is equal or the mean diff is below `CAM_MIN_MEAN_DIFF`. `CAM_STUCK_LIMIT` consecutive Bad frames make the source Unhealthy.
  - **First sample:** with no previous sample there is nothing to compare, so return `Bad` and let the pool report it as `Uncredited`.
- [ ] **Step 4: Run** `cargo test -p entropy-core`. Expected: PASS (including Task 1 tests).
- [ ] **Step 5: Commit:** `"feat: SP 800-90B-style health tests"`

### Task 3: Seed outputs: words, SeedQR, fingerprint, randomart

**Files:** Create `crates/entropy-core/src/seed.rs` and `src/randomart.rs`

**Interfaces:**
- Produces:
  - `pub fn words(entropy: &[u8]) -> Zeroizing<Vec<String>>`
  - `pub fn seedqr_digits(entropy: &[u8]) -> Zeroizing<String>`
  - `pub fn qr_matrix(data: &str) -> (usize, Vec<bool>)` (size, row-major, true = dark)
  - `pub fn master_fingerprint(entropy: &[u8]) -> [u8; 4]` (BIP39 seed with an empty passphrase, then HMAC-SHA512 with key `"Bitcoin seed"`, giving the k256 master secret; then compressed pubkey, then HASH160, then the first 4 bytes)
  - `pub fn randomart(fp: &[u8; 4]) -> String` (11 lines × 19 chars including the border; drunken bishop on a 17×9 field, walked over `SHA-256(fp)`)

- [ ] **Step 1: Failing tests** (standard vectors):

```rust
#[test] fn words_zero_16() { let w = words(&[0u8;16]); assert_eq!(w.len(), 12); assert_eq!(w[11], "about"); assert!(w[..11].iter().all(|x| x=="abandon")); }
#[test] fn words_zero_32() { let w = words(&[0u8;32]); assert_eq!(w.len(), 24); assert_eq!(w[23], "art"); }
#[test] fn seedqr_abandon_about() { assert_eq!(&*seedqr_digits(&[0u8;16]), &format!("{}0003", "0000".repeat(11))); }
#[test] fn fingerprint_abandon_about() { assert_eq!(master_fingerprint(&[0u8;16]), [0x73,0xc5,0xda,0x0a]); }
#[test] fn qr_is_square() { let (n, m) = qr_matrix("0123"); assert_eq!(m.len(), n*n); }
#[test] fn randomart_shape_and_determinism() { let a = randomart(&[1,2,3,4]); assert_eq!(a.lines().count(), 11); assert!(a.lines().all(|l| l.chars().count()==19)); assert_eq!(a, randomart(&[1,2,3,4])); assert_ne!(a, randomart(&[1,2,3,5])); }
```

- [ ] **Step 2: Run** `cargo test -p entropy-core seed randomart`. Expected: FAIL.
- [ ] **Step 3: Implement** with `bip39::Mnemonic::from_entropy`, its `word_indices` / `to_seed("")`, `qrcode::QrCode::new`, `hmac::Hmac<sha2::Sha512>`, `k256::SecretKey::from_slice(..).public_key().to_encoded_point(true)`, and `ripemd::Ripemd160`. Randomart uses the OpenSSH symbol set ` .o+=*BOX@%&#/^` with `S`/`E` for start/end.
- [ ] **Step 4: Run** `cargo test -p entropy-core`. Expected: PASS.
- [ ] **Step 5: Commit:** `"feat: words, SeedQR, master fingerprint, randomart"`

### Task 4: Dice and quiz

**Files:** Create `crates/entropy-core/src/dice.rs` and `src/quiz.rs`

**Interfaces:**
- Produces:
  - `pub enum DiceError { BadRoll(u8), TooFew { have: usize, need: usize } }`
  - `pub fn min_rolls(wc: WordCount) -> usize` (50/99)
  - `pub fn dice_entropy(rolls: &[u8], wc: WordCount) -> Result<Zeroizing<Vec<u8>>, DiceError>`
  - `pub struct QuizQuestion { pub position: usize /* 1-based */, pub choices: [String; 4], pub correct: usize }`
  - `pub fn make_quiz(words: &[String], rand: &[u8; 32]) -> Vec<QuizQuestion>` (3 questions, distinct positions; the decoys are distinct BIP39 words, none equal to the correct word; the correct index comes from `rand`)

- [ ] **Step 1: Failing tests:**

```rust
#[test] fn dice_coldcard_vector() {
    let rolls: Vec<u8> = "12345612345612345612345612345612345612345612345612".bytes().map(|b| b - b'0').collect();
    let e = dice_entropy(&rolls, WordCount::Twelve).unwrap();
    assert_eq!(hex(&e), "ee72ae915a4e6ea7ccbeb8e5e5eecef2"); // first 16 bytes of `echo -n <rolls> | sha256sum`
}
#[test] fn dice_rejects_bad_input() {
    assert!(matches!(dice_entropy(&[7; 99], WordCount::TwentyFour), Err(DiceError::BadRoll(7))));
    assert!(matches!(dice_entropy(&[0; 99], WordCount::TwentyFour), Err(DiceError::BadRoll(0))));
    assert!(matches!(dice_entropy(&[1; 49], WordCount::Twelve), Err(DiceError::TooFew { have: 49, need: 50 })));
}
#[test] fn quiz_properties() { /* for several rand arrays: 3 questions, distinct positions in 1..=12, choices[correct]==words[position-1], 4 distinct choices */ }
```

- [ ] **Step 2: Run** `cargo test -p entropy-core dice quiz`. Expected: FAIL.
- [ ] **Step 3: Implement.** Dice: map rolls to ASCII `'1'..'6'`, compute SHA-256, truncate to `entropy_len`. Quiz: derive indices from `rand` bytes (reject-and-continue on collisions). Decoys come from `bip39::Language::English.word_list()`.
- [ ] **Step 4: Run** `cargo test -p entropy-core`. Expected: PASS.
- [ ] **Step 5: Commit:** `"feat: dice mode and backup quiz"`

### Task 5: App shell: Session, Home, Chaos Painter, Summary, Error

**Files:**
- Create: `src/session.rs`, `ui/state.slint`, `ui/screens/{home,painter,summary,error}.slint`, `ui/widgets/meter.slint`
- Modify: `src/main.rs`, `ui/app.slint`, `Cargo.toml` (add `zeroize = "1"`, `getrandom = "0.2"`)

**Interfaces:**
- Consumes: `EntropyPool`, `WordCount`, `words`, `make_quiz`.
- Produces (Slint `global AppState`):
  - `in property <Screen> screen` where `enum Screen { home, painter, summary, words, seedqr, quiz, done, dice, error }`
  - Meter properties: `in property <int> touch-bits, camera-bits, target-bits`, `in property <bool> touch-healthy, camera-healthy, camera-on`
  - `in property <[{x: length, y: length, age: float}]> trail`
  - `in property <string> error-text`
  - Callbacks: `start-arcade(int /*12|24*/)`, `start-dice(int)`, `touch(length, length)`, `toggle-camera()`, `painter-done()`, `generate()`, `back-home()`
- Produces (Rust `Session` in `session.rs`):
  - `Session::new() -> Self`
  - `start_arcade(&mut self, wc: WordCount)`
  - `on_touch(&mut self, x: i32, y: i32, t_us: u64) -> SampleResult`
  - `generate(&mut self, hw: Result<[u8; 32], ()>) -> Result<(), String>` (stores `Zeroizing` entropy and words; Err when `hw` is Err)
  - `words(&self) -> &[String]`, `wipe(&mut self)`
  - Plus `fn hw_random() -> Result<[u8; 32], ()>` in `main.rs`, using `getrandom::getrandom`.

- [ ] **Step 1: Failing tests** in `src/session.rs` behind `#[cfg(test)]`. If the app crate won't build tests on host, move `Session` into `entropy-core` as `session.rs`. It has no KeyOS deps, so that works.

```rust
#[test] fn finish_requires_hw_random() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); assert!(s.generate(Err(())).is_err()); assert!(s.words().is_empty()); }
#[test] fn session_wipe_clears_secrets() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); s.generate(Ok([3;32])).unwrap(); assert_eq!(s.words().len(), 12); s.wipe(); assert!(s.words().is_empty()); }
```

- [ ] **Step 2: Run** the tests. Expected: FAIL. Then implement `Session` and run again. Expected: PASS.
- [ ] **Step 3: Build the UI.**
  - `app.slint` renders one screen per `AppState.screen` with `if` blocks.
  - **Home:** title "Entropy Arcade", tagline "Can you trust one chip?", Buttons "Arcade · 24 words", "Arcade · 12 words", "Dice Duel".
  - **Painter:** a full-screen `TouchArea` whose `moved`/`pointer-event` handler calls `AppState.touch(self.mouse-x, self.mouse-y)`. Draw the trail as up to 200 dots in a `for`, with opacity `1 - age`. The meter sits at the top; "Done" is enabled once full.
  - **Summary:** per-source bits, the "+ hardware 256" badge, and a Generate button.
  - **Error:** shows `error-text` and a Home button.
  - **Rust side:** `t_us` from `Instant` since app start. Trail ages decay with a ~16 ms `slint::Timer`. Never log coordinates.
- [ ] **Step 4: Verify in the simulator.** Check the layouts first with `foundation preview ui/app.slint --auto-reload`, the SDK's preferred UI check. Then run `foundation sim`. With the `passport-sim` MCP (or by hand), swipe on Painter. The meter rises to full in about 5–10 s of scribbling, and Generate moves to the Words screen (a placeholder until Task 6). Take a screenshot.
- [ ] **Step 5: Commit:** `"feat: app shell, chaos painter, meter, mixing"`

### Task 6: Words, SeedQR, Quiz, Done and the wipe

**Files:**
- Create: `ui/screens/{words,seedqr,quiz,done}.slint`
- Modify: `src/session.rs`, `src/main.rs`, `ui/state.slint`

**Interfaces:**
- Consumes: `seedqr_digits`, `qr_matrix`, `master_fingerprint`, `randomart`, `make_quiz`.
- Produces (AppState):
  - Words: `in property <[string]> page-words`, `in property <int> page, page-count, first-index`, with callbacks `next-page()`, `prev-page()`
  - QR: `in property <image> qr`
  - Quiz: `in property <int> quiz-step, quiz-position`, `in property <[string]> quiz-choices`, with callback `answer(int)`
  - Done: `in property <string> fingerprint, art`, with callback `finish()`

- [ ] **Step 1: Implement the flow:** Words (6 per page, numbered, with the warning "Write these down. Never photograph them.") → SeedQR → Quiz (3 questions; a wrong answer goes back to Words page 1) → Done.
- [ ] **Step 2: QR rendering.** Render the QR into a `SharedPixelBuffer<Rgba8Pixel>` at an integer scale filling about 400 px, with a quiet zone. Keep the Rust-owned byte buffer in `Zeroizing` and zeroize it on wipe.
- [ ] **Step 3: Done screen.** Show the fingerprint as 8 lowercase hex characters, the randomart in a monospace `Text`, and the copy "App buffers wiped". `finish()` and `back-home()` both call `Session::wipe()`. They also clear every secret-bearing AppState property (`page-words`, `quiz-choices`, `qr` set to an empty image).
- [ ] **Step 4: Verify in the simulator.** Do a full Arcade 12-word run. Answering the quiz wrongly loops back. Then cross-check the words: the fingerprint shown must equal the master fingerprint an offline tool (e.g. Sparrow) shows when it imports the same words. A quick check with the dev-only test vector is also fine.
- [ ] **Step 5: Commit:** `"feat: reveal, SeedQR, quiz, fingerprint, wipe"`

### Task 7: Health-test UI and haptics

**Files:**
- Create: `src/haptics.rs`
- Modify: `permission_templates.toml`, `app-config.toml`, `src/main.rs`, `ui/screens/painter.slint`, `ui/widgets/meter.slint`

**Interfaces:**
- Produces: `haptics::tick()`, `ramp()`, `full()`, `alert()`, all no-ops if the API connection fails. Every later game calls these on credit and on a source flipping unhealthy.

- [ ] **Step 1: Permissions.** Add a `[entropy-arcade]` template with `"os/haptics"` (the `Vibrate`-family message names from `~/.foundation/sdk/current/docs/api/v1.4.0/keyos-permissions.json`). Set `template = ["gui-app", "entropy-arcade"]`. The camera entry is added by Task 12.
- [ ] **Step 2: Haptics wrapper.** Use `haptics::use_api!` with `vibrate(HapticPattern::…)`:
  - `tick`: `SharpTickOne100`, every 8 credited bits
  - `ramp`: `TransitionRampUpShortSmoothOne0to100`, once at ≥90% full
  - `full`: `StrongClick100`
  - `alert`: `Alert750ms`, when a source flips to unhealthy
- [ ] **Step 3: Unhealthy state.** When `touch-healthy` is false, the meter's touch segment turns red and a toast reads "Touch rejected: keep moving".
- [ ] **Step 4: Verify in the simulator.** Hold the pointer still, and drag in a perfectly straight line with a script via `passport-sim` swipe. The toast appears and the meter stops. Real scribbling resumes the climb.
- [ ] **Step 5: Commit:** `"feat: live health rejection + haptics"`

### Task 8: "Neon on ink" theme and the game-cabinet Home

**Files:**
- Modify: `resources/theme.json` (through `foundation theme`; see the `/foundation-cli` skill), `ui/state.slint`, `ui/app.slint`, `ui/screens/home.slint`, `ui/widgets/meter.slint`, `ui/screens/summary.slint`, `src/main.rs`

**Interfaces:**
- Produces (AppState):
  - `Screen` gains `outguess` and `hunt`.
  - `start-arcade(int /*12|24*/, int /*game: 0 painter, 1 outguess, 2 hunt*/)` replaces `start-arcade(int)`.
  - Meter: `in property <int> game-bits`, `in property <bool> game-healthy`.
- Produces (Rust): nothing new in `Session`. The game choice is UI routing only.

- [ ] **Step 1: Palette.** Set the spec's "Neon on ink" values as the app theme: background `#0B0A12`, surface `#15131F`, raised `#1E1B2C`, line `#2C2840`, text `#F2F0FF`, muted `#9A94B8`, primary/success `#C6FF3D`, warning `#FFB020`, error `#FF4D6D`. Source colours that have no theme token (touch `#00E5FF`, camera `#FF2BD6`, game `#8B5CFF`) go in one Slint `global Neon` in `ui/state.slint`, used everywhere instead of literals.
- [ ] **Step 2: Home cabinet.** Title "Entropy Arcade", tagline "Can you trust one chip?", a 12/24-word toggle (default 24), and four cards: Chaos Painter, Outguess the Machine, Static Hunt, Dice Duel. Each card has a one-line hook in its source colour. Until Tasks 9 and 10 land, the Outguess and Hunt cards route to the Painter screen.
- [ ] **Step 3: Meter.** Three segments: touch (cyan), camera (magenta) and game (violet). The game segment shows `game-bits`. Summary gets a "Game skill" row, capped like the others so the shown total never exceeds the target.
- [ ] **Step 4: Verify in the simulator.** Screenshot Home and Painter, and check the colours against the hex values above.
- [ ] **Step 5: Commit:** `"feat: neon-on-ink theme and game cabinet"`

### Task 9: Outguess the Machine

**Files:**
- Create: `crates/entropy-core/src/outguess.rs`, `ui/screens/outguess.slint`
- Modify: `crates/entropy-core/src/{lib.rs,pool.rs,session.rs}`, `ui/state.slint`, `ui/app.slint`, `src/main.rs`

**Interfaces:**
- Produces (core):
  - `pub struct Oracle` (`Default`, `Clone`, `Zeroize`). It holds the player's door history, which is secret entropy material.
  - `Oracle::predict(&self) -> u8` returns 0..=3. It is pure, from history only.
  - `Oracle::record(&mut self, door: u8) -> Move`. The prediction is taken from `predict()` *before* the door is pushed, so it is committed by construction.
  - `Oracle::wipe(&mut self)`.
  - `pub struct Move { pub predicted: u8, pub hit: bool, pub credit_mbits: u32, pub read_pct: u8, pub healthy: bool }`
  - `EntropyPool::add_outguess(&mut self, door: u8, x: i32, y: i32, t_us: u64, credit_mbits: u32)` absorbs `TAG_OUTGUESS = 0x03` with `door ‖ x ‖ y ‖ t_us`, adds `credit_mbits` to `game_mbits`, and sets `game_ok = healthy`.
  - `EntropyPool::game_bits(&self) -> u32` is `game_mbits / 1000`. `credited_bits()` includes it. `EntropyPool::game_healthy(&self) -> bool`.
  - `Session::outguess_pick(&mut self, door: u8, x: i32, y: i32, t_us: u64) -> Move`
- Produces (AppState):
  - `callback pick-door(int, length, length)`
  - `in property <int> last-guess` (-1 means none yet), `in property <bool> last-hit`, `in property <string> last-credit` (e.g. "+1.4 bits"), `in property <int> read-pct`

Constants (in `outguess.rs`): `DOORS = 4`, `WINDOW = 32`, `WARMUP = 16`, `MARGIN_HITS = 4`, `MAX_MBITS = 2000`.

Predictor (Shannon mind-reading-machine style; ties break to the lowest door):

```
for k in [3, 2, 1]:
    if history.len() > k:
        ctx = last k doors
        counts[d] = how often ctx was followed by d earlier in history
        if any count > 0: return argmax(counts)
return most frequent door overall (0 if history is empty)
```

**Amendment (Task 9 review, 2026-10-30):** three experts, each committing a prediction from history before the door is pushed:
- **E1 doors:** the context predictor above, over the door sequence.
- **E2 offset:** over `r_t = (door_t − pred_{t−1}) mod 4`, where `pred` is the oracle's own revealed guess. Same k = 2/1/0 context counting over the `r` sequence (ties → lowest). Predict `(pred_{t−1} + r̂) mod 4`; with no previous guess, fall back to E1.
- **E3 win/lose:** over `δ_t = (door_t − door_{t−1}) mod 4` with context `(hit_{t−1}, δ_{t−1})`. Predict `(door_{t−1} + δ̂) mod 4`; with fewer than 2 doors, fall back to E1.
- The **shown/committed guess** (`Move::predicted`, `predict()`) is from whichever expert has the most hits in the last `WINDOW` moves (ties → E1, then E2, then E3).
- **Credit uses `hits` = the max over the three experts of their own hits in the window** (best in hindsight, ≥ the shown guess's hits), not the shown guess's hits. `read_pct` uses the same `hits`.

Credit per move: `n = min(moves so far including this one, WINDOW)`, `hits` = (per the amendment) the best expert's hits among the last `n`, and `p_ub = min(1, (hits + MARGIN_HITS) / n)`. If `moves ≤ WARMUP`, the move earns 0 and stays healthy. Otherwise, if `p_ub > 0.5` it earns 0 and is unhealthy, else `credit_mbits = min(MAX_MBITS, floor(−log2(p_ub) × 1000))`. `read_pct = round(100 × hits / n)`.

- [ ] **Step 1: Failing tests** in `outguess.rs`. Here `prng_doors(n)` is a test helper: door `i` is the first byte of `SHA-256(i as u32 LE)`, AND 3.

```rust
#[test] fn oracle_reads_constant_player() { let mut o = Oracle::default(); let moves: Vec<Move> = (0..64).map(|_| o.record(2)).collect(); assert_eq!(moves.iter().map(|m| m.credit_mbits).sum::<u32>(), 0); assert!(!moves.last().unwrap().healthy); }
#[test] fn oracle_reads_cycle() { let mut o = Oracle::default(); let moves: Vec<Move> = (0..64).map(|i| o.record((i % 4) as u8)).collect(); assert_eq!(moves[32..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0); }
#[test] fn oracle_warmup_earns_nothing() { let mut o = Oracle::default(); for d in prng_doors(16) { assert_eq!(o.record(d).credit_mbits, 0); } }
#[test] fn oracle_credits_random_player() { let mut o = Oracle::default(); let ms: Vec<Move> = prng_doors(256).into_iter().map(|d| o.record(d)).collect(); assert!(ms.iter().map(|m| m.credit_mbits).sum::<u32>() >= 150_000); assert!(ms.iter().all(|m| m.credit_mbits <= 2000)); }
#[test] fn oracle_reads_offset_from_last_guess() { for off in [1u8, 3] { let mut o = Oracle::default(); let mut last = 0u8; let ms: Vec<Move> = (0..400).map(|_| { let m = o.record((last + off) % 4); last = m.predicted; m }).collect(); assert_eq!(ms[64..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0, "offset {off}"); } }
#[test] fn oracle_reads_win_stay_lose_shift() { let mut o = Oracle::default(); let mut d = 0u8; let ms: Vec<Move> = (0..400).map(|_| { let m = o.record(d); if m.hit { d = (d + 1) % 4; } m }).collect(); assert_eq!(ms[64..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0); }
#[test] fn oracle_prediction_is_fixed_before_pick() { let mut o = Oracle::default(); for d in prng_doors(20) { o.record(d); } let p = o.predict(); for door in 0..4 { assert_eq!(o.clone().record(door).predicted, p); } }
#[test] fn oracle_wipe_forgets_history() { let mut o = Oracle::default(); for _ in 0..40 { o.record(1); } o.wipe(); assert_eq!(o.predict(), 0); assert_eq!(o.record(3).credit_mbits, 0); }
```

  In `pool.rs`: `game_credit_fills_meter` (64 × `add_outguess(.., 2000)` on Twelve gives `is_full()`) and `wipe_resets_game_credit`. In `session.rs`: `outguess_pick_feeds_pool` (after warm-up, a random sequence raises `pool().unwrap().game_bits()`).
- [ ] **Step 2: Run** `cargo test -p entropy-core --lib outguess`. Expected: FAIL (`Oracle` is not defined). Implement, then run the full `cargo test -p entropy-core --lib`. Expected: PASS.
- [ ] **Step 3: UI.** The meter sits on top, with a 2×2 grid of large doors in game violet. Each door's `TouchArea` calls `AppState.pick-door(i, abs-x, abs-y)`. **The guess is revealed only after the tap**: "Machine guessed 3 · MISS · +1.4 bits", or "READ YOU · +0" in red. A "The machine reads you N%" line follows, then Done once full (routes to Summary). Wire the Task 7 haptics: tick on credit, alert when it flips unhealthy. Never log doors, guesses or tap positions.
- [ ] **Step 4: Verify in the simulator.** Tap door 1 twenty-five times: "reads you" climbs past 50%, the game segment stops, and the alert fires. Then tap a varied sequence: the meter climbs again. Screenshot both states.
- [ ] **Step 5: Commit:** `"feat: Outguess the Machine"`

### Task 10: Static Hunt

**Files:**
- Create: `src/hunt.rs` (game state and software renderer; nothing secret lives here), `ui/screens/hunt.slint`
- Modify: `crates/entropy-core/src/{pool.rs,session.rs}`, `ui/state.slint`, `ui/app.slint`, `src/main.rs`

**Interfaces:**
- Produces (core):
  - `EntropyPool::add_shot(&mut self, miss_dx: i32, miss_dy: i32, t_us: u64) -> SampleResult` absorbs `TAG_SHOT = 0x04` and checks the offset with its own `TouchHealth` instance (`shot_health`). `Good` adds 1000 to `game_mbits`. `Unhealthy` sets `game_ok = false`.
  - **Amendment (Task 10 review, 2026-10-30):** `add_shot(&mut self, tap_x: i32, tap_y: i32, miss_dx: i32, miss_dy: i32, t_us: u64)` (and `Session::on_shot` likewise). It absorbs the tap position too, and runs the tap delta from the previous shot's tap through a second `TouchHealth` (`shot_tap_health`). A shot earns 1000 only when **both** checks are Good. The first shot after start or wipe has no previous tap and earns nothing. Test: `shot_fixed_tap_spot_earns_nothing`: 30 shots at tap (120, 200) with varied offsets give `game_bits() <= 1`. The view is **240×288 at 2× (576 px)**, so the whole screen fits in 800 px with no clipping.
  - `Session::on_shot(&mut self, miss_dx: i32, miss_dy: i32, t_us: u64) -> SampleResult`
- Produces (app): `hunt::Hunt` with `new(seed: u32)`, `turn(dx_px: i32)`, `tick(dt_ms: u32)`, `render(&mut SharedPixelBuffer<Rgb8Pixel>)` and `tap(x: i32, y: i32) -> Option<(i32, i32)>`. `tap` returns the miss offset from the nearest on-screen ghost centre, or `None` when no ghost is on screen.
- Produces (AppState): `in property <image> hunt-frame`, `callback hunt-press(length, length)`, `callback hunt-move(length, length)`, `callback hunt-release(length, length)`

Rules: the view is 240×320, scaled 2× under the meter. A press that moves more than 8 px is a drag: it turns the view at 0.5° per px, and each move event also goes to `session.on_touch` (the normal credited touch rule). A release within 8 px of its press is a shot: `tap` → `session.on_shot`. A shot within the ghost's half-width bursts it. Up to 3 ghosts spawn at random angles, distance 8, and approach at 1 unit/s. A ghost reaching you just fades out: no game over, no lost bits. The static and the spawns come from a xorshift32 seeded once from `hw_random()`, never from the pool.

- [ ] **Step 1: Renderer spike** (throwaway if it fails). In `hunt.rs`, raycast a fixed 16×16 room from its centre: one column per x, with walls textured by animated static. Drive it from a 33 ms `slint::Timer` into `hunt-frame`. Log the average render time every 100 frames, as `log::info!("screen hunt frame_ms={}", avg)` (a timing, not a secret). **Rule:** if the simulator average is ≤ 16 ms, keep first-person. Otherwise build the flat fallback, where ghosts drift across a full-screen static field with the same `tap`/`turn` API. Record the measured number and the choice in the report.
- [ ] **Step 2: Failing tests** in `pool.rs`:

```rust
#[test] fn shot_repeated_offset_earns_once() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..20 { p.add_shot(3, -2, i * 1000); } assert!(p.game_bits() <= 1); }
#[test] fn shot_bullseye_earns_nothing() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..20 { p.add_shot(0, 0, i * 1000); } assert_eq!(p.game_bits(), 0); }
#[test] fn shots_varied_credit_one_bit_each() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..10i32 { p.add_shot(i * 3 - 7, 11 - i * 5, i as u64 * 1000); } assert_eq!(p.game_bits(), 10); }
```

  Run `cargo test -p entropy-core --lib shot`. Expected: FAIL. Implement, then run the full suite. Expected: PASS.
- [ ] **Step 3: Game.** Ghosts are violet noise billboards. Add a crosshair, a burst flash on hit, a "+1 bit" pop on a credited shot, Task 7 haptics, and Done once full (routes to Summary). Never log tap positions or offsets.
- [ ] **Step 4: Verify in the simulator.** Drag to turn, tap ghosts to burst them, and watch the meter rise. Screenshot. Tap the same spot repeatedly with no ghost moving: it earns no shot credit.
- [ ] **Step 5: Commit:** `"feat: Static Hunt"`

### Task 11: Dice Duel

**Files:** Create `ui/screens/dice.slint`. Modify `crates/entropy-core/src/session.rs`, `src/main.rs` and `ui/state.slint`.

**Interfaces:**
- Consumes: `dice_entropy`, `min_rolls`, and the Words → Done flow from Task 6.
- Produces (AppState): `in property <int> roll-count, rolls-needed`, `in property <float> dice-bits`, `in property <string> rolls-text` (shown masked as "•••• 1 5"), with callbacks `roll(int)`, `undo-roll()`, `dice-done()`.
- Produces (Session): `start_dice(wc)`, `push_roll(u8)`, `pop_roll()`, and `generate_from_dice() -> Result<(), String>`.

- [ ] **Step 1: Test.** Add `#[test] fn dice_session_matches_core()`: push the 50-roll vector from Task 4, call `generate_from_dice()`, and check that the words equal `words(&dice_entropy(..))`.
- [ ] **Step 2: UI.** A 1–6 keypad of large `Button`s, with "x / 50" (or "x / 99"), bits = rolls × 2.585, Undo, and "Done" enabled at the minimum.
- [ ] **Step 3: Verify panel.** After Done, the same Words/QR/Quiz flow runs. The Done screen also shows a "Verify on a laptop" card with `echo -n "<your rolls>" | sha256sum`, and a note that the first 32 hex characters (12 words) or all 64 (24 words) are the entropy. Never display the actual rolls on this card.
- [ ] **Step 4: Verify in the simulator.** Tap the 50-roll vector, then confirm that `echo -n 12345612345612345612345612345612345612345612345612 | sha256sum` starts with `ee72ae915a4e6ea7ccbeb8e5e5eecef2`, matching what the app used.
- [ ] **Step 5: Commit:** `"feat: verifiable Dice Duel mode"`

### Task 12: LavaRand (camera) — cut first if time runs short

**Files:** Create `src/camera.rs`. Modify `src/main.rs`, `ui/screens/painter.slint`, `permission_templates.toml`.

**Interfaces:**
- Consumes: `EntropyPool::add_camera_frame`, and the `AppState.camera-on` / `toggle-camera()` from Task 5.
- Produces: `camera::start(ui: Weak<AppWindow>, session: Rc<RefCell<Session>>)`, `camera::stop()`, `Session::on_camera_frame(&mut self, frame: &[u8], t_us: u64) -> SampleResult`, and `in property <image> camera-preview` on AppState.

- [ ] **Step 1: Permission and subscribe.** Add `"os/camera" = ["Subscribe", "SetEnabled", "NotifyVisible", "IsEnabled", "IsInUse"]` to the `[entropy-arcade]` template. Use `camera::use_api!()`, then `slint_keyos_platform::try_subscribe_scalar::<camera_permissions::CameraPermissions, camera::messages::Subscribe>(Subscribe)` and loop over `next().await` in `spawn_local`, as in `src/theme.rs`. Read `frame.content()` as bytes: RGBA on the simulator, RGB565 on hardware (`camera::CAMERA_BYTES_PER_PX`). Call `set_enabled(true)` / `notify_visible(true)` when toggled on, and `false` when toggled off or the screen is left.
- [ ] **Step 2: Feed and preview.** Pass each frame to `session.on_camera_frame(bytes, t_us)`. Build a 120×120 downscaled preview where each pixel is the XOR of the low 2 bits with the previous frame, scaled to magenta intensity. This is the "noise" visual; the raw image is not shown.
- [ ] **Step 3: Verify in the simulator** with the webcam. With the camera on and no touch, the meter doesn't move. Camera on while scribbling fills faster, and the camera segment stops at 50%. Covering the webcam still gives noise; freezing it gets it flagged.
- [ ] **Step 4: Commit:** `"feat: LavaRand camera entropy"`

### Task 13: Polish and demo

**Scope update (2026-10-31):** Finish the project and the user's additional ideas first. Video recording, narration, and submission preparation are deferred at the user's request.

**Files:** Modify `ui/screens/*`, `ui/widgets/meter.slint`, `resources/icon.svg` and `resources/icon-dark.svg`.

- [ ] **Step 1: Glow and motion.** Glow on trail dots, doors and ghosts (2–3 layered rounded rectangles with decreasing opacity), and animated meter width (`animate width { duration: 120ms; }`).
- [ ] **Step 2: Home screen.** Add a "How it works" `Sheet` with these 5 short lines:
  - "We mix your chaos with the chip's randomness: SHA-256(chip + you)."
  - "Even if you play badly, it's never weaker than the chip."
  - "Credits are conservative estimates; stale input is rejected live."
  - "Outguess and Static Hunt only pay for what the machine can't predict."
  - "Dice Duel uses only your dice, so you can verify it on a laptop."
- [ ] **Step 3: App icon.** A neon die with a spark, on ink, in cyan and lime.
- [ ] **Step 4: Check the game flows** in `foundation sim`, with screenshots as needed:
  1. The hook: "Can you trust one chip?"
  2. Scribble while the meter climbs and haptic ticks fire. Hold still and get rejected.
  3. Outguess: a lazy pattern gets read ("reads you 70%"), then real play pays.
  4. Static Hunt: a few blasts.
  5. Fill the meter, then show the summary, words, SeedQR, quiz, and the fingerprint with its art.
  6. Dice Duel with the laptop check.
- [ ] **Step 5: Final check.** Run `cargo test -p entropy-core --lib` (all pass), then `grep -rn "log::" src | grep -v -E "info!\(\"(screen|bits|health)"` and review that nothing secret is logged.
- [ ] **Step 6: Commit** with `"feat: arcade polish"`. The submission email is sent by the user (ledger Ruling D).


### Task 14: Glitch Glider (and the shared MCV estimator)

**Files:**
- Create: `crates/entropy-core/src/mcv.rs`, `crates/entropy-core/src/glide.rs`, `src/glide.rs` (flight physics and course; nothing secret), `ui/screens/glide.slint`
- Modify: `crates/entropy-core/src/{lib.rs,pool.rs,session.rs}`, `ui/state.slint`, `ui/app.slint`, `ui/screens/home.slint`, `src/main.rs`

**Interfaces:**
- Produces (core, `mcv.rs`). Task 15 reuses these:
  - `pub struct Estimate { pub credit_mbits: u32, pub healthy: bool }`
  - `pub struct Mcv` (`Zeroize`; it holds a symbol window, which is entropy material), built with `Mcv::new(warmup: usize, cap_mbits: u32)`.
  - `Mcv::push(&mut self, symbol: u16) -> Estimate`
  - `Mcv::wipe(&mut self)`
  - Constants `MCV_WINDOW = 64`, `MCV_Z = 2.576`, `MCV_REPEAT = 5`.
  - Rule: append the symbol and keep the last `n ≤ 64`. The move counter includes this symbol.
    - If moves ≤ warmup: 0, healthy.
    - Else if the last 5 symbols are identical: 0, unhealthy.
    - Else `p̂ = max_count / n`, `p_u = min(1, p̂ + 2.576·√(p̂(1−p̂)/(n−1)))`, and credit `= min(cap, floor(−log2(p_u)·1000))`, healthy.
- Produces (core, `glide.rs`):
  - `pub struct GlideTimer` (`Zeroize`) with `tap(&mut self, t_us: u64) -> Option<u16>`.
  - It keeps the last 16 intervals.
  - It returns `None` on the first tap, until 16 intervals exist, and after a pause (interval > 1.5 s, which also resets the baseline).
  - Otherwise `r = interval − median(last 16 intervals before this one)` in µs, and the symbol is `clamp(r / 4000, −16, 16) + 16` (0..=32).
- Produces (core, pool/session):
  - `EntropyPool::add_glide(&mut self, x: i32, y: i32, t_us: u64, credit_mbits: u32, healthy: bool)` absorbs `TAG_GLIDE = 0x05` with `x ‖ y ‖ t_us`, adds to `game_mbits`, and sets `game_ok`.
  - Refactor `add_outguess` and `add_glide` through one private helper; the absorb format is unchanged.
  - `Session::glide_tap(&mut self, x: i32, y: i32, t_us: u64) -> Estimate` uses `Mcv::new(32, 2000)`. Every tap is absorbed, including ones with no symbol (credit 0, healthy).
  - `wipe` resets the timer and the Mcv.
- Produces (AppState): `Screen.glide`, `callback glide-tap(length, length)`, `in property <length> glider-y`, `in property <[{x: length, gap-y: length}]> pillars`, `in property <string> glide-note` (e.g. "+1.2 bits" / "Too steady: vary your timing").

Course rules (`src/glide.rs`):
- Constant scroll speed and pillar spacing, with every gap at the same height. **No randomness in the geometry** (spec Goal 12).
- Gravity, plus a fixed upward pulse per tap.
- Hitting a pillar resets the spark to the middle. No game over, and no bits lost.
- A 16 ms `slint::Timer`, stopped on leave, Done and new run (same lifecycle as Static Hunt).

- [ ] **Step 1: Failing tests.** `mcv.rs`:

```rust
#[test] fn mcv_constant_symbol_earns_nothing() { let mut m = Mcv::new(16, 2000); let es: Vec<Estimate> = (0..100).map(|_| m.push(7)).collect(); assert_eq!(es.iter().map(|e| e.credit_mbits).sum::<u32>(), 0); assert!(!es.last().unwrap().healthy); }
#[test] fn mcv_warmup_earns_nothing() { let mut m = Mcv::new(16, 2000); for s in 0..16u16 { assert_eq!(m.push(s).credit_mbits, 0); } }
#[test] fn mcv_matches_formula() { let mut m = Mcv::new(0, 3000); let mut last = None; for i in 0..64u16 { last = Some(m.push(if i % 4 == 0 { 0 } else { 1 + i % 13 })); } assert_eq!(last.unwrap().credit_mbits, 1356); } // max count 16 of 64 → p_u ≈ 0.3905
#[test] fn mcv_uniform_symbols_earn() { let mut m = Mcv::new(16, 2000); let total: u32 = (0..200u32).map(|i| m.push((Sha256::digest(i.to_le_bytes())[0] % 16) as u16).credit_mbits).sum(); assert!(total >= 150_000); }
```

  In `mcv_matches_formula`, make sure no symbol other than 0 reaches 16 occurrences and no 5-in-a-row happens. Adjust the filler sequence if needed, but keep the asserted 1356. `glide.rs`:

```rust
#[test] fn glide_metronome_earns_nothing() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); let total: u32 = (0..120u64).map(|i| s.glide_tap(100, 400, i * 400_000).credit_mbits).sum(); assert_eq!(total, 0); }
#[test] fn glide_steady_drift_earns_nothing() { /* intervals 400 ms + 2 ms per tap (deterministic ramp), 120 taps → total 0 */ }
#[test] fn glide_frame_quantised_jitter_is_bounded() { /* intervals 400 ms + k·16_667 µs, k ∈ {−1,0,1} from a SHA-256 counter, 200 taps → every credit ≤ 1585 mbits (log2 3: three possible values can't carry more) and total > 0 */ }
#[test] fn glide_human_jitter_earns() { /* intervals 400 ms + (sum of 4 uniform SHA-256 bytes − 510)·80 µs (≈ ±20 ms bell), 200 taps → total ≥ 100_000 mbits */ }
#[test] fn glide_pause_resets_baseline() { /* 20 steady taps, a 3 s pause, then the next 16 taps return no symbol (credit 0, healthy) */ }
```

  Plus `wipe_resets_glide` in `session.rs`.
- [ ] **Step 2: Run** `cargo test -p entropy-core --lib mcv glide`. Expected: FAIL (types missing). Implement, then run the full `cargo test -p entropy-core --lib` (warning-free). Expected: PASS.
- [ ] **Step 3: UI and Home.**
  - Home: switch the cards to a 2-column grid of compact tiles: title plus a one-line hook in the source colour. There are six modes now (Painter, Outguess, Hunt, Glider, Shard (routes to Painter until Task 15), Dice), and Home plus the How-it-works button must still fit 800 px.
  - Glider card hook: "Tap to fly. Your timing wobble is the noise."
  - `start-arcade(words, 3)` routes to `Screen.glide`.
  - Glide screen:
    - The meter is on top. A full-screen `TouchArea` calls `glide-tap(mouse-x, mouse-y)`, and Rust stamps `t_us` with the shared `Instant` at the callback.
    - The spark is drawn in `Neon.game` with a glow, and the pillars as static-textured rectangles.
    - `glide-note` shows the last credit, or "Too steady: vary your timing" while unhealthy.
    - Done once full, and Home → `back-home()`.
  - Haptics go through the existing `credit()`/`Feedback` path. Never log tap times or intervals.
- [ ] **Step 4: Verify in the simulator.**
  - Script about 60 taps at a fixed 400 ms with `passport-sim` tap: credit stays 0 and the note says too steady.
  - Then tap by hand-varied timing: the meter climbs.
  - Report the observed credit per tap and whether the sim's touch timestamps look frame-quantised (bins hit).
  - Screenshot.
- [ ] **Step 5: Commit:** `"feat: Glitch Glider with MCV timing credit"`

### Task 15: Shard Stack (cut first if time runs short)

**Files:**
- Create: `crates/entropy-core/src/shard.rs`, `ui/screens/shard.slint`
- Modify: `crates/entropy-core/src/{lib.rs,pool.rs,session.rs}`, `ui/state.slint`, `ui/app.slint`, `ui/screens/home.slint`, `src/main.rs`

**Interfaces:**
- Consumes: `Mcv`, `Estimate` (Task 14).
- Produces (core, `shard.rs`, all `Zeroize`, since the board encodes the player's choices):
  - `pub struct Board` (8 wide × 16 tall, a `[u8; 16]` row bitmask).
  - `pub const SHAPES: [&[(u8, u8)]; 6]`: two 3-cell shapes (straight, corner) and four 5-cell shapes (P, U, T, S-like). Rotations are generated by 90° turns with duplicates removed.
  - `Board::placements(&self, piece: u8) -> Vec<(u8, u8)>` lists legal (rotation, column) hard-drop placements, in a fixed order.
  - `Board::place(&mut self, piece: u8, rot: u8, col: u8) -> Option<u8>` returns the lines cleared, or `None` if the placement is illegal.
  - `Board::score_after(&self, piece, rot, col) -> f32` uses aggregate height −0.51, lines +0.76, holes −0.36, bumpiness −0.18.
  - `Board::rank(&self, piece, rot, col) -> u8` is the number of legal placements with a strictly higher score, capped at 15.
  - `Board::cells(&self) -> [u8; 128]`.
  - When a spawned piece has no legal placement, the board clears ("overflow"): no game over, and no bits lost.
- Produces (core, pool/session):
  - `EntropyPool::add_shard(&mut self, piece: u8, rot: u8, col: u8, t_us: u64, credit_mbits: u32, healthy: bool)` absorbs `TAG_SHARD = 0x06`, through the shared helper.
  - `Session::shard_spawn(&mut self, piece: u8)`. The piece comes from the app's game PRNG and is **never credited**.
  - `Session::shard_place(&mut self, rot: u8, col: u8, t_us: u64) -> Option<Estimate>` ranks the placement *before* placing it, pushes the rank into `Mcv::new(16, 3000)`, absorbs it, then places the piece.
  - `Session::shard_cells(&self) -> [u8; 128]`. `wipe` zeroizes the board and the Mcv.
- Produces (AppState): `Screen.shard`, `in property <[int]> shard-cells` (128 cells), `in property <[int]> ghost-cells` (the falling piece's preview), and `callback shard-move(int)` (column delta), `shard-rotate()`, `shard-drop()`.

- [ ] **Step 1: Failing tests** in `shard.rs`:

```rust
#[test] fn shard_line_clears() { /* fill a row except one gap, place a piece that fills it → place() returns Some(1) and the row is gone */ }
#[test] fn shard_rank_of_best_is_zero() { let b = Board::default(); let (r, c) = best placement by score_after; assert_eq!(b.rank(0, r, c), 0); }
#[test] fn shard_illegal_placement_rejected() { /* column past the right edge → place() returns None */ }
#[test] fn shard_best_play_earns_nothing() { /* Session: 120 pieces from a SHA-256 counter, always placing the rank-0 placement → total credit 0 */ }
#[test] fn shard_varied_play_earns() { /* same pieces, choose the placement whose rank = (SHA-256 counter byte % 8) when available → total ≥ 100_000 mbits */ }
#[test] fn shard_piece_sequence_is_not_credited() { /* identical choices (always rank 0) under two different piece sequences → both earn 0 */ }
#[test] fn wipe_clears_board() { /* place pieces, wipe, cells all zero */ }
```

- [ ] **Step 2: Run** `cargo test -p entropy-core --lib shard`. Expected: FAIL. Implement, then run the full suite (warning-free). Expected: PASS.
- [ ] **Step 3: UI.**
  - The 8×16 grid is drawn as `for` rectangles, with filled cells in `Neon.game`, the falling piece outlined, and the meter on top.
  - Controls: horizontal drag moves one column per 52 px, a tap rotates, and a Drop button (or a downward swipe) places the piece. The piece sits at the top until dropped (no gravity timer), so the choice is deliberate.
  - The next piece comes from a xorshift32 seeded once from `hw_random()`, never from the pool.
  - A note shows "+N bits" or "Too predictable: try other spots" while unhealthy.
  - Card hook: "Stack shards. Surprise the solver."
  - Done once full, and Home → `back-home()`. Never log pieces, placements or ranks.
- [ ] **Step 4: Verify in the simulator.** Play about 25 pieces always into the left-most legal slot, then vary: report credits and screenshots.
- [ ] **Step 5: Commit:** `"feat: Shard Stack with placement-rank credit"`

---

## Time budget

- **Event (70 min):** Tasks 1–3, plus starting Task 5. Tasks 1 and 5 alone give a demoable core.
- **Overnight:** Tasks 5–15, in that order. If time runs short, cut from the bottom: LavaRand (12) first, then fall back to flat Static Hunt, then polish.
