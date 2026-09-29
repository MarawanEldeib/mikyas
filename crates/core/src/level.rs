//! Usage colour bands: green below [`WARN_AT`], orange from it, red from [`CRIT_AT`]. Fixed
//! display bands, separate from the user's alert thresholds in Settings. The tray icon and the
//! statusline helper use these; the UI mirrors them in `src/lib/color.ts` (a vitest reads this
//! file and fails when they differ).
//!
//! One rounding rule everywhere: the band comes from the number shown ([`display_pct`]), so 39.6 %
//! reads "40" and is orange in the widget, the tray and the statusline alike.

/// Lowest shown % (inclusive) of the orange band.
pub const WARN_AT: u8 = 40;
/// Lowest shown % (inclusive) of the red band.
pub const CRIT_AT: u8 = 70;

/// Colour band of a usage percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLevel {
    Ok,
    Warn,
    Crit,
}

/// The whole percentage shown for `pct`: clamped to 0..=100 and rounded (half away from zero);
/// non-finite input is 0.
pub fn display_pct(pct: f32) -> u8 {
    if pct.is_finite() {
        // Clamped first, so the cast cannot truncate.
        pct.clamp(0.0, 100.0).round() as u8
    } else {
        0
    }
}

impl UsageLevel {
    /// Band of an already displayed (whole) percentage.
    pub fn for_shown(pct: u8) -> Self {
        if pct >= CRIT_AT {
            Self::Crit
        } else if pct >= WARN_AT {
            Self::Warn
        } else {
            Self::Ok
        }
    }

    /// Band of a raw percentage, judged by the number it is shown as.
    pub fn for_pct(pct: f32) -> Self {
        Self::for_shown(display_pct(pct))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_are_ordered() {
        const { assert!(0 < WARN_AT && WARN_AT < CRIT_AT && CRIT_AT <= 100) };
    }

    #[test]
    fn display_pct_clamps_and_rounds() {
        assert_eq!(display_pct(39.4), 39);
        assert_eq!(display_pct(39.5), 40);
        assert_eq!(display_pct(-3.0), 0);
        assert_eq!(display_pct(250.0), 100);
        assert_eq!(display_pct(f32::NAN), 0);
        assert_eq!(display_pct(f32::INFINITY), 0);
    }

    #[test]
    fn level_follows_the_shown_number() {
        assert_eq!(UsageLevel::for_pct(0.0), UsageLevel::Ok);
        assert_eq!(UsageLevel::for_pct(39.4), UsageLevel::Ok);
        assert_eq!(UsageLevel::for_pct(39.5), UsageLevel::Warn, "shown as 40");
        assert_eq!(UsageLevel::for_pct(69.4), UsageLevel::Warn);
        assert_eq!(UsageLevel::for_pct(69.5), UsageLevel::Crit, "shown as 70");
        assert_eq!(UsageLevel::for_pct(100.0), UsageLevel::Crit);
        assert_eq!(UsageLevel::for_shown(WARN_AT - 1), UsageLevel::Ok);
        assert_eq!(UsageLevel::for_shown(WARN_AT), UsageLevel::Warn);
        assert_eq!(UsageLevel::for_shown(CRIT_AT), UsageLevel::Crit);
    }
}
