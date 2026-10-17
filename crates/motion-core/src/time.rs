use serde::{Deserialize, Serialize};
use std::fmt;

/// Project clock resolution: 254,016,000,000 ticks per second.
pub const TICKS_PER_SECOND: i64 = 254_016_000_000;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tick(pub i64);

impl Tick {
    pub const ZERO: Self = Self(0);

    pub fn checked_add(self, rhs: Self) -> Option<Self> { self.0.checked_add(rhs.0).map(Self) }
    pub fn checked_sub(self, rhs: Self) -> Option<Self> { self.0.checked_sub(rhs.0).map(Self) }

    /// Converts an integer count at `rate` units per second exactly.
    pub fn from_rate_units(units: i64, rate: FrameRate) -> Option<Self> {
        if !rate.validate() { return None; }
        let product = i128::from(units).checked_mul(i128::from(TICKS_PER_SECOND))?
            .checked_mul(i128::from(rate.denominator))?;
        let denominator = i128::from(rate.numerator);
        if product % denominator != 0 { return None; }
        let ticks = product / denominator;
        i64::try_from(ticks).ok().map(Self)
    }

    /// Converts ticks to an integer unit count exactly.
    pub fn to_rate_units(self, rate: FrameRate) -> Option<i64> {
        if !rate.validate() { return None; }
        let product = i128::from(self.0).checked_mul(i128::from(rate.numerator))?;
        let divisor = i128::from(TICKS_PER_SECOND).checked_mul(i128::from(rate.denominator))?;
        if product % divisor != 0 { return None; }
        i64::try_from(product / divisor).ok()
    }
}

impl fmt::Display for Tick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}

/// Rational units-per-second rate (frames/sec or samples/sec).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct FrameRate {
    pub numerator: u32,
    pub denominator: u32,
}

/// Rational seconds-per-PTS-unit carried by a media stream's original clock.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct TimeBase { pub numerator: u32, pub denominator: u32 }

impl TimeBase {
    pub const fn new(numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 { None } else { Some(Self { numerator, denominator }) }
    }
    pub fn validate(self) -> bool { self.numerator != 0 && self.denominator != 0 }
}

impl FrameRate {
    pub const fn new(numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 { None } else { Some(Self { numerator, denominator }) }
    }

    pub fn validate(self) -> bool { self.numerator != 0 && self.denominator != 0 }

    /// Duration of one unit, represented in project ticks. Non-integral rates
    /// are rejected instead of silently accumulating rounding drift.
    pub fn unit_duration(self) -> Option<Tick> {
        if !self.validate() { return None; }
        let scaled = i128::from(TICKS_PER_SECOND).checked_mul(i128::from(self.denominator))?;
        let denom = i128::from(self.numerator);
        if scaled % denom != 0 { return None; }
        i64::try_from(scaled / denom).ok().map(Tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_frame_and_sample_rates_are_exact() {
        assert_eq!(FrameRate::new(30, 1).unwrap().unit_duration(), Some(Tick(8_467_200_000)));
        assert_eq!(FrameRate::new(48_000, 1).unwrap().unit_duration(), Some(Tick(5_292_000)));
        assert_eq!(FrameRate::new(30_000, 1_001).unwrap().unit_duration(), Some(Tick(8_475_667_200)));
    }
    #[test]
    fn conversions_use_wide_checked_intermediates() {
        assert_eq!(Tick::from_rate_units(30, FrameRate::new(30, 1).unwrap()), Some(Tick(TICKS_PER_SECOND)));
        assert_eq!(Tick(TICKS_PER_SECOND).to_rate_units(FrameRate::new(30, 1).unwrap()), Some(30));
        assert_eq!(Tick::from_rate_units(i64::MAX, FrameRate::new(1, 1).unwrap()), None);
    }
}
