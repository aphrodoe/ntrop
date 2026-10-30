# Entropy Arcade — Design

Date: 2026-10-30 · Bitmela Hackathon (Foundation / Passport Prime) · Deadline: video emailed by 10:00, 2026-10-31

## Intent

A seed-phrase generator for Passport Prime that you *play*. Users feed their own randomness
into the seed through touch and camera "games", the app mixes it with the device's hardware
RNG, and it is radically honest about how much entropy each source actually contributed.

**Why:** trusting one chip's RNG is a single point of failure. Users can add their own
entropy as defense in depth, and the app makes that entropy visible, fun and trustworthy.
It has to feel like a game and survive a security engineer's review. The judges build
hardware wallets, so "entropy theater" loses.

**Output:** a BIP39 phrase (and SeedQR) for setting up *other* wallets. It does not
replace the Prime's own master seed.

**Prize targets:** Most unexpected/innovative first, Most polished second.

## Goals

1. **Chaos Painter (hero, must-have).** Free-draw with glowing neon trails. Every touch-move
   sample (x, y, monotonic timestamp in µs) is absorbed into the entropy pool.
2. **LavaRand (must-have if the camera works in the sim, otherwise the first stretch goal).**
   Point the camera at anything. Sensor noise from raw frames is absorbed. Named after
   Cloudflare's lava-lamp wall.
3. **Entropy meter.** One bar filling to the target (256 bits for 24 words), with a
   per-source breakdown and honest per-sample credit shown on screen. Haptic and visual
   feedback as it fills.
4. **Live health tests.** Each source is checked continuously. A finger held still or a
   frozen/covered-and-static camera gets the source flagged *unhealthy* on screen and
   credited 0 until it recovers. This is a planned demo moment.
5. **Mixing that can't make things worse:**
   `final = SHA-256("entropy-arcade/v1" ‖ hw_random[32] ‖ pool_digest[32])`, truncated
   to 256 bits (24 words) or 128 bits (12 words). `hw_random` comes from
   `getrandom` 0.2, which the scaffold patches to route to the device TRNG
   (`security::Security::get_random()` is the equivalent direct API). `pool_digest` is a running SHA-256 over every raw
   sample, each prefixed with a source tag byte and length. Even if all user input were
   zero, the result is as strong as the hardware RNG.
6. **Reveal.** BIP39 words (paged, large type), a Standard SeedQR (4-digit word indices),
   and the **BIP32 master fingerprint** (8 hex chars, the same value Sparrow and other
   wallets show after import) plus a randomart image derived from that fingerprint, so
   the user can confirm another wallet imported the right seed.
7. **Backup quiz.** Before finishing, the user answers 3 randomly chosen positions
   ("Word #7?") by **picking from 4 in-app choices** (1 correct + 3 random BIP39 decoys).
   It uses no system keyboard, so no seed words pass through the gui-server's keyboard.
   A wrong answer sends them back to the words.
8. **Wipe.** On finish or app exit, every buffer the app owns (entropy, pool, words, seed,
   QR pixel buffer) is zeroized (`zeroize` crate). Nothing secret is ever written to
   storage or logs. Slint string and render caches can't be guaranteed wiped, so the UI
   says **"App buffers wiped"**, never "all memory wiped". The SeedQR is rendered by a
   pure-Rust QR crate into a pixel buffer the app owns (the SDK has no QR widget).
9. **Dice Duel (verifiable mode, should-have).** Enter physical d6 rolls (1–6). The seed is
   **only** `SHA-256(ASCII string of rolls)`, the Coldcard-compatible method: 50 rolls for
   12 words, 99 rolls for 24. Show the laptop one-liner to verify it:
   `echo -n "<rolls>" | sha256sum`, then convert the hash to BIP39 with any offline tool.
   No hardware RNG is mixed in here, by design. That is what makes it verifiable.

## Entropy accounting (conservative; shown in-app)

| Source | Credit | Notes |
|---|---|---|
| Touch move sample | 1 bit per sample that passes the health check | Raw sample (x, y, Δt µs) all hashed in. Credit is only a pessimistic min-entropy estimate. |
| Camera frame | 4 bits per frame that passes the health check, **credited only while the user is drawing** (a touch sample in the last 250 ms), and **capped at 50% of the target** | Every frame is hashed in regardless. "Combo" mechanic: LavaRand boosts drawing but can never fill the meter on its own (no waiting-to-fill). |
| Dice roll | log2(6) ≈ 2.585 bits | Exact value, dice mode only. |
| Hardware RNG | Not counted on the meter | Always mixed in, shown as a fixed "+ hardware 256" badge. |

The meter shows user entropy only. The point is that the user's contribution alone reaches
the target.

## Health tests (inspired by NIST SP 800-90B, simplified)

- **Touch, repetition count:** a sample whose (Δx, Δy) is (0, 0), or which equals any of
  the last 4 deltas (catches two-spot tapping and short cycles), earns no credit. After 8 consecutive bad samples the source is
  flagged unhealthy until movement resumes.
- **Touch, adaptive proportion:** in a window of 64 samples, if one (Δx, Δy) pair
  accounts for more than 50% of them, flag the source (catches dominant repeated
  input; long scripted cycles can still pass, which is why credit is a conservative estimate
  and the hardware RNG is always mixed in). Δt is measured with `Instant::now()` in the callback and hashed
  in, but it is **not** health-tested or credited. Slint gives no hardware timestamp, and
  events are likely quantized to the frame rate, so timing-jitter claims would be dishonest.
- **Camera, stuck frame:** if a frame's hash equals the previous frame's, or the mean
  absolute difference of the low bits versus the previous frame is below a threshold,
  credit 0. After 5 consecutive failures, flag unhealthy.
- Thresholds are constants in one place, so they can be tuned in the simulator.

Unhealthy data is still hashed into the pool (it can't hurt). It just earns no credit.

## Screens (480×800, portrait)

1. **Home.** Title, the "Can you trust one chip?" tagline, and buttons: *Arcade (24 words)*,
   *Arcade (12 words)*, *Dice Duel*, plus a short "How it works".
2. **Chaos Painter.** Full-screen canvas, neon trails, the meter at the top, a "LavaRand"
   switch to add the camera, and Done (enabled once the meter is full).
3. **LavaRand.** Live camera preview with a noise-visualisation overlay and the same meter.
   May be merged into screen 2 as a toggle (builder's call).
4. **Mix summary.** Per-source bits credited, health incidents, "+ hardware 256", and a
   Generate button.
5. **Words.** 6 words per page, numbered, with a "never photograph this" warning.
6. **SeedQR.** Full-screen QR.
7. **Quiz.** 3 random word positions.
8. **Done.** Master fingerprint, randomart, a "Secrets wiped from memory" confirmation,
   and New / Exit.
9. **Dice Duel.** A 1–6 keypad, roll counter (x/50 or x/99), live bit count, Undo, then the
   Words, SeedQR, Quiz and Done flow, plus a "Verify on a laptop" panel showing the
   one-liner.

**Visual direction:** dark "arcade" theme with neon accents (cyan/magenta), rounded type,
smooth meter animation. Use the SDK UI kit components and theme where they exist.
**Haptics:** a soft tick each time 8 bits accrue, a ramp-up near full, a strong click at
100%, and an alert pattern on a health failure.

## Non-goals

- Storing seeds, signing, or acting as a wallet. Replacing the Prime's master seed.
- Networking of any kind (QuantumLink is out of scope today anyway).
- Claiming certified or measured min-entropy. The credits are labelled as conservative estimates.
- Passphrase (BIP39 25th word) support.

## Anti-goals (a failure even if it "works")

- Inflated entropy credit, or a meter that fills by just waiting. That is theater.
- Any secret in logs (`foundation logs` is visible), files, settings or crash output.
- Hardware RNG *replaced* (rather than mixed) in Arcade mode.
- A game slow or fiddly enough to annoy. The target is a full meter in about 5–10 s of scribbling.
- A demo that crashes, or an unpolished look.

## Constraints

- KeyOS SDK v1.1.0 / KeyOS v1.4.0, Rust (std) + Slint, scaffolded with `foundation new`.
  Pure-Rust crates preferred.
- The demo runs in `foundation sim`. The webcam stands in for the camera.
- APIs confirmed in the SDK docs:
  - `security::Security::get_random() -> Result<[u8; 32], AccessDenied>`
  - `camera::CameraApi::subscribe` with `Frame` events, 480×480 (needs the
    `peripherals.camera-use` permission, granted on first use)
  - `haptics::HapticPattern` (about 130 patterns, including ramps)
  - `security::parse_seedqr` (parse only; generating SeedQR digits is trivial)
  - The UI kit lists QR display.
- Timeline: about 70 min at the event, plus overnight. A video under about 2 min is emailed
  to ken@foundation.xyz with the subject `Bitmela App: Entropy Arcade`.

## Build order (priority)

1. Entropy pool, mixing, BIP39, and a Chaos Painter canvas with a working meter. **This alone is demoable.**
2. Words, Quiz, Done, wipe.
3. Touch health tests and their on-screen rejection.
4. SeedQR, master fingerprint and randomart.
5. LavaRand.
6. Dice Duel and its verify panel.
7. Polish: theme, animations, haptics, and the "How it works" screen.
8. (Stretch) Chaos Plinko.

## Decided by the user

- Entropy Arcade concept, combining several play sources with mixing.
- A design doc first.

## Calls made by Claude (open to change)

- Chaos Painter is the hero (it relies only on touch input, so it's the least risky with the SDK).
- The specific credit rates, health-test thresholds and domain tag above.
- Dice Duel uses Coldcard's SHA-256-of-rolls method, for compatibility with existing verify tools.
- Master fingerprint plus randomart as the "seed fingerprint".
- App name: "Entropy Arcade".
- Camera credit counts only while drawing and is capped at 50% of the target ("combo" boost), so the meter never fills by waiting.
- Quiz uses 4-way multiple choice, not the system keyboard. The UI claims "App buffers wiped", not "all memory".
- Touch credit is based on coordinates only. Δt is hashed in but not credited or tested (no hardware timestamps).

## Left to the builder

- Exact Slint component structure, routing, and animation details.
- Whether LavaRand is a separate screen or a toggle on the canvas.
- Choice of BIP39/BIP32 crates (e.g. `bip39`; `bitcoin` or `k256` for the fingerprint).
- Randomart algorithm (drunken bishop is fine).
