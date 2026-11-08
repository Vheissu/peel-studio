use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

/// Largest permitted denominator. Products of ordinary musical divisions
/// (2, 3, 5, 7 and MIDI resolutions such as 960 or 15360) stay far below it.
pub const MAX_DENOMINATOR: i128 = 1 << 40;

/// An exact position or duration in quarter notes.
///
/// Always stored in canonical reduced form with a positive denominator, so
/// derived `Eq`/`Hash` are structural equality of the rational value.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Time {
    num: i128,
    den: i128,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    #[error("denominator must be positive")]
    ZeroDenominator,
    #[error("denominator exceeds the supported bound")]
    DenominatorTooLarge,
    #[error("arithmetic overflow")]
    Overflow,
    #[error("invalid time literal: {0}")]
    Parse(String),
}

const fn gcd(mut a: i128, mut b: i128) -> i128 {
    if a < 0 {
        a = -a;
    }
    if b < 0 {
        b = -b;
    }
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

impl Time {
    pub const ZERO: Time = Time { num: 0, den: 1 };
    pub const BEAT: Time = Time { num: 1, den: 1 };

    /// Creates `num / den`, reducing it. Returns an error for a zero or
    /// oversized denominator.
    pub fn try_new(num: i128, den: i128) -> Result<Time, TimeError> {
        if den == 0 {
            return Err(TimeError::ZeroDenominator);
        }
        let (mut num, mut den) = if den < 0 { (-num, -den) } else { (num, den) };
        let g = gcd(num, den);
        if g > 1 {
            num /= g;
            den /= g;
        }
        if den > MAX_DENOMINATOR {
            return Err(TimeError::DenominatorTooLarge);
        }
        Ok(Time { num, den })
    }

    /// Creates `num / den`. Panics on an invalid denominator; use
    /// [`Time::try_new`] for untrusted input.
    pub fn new(num: i64, den: i64) -> Time {
        Time::try_new(num as i128, den as i128).expect("valid musical time")
    }

    /// Compile-time constructor. `num/den` must already be in lowest terms
    /// with a positive denominator.
    pub const fn new_const(num: i64, den: i64) -> Time {
        assert!(den > 0);
        Time { num: num as i128, den: den as i128 }
    }

    pub const fn beats(n: i64) -> Time {
        Time {
            num: n as i128,
            den: 1,
        }
    }

    pub const fn numerator(self) -> i128 {
        self.num
    }

    pub const fn denominator(self) -> i128 {
        self.den
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn is_negative(self) -> bool {
        self.num < 0
    }

    pub fn is_positive(self) -> bool {
        self.num > 0
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Nearest rational with the given denominator (for converting UI pixels
    /// or recorded timestamps onto a grid).
    pub fn from_f64(value: f64, den: i64) -> Time {
        let num = (value * den as f64).round() as i128;
        Time::try_new(num, den as i128).unwrap_or(Time::ZERO)
    }

    pub fn checked_add(self, rhs: Time) -> Result<Time, TimeError> {
        let g = gcd(self.den, rhs.den);
        let l = self.den / g;
        let num = self
            .num
            .checked_mul(rhs.den / g)
            .and_then(|a| rhs.num.checked_mul(l).and_then(|b| a.checked_add(b)))
            .ok_or(TimeError::Overflow)?;
        let den = l.checked_mul(rhs.den).ok_or(TimeError::Overflow)?;
        Time::try_new(num, den)
    }

    pub fn checked_sub(self, rhs: Time) -> Result<Time, TimeError> {
        self.checked_add(-rhs)
    }

    pub fn checked_mul(self, rhs: Time) -> Result<Time, TimeError> {
        let g1 = gcd(self.num, rhs.den).max(1);
        let g2 = gcd(rhs.num, self.den).max(1);
        let num = (self.num / g1)
            .checked_mul(rhs.num / g2)
            .ok_or(TimeError::Overflow)?;
        let den = (self.den / g2)
            .checked_mul(rhs.den / g1)
            .ok_or(TimeError::Overflow)?;
        Time::try_new(num, den)
    }

    pub fn checked_div(self, rhs: Time) -> Result<Time, TimeError> {
        if rhs.num == 0 {
            return Err(TimeError::ZeroDenominator);
        }
        self.checked_mul(
            Time {
                num: rhs.den,
                den: rhs.num,
            }
            .normalized(),
        )
    }

    fn normalized(self) -> Time {
        if self.den < 0 {
            Time {
                num: -self.num,
                den: -self.den,
            }
        } else {
            self
        }
    }

    /// Multiplies by an integer.
    pub fn mul_int(self, k: i64) -> Time {
        self.checked_mul(Time::beats(k))
            .expect("musical time overflow")
    }

    /// Divides by a positive integer.
    pub fn div_int(self, k: i64) -> Time {
        Time::try_new(self.num, self.den * k as i128).expect("musical time division")
    }

    /// Largest integer `k` with `k <= self`.
    pub fn floor_int(self) -> i128 {
        self.num.div_euclid(self.den)
    }

    /// Snaps down to a multiple of `grid` (grid must be positive).
    pub fn floor_to(self, grid: Time) -> Time {
        let k = self.checked_div(grid).map(|q| q.floor_int()).unwrap_or(0);
        grid.mul_int(k as i64)
    }

    /// Snaps to the nearest multiple of `grid` (ties round up).
    pub fn round_to(self, grid: Time) -> Time {
        let half = grid.div_int(2);
        (self + half).floor_to(grid)
    }

    /// Snaps up to a multiple of `grid`.
    pub fn ceil_to(self, grid: Time) -> Time {
        let f = self.floor_to(grid);
        if f == self { f } else { f + grid }
    }

    pub fn min(self, other: Time) -> Time {
        if self <= other { self } else { other }
    }

    pub fn max(self, other: Time) -> Time {
        if self >= other { self } else { other }
    }

    pub fn clamp(self, lo: Time, hi: Time) -> Time {
        self.max(lo).min(hi)
    }

    /// Euclidean remainder in `[0, m)` for positive `m`.
    pub fn rem_euclid(self, m: Time) -> Time {
        self - self.floor_to(m)
    }
}

impl Default for Time {
    fn default() -> Self {
        Time::ZERO
    }
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        // Denominators are positive and bounded by 2^40, numerators fit
        // comfortably for musical ranges; fall back to f64 only on overflow.
        match (
            self.num.checked_mul(other.den),
            other.num.checked_mul(self.den),
        ) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => self.as_f64().total_cmp(&other.as_f64()),
        }
    }
}

impl Add for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Time {
        self.checked_add(rhs).expect("musical time overflow")
    }
}

impl AddAssign for Time {
    fn add_assign(&mut self, rhs: Time) {
        *self = *self + rhs;
    }
}

impl Sub for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Time {
        self.checked_sub(rhs).expect("musical time overflow")
    }
}

impl SubAssign for Time {
    fn sub_assign(&mut self, rhs: Time) {
        *self = *self - rhs;
    }
}

impl Neg for Time {
    type Output = Time;
    fn neg(self) -> Time {
        Time {
            num: -self.num,
            den: self.den,
        }
    }
}

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Time({self})")
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl FromStr for Time {
    type Err = TimeError;
    fn from_str(s: &str) -> Result<Time, TimeError> {
        let s = s.trim();
        let parse = |p: &str| {
            p.trim()
                .parse::<i128>()
                .map_err(|_| TimeError::Parse(s.to_string()))
        };
        match s.split_once('/') {
            Some((n, d)) => Time::try_new(parse(n)?, parse(d)?),
            None => Time::try_new(parse(s)?, 1),
        }
    }
}

impl serde::Serialize for Time {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Time {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Time, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_and_orders() {
        assert_eq!(Time::new(2, 4), Time::new(1, 2));
        assert_eq!(Time::new(-3, -6), Time::new(1, 2));
        assert!(Time::new(1, 3) < Time::new(1, 2));
        assert_eq!(Time::new(1, 3) + Time::new(1, 6), Time::new(1, 2));
        assert_eq!(Time::new(1, 4) - Time::new(1, 2), Time::new(-1, 4));
    }

    #[test]
    fn snapping() {
        let g = Time::new(1, 4);
        assert_eq!(Time::new(5, 8).floor_to(g), Time::new(1, 2));
        assert_eq!(Time::new(5, 8).round_to(g), Time::new(3, 4));
        assert_eq!(Time::new(-1, 8).floor_to(g), Time::new(-1, 4));
        assert_eq!(Time::new(1, 2).ceil_to(g), Time::new(1, 2));
        assert_eq!(Time::new(9, 4).rem_euclid(Time::beats(2)), Time::new(1, 4));
    }

    #[test]
    fn triplets_stay_exact() {
        let t = Time::new(1, 3);
        let sum = t + t + t;
        assert_eq!(sum, Time::BEAT);
    }

    #[test]
    fn serde_round_trip() {
        let t = Time::new(-7, 12);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"-7/12\"");
        let back: Time = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        let whole: Time = serde_json::from_str("\"4\"").unwrap();
        assert_eq!(whole, Time::beats(4));
    }
}
