//! Canonical time representation and id generation.
//!
//! MyCut stores all time as signed 64-bit milliseconds. Fields named
//! `source_in_ms` / `source_out_ms` are in **source** time; fields named
//! `timeline_*` are in **timeline** time. The AI plan boundary uses f64
//! seconds and converts here (see `seconds_to_ms`).

use serde::{Deserialize, Serialize};

pub type TimeMs = i64;

pub const MS_PER_SECOND: f64 = 1000.0;

/// Convert AI-facing seconds to canonical milliseconds, clamping negatives to 0.
#[must_use]
pub fn seconds_to_ms(seconds: f64) -> TimeMs {
    if !seconds.is_finite() || seconds <= 0.0 {
        0
    } else {
        (seconds * MS_PER_SECOND).round() as TimeMs
    }
}

/// Convert milliseconds to f64 seconds (AI-facing / display).
#[must_use]
pub fn ms_to_seconds(ms: TimeMs) -> f64 {
    ms as f64 / MS_PER_SECOND
}

/// Monotonic-ish unique id: prefix + nanos + counter. No external deps.
#[must_use]
pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}_{nanos:x}{n:x}")
}

/// Duration formatting for logs/UI: `12.345s`.
#[must_use]
pub fn fmt_seconds(ms: TimeMs) -> String {
    format!("{:.3}s", ms_to_seconds(ms))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    /// Map linear time `t` in [0,1] to eased progress in [0,1].
    #[must_use]
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_roundtrip() {
        assert_eq!(seconds_to_ms(1.5), 1500);
        assert_eq!(seconds_to_ms(-3.0), 0);
        assert_eq!(seconds_to_ms(f64::NAN), 0);
        assert!((ms_to_seconds(1234) - 1.234).abs() < 1e-9);
    }

    #[test]
    fn easing_bounds() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            assert!((e.apply(0.0) - 0.0).abs() < 1e-9);
            assert!((e.apply(1.0) - 1.0).abs() < 1e-9);
        }
        assert!(Easing::EaseIn.apply(0.5) < Easing::EaseOut.apply(0.5));
    }

    #[test]
    fn ids_unique() {
        assert_ne!(new_id("clip"), new_id("clip"));
    }
}
