# Contributing to MyCut

## Ground rules
1. **No fakes in production paths.** Mocked AI, simulated progress bars, and
   placeholder renders belong in `tests/` only, clearly named.
2. **Verify with evidence.** Any PR claiming a test/build/render result must
   include the real command output. Never claim a video "plays" unless you
   played it.
3. **Fix, don't suppress.** No `#[allow(...)]` to silence real errors, no
   skipping failing tests, no weakened assertions.
4. **Non-destructive, safe, private.** Source media is read-only; all process
   spawning uses argument arrays; nothing leaves the machine without an
   explicit, visible user action.
5. **Respect the gates.** P0 before P1 before P2. Do not start a phase with
   failing previous gates. Keep `STATUS.md` honest in the same PR that
   changes behavior.

## Dev workflow
```
cargo test --workspace        # all gates must stay green
cargo clippy --workspace      # no new warnings
npm --prefix ui run build     # UI must typecheck + bundle
./scripts/bench.sh            # for performance-affecting changes
```

## Adding an effect
See `docs/EFFECT_PLUGIN_GUIDE.md`. Effects must map to real FFmpeg filters
and ship with a render test that inspects actual output pixels.

## Commits
Small, focused, imperative subject lines. Docs-first: changes to the edit
plan schema require a PR updating `docs/schema/` + validator + tests together.
