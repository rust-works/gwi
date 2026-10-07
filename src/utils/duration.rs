//! Relative-duration parsing for `--older-than`-style flags.
//!
//! Forked from omni-dev's `cli::log::query::parse_since` (rust-works/omni-dev#2203): the
//! Drive lease `prune` command is the only consumer gwi needs, and omni-dev's log command
//! it came from is not part of gwi.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};

/// Parses a relative duration like `30m`, `2h`, `1d`, `1w`, `45s` into the
/// absolute cutoff `now - duration`. Used by `drive lease prune --older-than`.
pub fn parse_since(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    let (num, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .with_context(|| format!("invalid duration: {s} (expected e.g. 30m, 2h, 1d)"))?,
    );
    let n: i64 = num
        .parse()
        .with_context(|| format!("invalid duration: {s} (expected e.g. 30m, 2h, 1d)"))?;
    let dur = match unit {
        "s" => Duration::try_seconds(n),
        "m" => Duration::try_minutes(n),
        "h" => Duration::try_hours(n),
        "d" => Duration::try_days(n),
        "w" => Duration::try_weeks(n),
        other => bail!("invalid duration unit: {other} (use s, m, h, d, or w)"),
    }
    .with_context(|| format!("invalid duration: {s} (out of range)"))?;
    Utc::now()
        .checked_sub_signed(dur)
        .with_context(|| format!("invalid duration: {s} (out of range)"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn since_parses_units() {
        for ok in ["45s", "30m", "2h", "1d", "1w", " 2h "] {
            let cutoff = parse_since(ok).unwrap();
            assert!(cutoff < Utc::now(), "{ok}");
        }
        assert!(parse_since("10x").is_err());
        assert!(parse_since("h").is_err());
        assert!(parse_since("").is_err());
        assert!(parse_since("30").is_err());
    }

    #[test]
    fn since_rejects_oversized_duration() {
        assert!(parse_since("100000000d").is_err()); // subtraction overflow
        assert!(parse_since("9999999999999999w").is_err()); // construction overflow
    }
}
