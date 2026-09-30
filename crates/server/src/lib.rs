//! MyCut Local Web Runtime — the same typed command surface as the Tauri
//! shell, served over loopback HTTP so the identical React UI runs in any
//! browser on any Linux (including Ubuntu 20.04 / GLIBC 2.31, where no
//! WebKitGTK 4.1 stack exists). See docs/DECISIONS.md D15.
//!
//! Security posture: binds loopback by default, requires an `X-MyCut`
//! header on API calls (blocks cross-origin form POSTs), validates `Origin`
//! when present, never returns the API key, and streams media only from
//! jailed roots (project dir, cache dir, export dir).

pub mod assets;
pub mod edit;
pub mod jobs;
pub mod doctor;
pub mod http;
pub mod state;

pub use state::{AppState, CommandError};
