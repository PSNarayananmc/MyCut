# Security Policy

## Supported
Security fixes land on the latest release tag.

## Reporting
Email security@mycut.invalid (placeholder until a real mailbox exists) or
open a GitHub security advisory. Do not open public issues for
vulnerabilities.

## Security model (what we promise)
- **No shell**: every subprocess uses argument arrays. A source-level audit
  test (`crates/engine/tests/security.rs`) enforces this.
- **Path jail**: media/LUT/font paths must resolve inside the project
  directory or declared asset roots; `..` traversal and symlink escapes are
  rejected by `mycut_engine::pathing` (tested, incl. symlink escape).
- **Secrets**: the NVIDIA NIM key is stored in the OS keyring (Secret
  Service) or a 0600 file. It is only read by Rust. Behavioral tests assert
  the key never appears in logs, error messages, or AI payloads (canary).
- **No telemetry**: the app makes zero network requests except the user's
  explicit AI requests (and optional frame sharing, off by default, visibly
  indicated).
- **Unvalidated AI output is never executed**: the Rust validator is the only
  path from LLM JSON to the timeline; adversarial plans are tested
  (traversal, injection, overlaps, out-of-range params).

## Known limitations
- Preview renders write temporary files into the project dir; they are
  cleaned on cancel/exit but readable by the local user while present.
- whisper.cpp runs as a child process with the user's privileges; model
  files are user-provided downloads (size + license shown before download).
