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
- Credits: 1 bit per healthy touch sample. 4 bits per healthy camera frame, only if a touch sample arrived ≤250 ms earlier, with camera total capped at `target_bits / 2`. Targets are 256 bits (24 words) or 128 bits (12 words).
- UI copy: the wipe message reads **"App buffers wiped"**, never "all memory". Credits are labelled as **conservative estimates**.
- Stay on `getrandom = "0.2"` (the SDK patch covers only 0.2).
- Screen 480×800, portrait. Use `@ui/*` kit components and `Theme.*` tokens where they exist.

## Review Focus

1. **Scribbling in a straight line or tapping one spot.** Repeated identical deltas must earn 0 credit and eventually flag the source unhealthy, never crash or stall the meter forever once real scribbling resumes. Test: `touch_recovers_after_unhealthy` (Task 2).
2. **Camera on while not touching.** The meter must not move. Test: `camera_needs_recent_touch` (Task 1).
3. **Leaving a screen mid-flow (back or exit) after words were shown.** Secrets must be wiped. Test: `session_wipe_clears_secrets` (Task 5).
4. **The hardware RNG failing (`getrandom` returns Err).** Generation must stop with an error screen, never fall back to user-only entropy in Arcade mode. Test: `finish_requires_hw_random` (Task 5).
5. **A dice roll outside 1..=6, or too few rolls.** These must be rejected and never produce a seed. Test: `dice_rejects_bad_input` (Task 4).

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
  src/main.rs                   # app_main: wiring callbacks -> Session -> AppState
  src/session.rs                # Session state machine, holds Zeroizing secrets, wipe()
  src/camera.rs                 # subscribe frames -> pool + preview image
  src/haptics.rs                # tiny wrapper: tick/ramp/full/alert
  src/theme.rs                  # (scaffolded, unchanged)
  ui/app.slint                  # AppWindow: switch on AppState.screen
  ui/state.slint                # global AppState (props + callbacks), Screen enum
  ui/screens/home.slint, painter.slint, summary.slint, words.slint,
             seedqr.slint, quiz.slint, done.slint, dice.slint, error.slint
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
- Produces: `haptics::tick()`, `ramp()`, `full()`, `alert()`, all no-ops if the API connection fails.

- [ ] **Step 1: Permissions.** Add a `[entropy-arcade]` template with `"os/haptics"` (the `Vibrate`-family message names from `~/.foundation/sdk/current/docs/api/v1.4.0/keyos-permissions.json`), and later `"os/camera" = ["Subscribe", "SetEnabled", "NotifyVisible", "IsEnabled", "IsInUse"]`. Set `template = ["gui-app", "entropy-arcade"]`.
- [ ] **Step 2: Haptics wrapper.** Use `haptics::use_api!` with `vibrate(HapticPattern::…)`:
  - `tick`: `SharpTickOne100`, every 8 credited bits
  - `ramp`: `TransitionRampUpShortSmoothOne0to100`, once at ≥90% full
  - `full`: `StrongClick100`
  - `alert`: `Alert750ms`, when a source flips to unhealthy
- [ ] **Step 3: Unhealthy state.** When `touch-healthy` is false, the meter's touch segment turns red and a toast reads "Touch rejected: keep moving".
- [ ] **Step 4: Verify in the simulator.** Hold the pointer still and drag in a perfectly straight line with a script via `passport-sim` swipe. The toast appears and the meter stops. Real scribbling resumes the climb.
- [ ] **Step 5: Commit:** `"feat: live health rejection + haptics"`

### Task 8: LavaRand (camera)

**Files:** Create `src/camera.rs`. Modify `src/main.rs` and `ui/screens/painter.slint`.

**Interfaces:**
- Consumes: `EntropyPool::add_camera_frame`, and the `AppState.camera-on` / `toggle-camera()` from Task 5.
- Produces: `camera::start(ui: Weak<AppWindow>, session: Rc<RefCell<Session>>)`, `camera::stop()`, and `in property <image> camera-preview` on AppState.

- [ ] **Step 1: Subscribe.** Use `camera::use_api!()`, then `slint_keyos_platform::try_subscribe_scalar::<camera_permissions::CameraPermissions, camera::messages::Subscribe>(Subscribe)` and loop over `next().await` in `spawn_local`, as in `src/theme.rs`. Read `frame.content()` as bytes: RGBA on the simulator, RGB565 on hardware (`camera::CAMERA_BYTES_PER_PX`). Call `set_enabled(true)` / `notify_visible(true)` when toggled on, and `false` when toggled off or the screen is left.
- [ ] **Step 2: Feed and preview.** Pass each frame to `session.on_camera_frame(bytes, t_us)`. Build a 120×120 downscaled preview where each pixel is the XOR of the low 2 bits with the previous frame, scaled to neon intensity. This is the "noise" visual; the raw image is not shown.
- [ ] **Step 3: Verify in the simulator** with the webcam. With the camera on and no touch, the meter doesn't move. Camera on while scribbling fills faster, and the camera segment stops at 50%. Covering the webcam still gives noise; freezing it (unplug, or a static virtual cam) gets it flagged.
- [ ] **Step 4: Commit:** `"feat: LavaRand camera entropy"`

### Task 9: Dice Duel

**Files:** Create `ui/screens/dice.slint`. Modify `src/session.rs`, `src/main.rs` and `ui/state.slint`.

**Interfaces:**
- Consumes: `dice_entropy`, `min_rolls`, and the Words → Done flow from Task 6.
- Produces (AppState): `in property <int> roll-count, rolls-needed`, `in property <float> dice-bits`, `in property <string> rolls-text` (shown masked as "•••• 1 5"), with callbacks `roll(int)`, `undo-roll()`, `dice-done()`.
- Produces (Session): `start_dice(wc)`, `push_roll(u8)`, `pop_roll()`, and `generate_from_dice() -> Result<(), String>`.

- [ ] **Step 1: Test.** Add `#[test] fn dice_session_matches_core()`: push the 50-roll vector from Task 4, call `generate_from_dice()`, and check that the words equal `words(&dice_entropy(..))`.
- [ ] **Step 2: UI.** A 1–6 keypad of large `Button`s, with "x / 50" (or "x / 99"), bits = rolls × 2.585, Undo, and "Done" enabled at the minimum.
- [ ] **Step 3: Verify panel.** After Done, the same Words/QR/Quiz flow runs. The Done screen also shows a "Verify on a laptop" card with `echo -n "<your rolls>" | sha256sum`, and a note that the first 32 hex characters (12 words) or all 64 (24 words) are the entropy. Never display the actual rolls on this card.
- [ ] **Step 4: Verify in the simulator.** Tap the 50-roll vector, then confirm that `echo -n 12345612345612345612345612345612345612345612345612 | sha256sum` starts with `ee72ae915a4e6ea7ccbeb8e5e5eecef2`, matching what the app used.
- [ ] **Step 5: Commit:** `"feat: verifiable Dice Duel mode"`

### Task 10: Polish and demo

**Files:** Modify `resources/theme.json` (via `foundation theme`), the `ui/screens/*`, `resources/icon.svg` and `resources/icon-dark.svg`.

- [ ] **Step 1: Arcade theme.** Near-black background, cyan `#00E5FF` and magenta `#FF2BD6` accents, glow on trail dots (layered rounded rectangles with decreasing opacity), animated meter width (`animate width { duration: 120ms; }`).
- [ ] **Step 2: Home screen.** Add a "How it works" `Sheet` with 4 short lines:
  - "We mix your chaos with the chip's randomness: SHA-256(chip ‖ you)."
  - "Even if you scribble badly, it's never weaker than the chip."
  - "Credits are conservative estimates; stale input is rejected live."
  - "Dice Duel uses only your dice, so you can verify it on a laptop."
- [ ] **Step 3: App icon.** A neon die with a spark.
- [ ] **Step 4: Rehearse the demo** in `foundation sim` using the built-in recorder. Script, about 90 s:
  1. The hook: "Can you trust one chip?"
  2. Scribble while the meter climbs and haptic ticks fire.
  3. Hold still and get rejected.
  4. Switch on LavaRand for the combo.
  5. Fill the meter, then show the summary, words, SeedQR, quiz, and the fingerprint with its art.
  6. Dice Duel with the laptop check.
- [ ] **Step 5: Final check.** Run `cargo test -p entropy-core` (all pass), then `grep -rn "log::" src | grep -v -E "info!\(\"(screen|bits|health)"` and review that nothing secret is logged.
- [ ] **Step 6: Commit and submit.** Commit with `"feat: arcade polish"`, then email the video to ken@foundation.xyz with the subject `Bitmela App: Entropy Arcade` before 10:00.

---

## Time budget

- **Event (70 min):** Tasks 1–3, plus starting Task 5. Tasks 1 and 5 alone give a demoable core.
- **Overnight:** Tasks 5–10, in that order. If time runs short, cut from the bottom: Task 9 before Task 8, and polish last.
