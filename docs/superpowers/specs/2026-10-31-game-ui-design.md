# Game UI and playability

The game rules are fixed. This pass changes presentation and input ergonomics while preserving the approved iPhone OS 1 chrome, original art, and neon source colours.

The original Passport Prime problem statement sets a 480×800 touchscreen, 500 MHz CPU and 128 MB RAM. Keep rendering bounded and local, use the existing Rust/Slint SDK, and add no permissions, networking, or persistence. The software line renderer requires solid rectangles and supported image assets rather than paths, transforms, or expensive effects.

- Use a compact, consistent progress HUD. Keep conservative-estimate labels and the existing full-meter Done gate.
- Give Painter a dedicated drawing area, unobstructed trails, and compact health/camera feedback.
- Keep Hunt's complete 480×576 viewport visible, with its existing 2× input mapping. Separate hits from credited entropy.
- Mark the previous round's pick and revealed machine guess in Outguess; explain its learning period without revealing the next guess.
- Enlarge Shard's board and add Left / Rotate / Right / Drop buttons. Retain gestures with cell-sized movement and axis locking. Show the selected piece's landing, never a recommended placement or heuristic rank.
- Give Dice familiar pip faces, a clear physical-roll count, masked recent rolls, Undo feedback, and its existing 50/99-roll gate.

All transient display models are cleared on exit. Do not expose secret values in logs. Video recording remains outside scope.

Verify the existing core suite, new display-only geometry helpers, the hosted build, and actual simulator controls at 480×800. Screenshots document the UI; hardware speed and physical haptics require a real-device check.
