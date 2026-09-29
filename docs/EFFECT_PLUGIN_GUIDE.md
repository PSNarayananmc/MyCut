# Writing Effect and Audio Plugins (in-repo registry)

Effects are data + one builder function. If an effect cannot be implemented
faithfully with real FFmpeg filters, do not add it.

## 1. Video effect

Edit `crates/engine/src/effects.rs`:

```rust
// 1. Register (registry()): id, label, params with declared ranges.
EffectDefinition::new("my_effect", "My Effect", "What it does", vec![
    EffectParam::new("amount", "Amount", 0.5, 0.0, 1.0, 0.01),
])

// 2. Build: match arm in build_effect() returning a FILTER CHAIN fragment.
//    Time window: use ctx.start/ctx.end (piece-local seconds).
//    Any per-frame expression may reference t.
fn build_my_effect(ctx: &EffectContext) -> Result<String, EngineError> {
    let a = ctx.p_or("amount", 0.5).clamp(0.0, 1.0);
    Ok(format!("eq=saturation={a:.3}:{}", en(ctx.start, ctx.end)))
}
```

Rules:
- The fragment is comma-joined into the per-piece chain; no `;` graph
  separators inside a single effect (split/blend effects need explicit
  graph wiring — coordinate with the engine before adding one).
- Params arrive pre-clamped by the validator AND the engine (defense in
  depth). Still clamp in your builder.
- The plan schema (`docs/schema/edit-plan.schema.json`) and
  `crates/schema/src/plan.rs` (`EffectId`) must list the effect id, and
  `crates/schema/src/validate.rs` must declare its ranges, otherwise plans
  referencing it are rejected.

## 2. Required tests (the gate)

1. Unit: `all_registered_effects_build` picks your effect up automatically.
2. **Render test** in `crates/engine/tests/render_integration.rs`: render a
   fixture (lavfi color/testsrc2) with the effect and assert on REAL output
   pixels — e.g. vignette darkens corners, blur lowers edge energy.
3. If the effect is windowed, assert the window: outside frames are
   unchanged.

## 3. Audio effect

Same pattern in `crates/engine/src/audiofx.rs`:
register in `registry()`, add a match arm in `build_audio_effect()`
(returning an ffmpeg audio filter fragment), extend the plan schema's
`adjust_audio` params if user-tunable, and add a test that measures the
REAL audio output (e.g. loudnorm measurement via loudnorm print_format=json).

## 4. Memory discipline

Never buffer frames in Rust. Everything streams through FFmpeg; your builder
only emits filter strings.
