use std::fmt;
use std::ops::{Add, Sub, Mul, Div, Neg};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MathError {
    Overflow,
    Underflow,
    DivisionByZero,
    NegativeSquareRoot,
    ZeroLengthVector,
    InvalidDomain,
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MathError::Overflow => write!(f, "Q64.64 arithmetic overflow"),
            MathError::Underflow => write!(f, "Q64.64 arithmetic underflow"),
            MathError::DivisionByZero => write!(f, "Division by zero in Q64.64"),
            MathError::NegativeSquareRoot => write!(f, "Negative square root in Q64.64"),
            MathError::ZeroLengthVector => write!(f, "Cannot normalize zero-length vector"),
            MathError::InvalidDomain => write!(f, "Value outside allowed physical domain"),
        }
    }
}

impl std::error::Error for MathError {}

/// Strict Software Fixed-Point Q64.64 Number.
/// Value = raw / 2^64.
/// Eliminates all cross-architecture floating-point divergence (IEEE 754 differences).
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Q64(pub i128);

impl Q64 {
    pub const FRACTIONAL_BITS: u32 = 64;
    pub const ONE_RAW: i128 = 1i128 << Self::FRACTIONAL_BITS;
    pub const HALF_RAW: i128 = 1i128 << (Self::FRACTIONAL_BITS - 1);
    
    pub const ZERO: Q64 = Q64(0);
    pub const ONE: Q64 = Q64(Self::ONE_RAW);
    pub const HALF: Q64 = Q64(Self::HALF_RAW);
    pub const MIN: Q64 = Q64(i128::MIN);
    pub const MAX: Q64 = Q64(i128::MAX);

    #[inline(always)]
    pub const fn from_raw(raw: i128) -> Self {
        Q64(raw)
    }

    #[inline(always)]
    pub const fn to_raw(self) -> i128 {
        self.0
    }

    #[inline]
    pub fn from_int(val: i64) -> Result<Self, MathError> {
        let raw = (val as i128).checked_mul(Self::ONE_RAW).ok_or(MathError::Overflow)?;
        Ok(Q64(raw))
    }

    #[inline]
    pub const fn to_int_trunc(self) -> i64 {
        (self.0 / Self::ONE_RAW) as i64
    }

    /// Exact mathematical floor conversion: floor(X / 2^64).
    /// Critical for negative world coordinate to chunk/lattice conversion (F14, F11).
    #[inline]
    pub const fn to_int_floor(self) -> i64 {
        (self.0 >> Self::FRACTIONAL_BITS) as i64
    }

    /// Checked negation with explicit overflow detection for i128::MIN (F14).
    #[inline]
    pub fn checked_neg(self) -> Result<Self, MathError> {
        self.0.checked_neg().map(Q64).ok_or(MathError::Overflow)
    }

    /// Checked absolute value with explicit overflow detection for i128::MIN (F14).
    #[inline]
    pub fn checked_abs(self) -> Result<Self, MathError> {
        self.0.checked_abs().map(Q64).ok_or(MathError::Overflow)
    }

    /// Checked addition with explicit overflow detection.
    #[inline]
    pub fn checked_add(self, rhs: Self) -> Result<Self, MathError> {
        self.0.checked_add(rhs.0).map(Q64).ok_or(MathError::Overflow)
    }

    /// Checked subtraction with explicit underflow/overflow detection.
    #[inline]
    pub fn checked_sub(self, rhs: Self) -> Result<Self, MathError> {
        self.0.checked_sub(rhs.0).map(Q64).ok_or(MathError::Overflow)
    }

    /// Checked exact Q64.64 multiplication with intermediate 256-bit product.
    /// Truncates toward zero normative rule.
    pub fn checked_mul(self, rhs: Self) -> Result<Self, MathError> {
        if self.0 == 0 || rhs.0 == 0 {
            return Ok(Q64::ZERO);
        }

        let is_negative = (self.0 < 0) ^ (rhs.0 < 0);
        let u_mag = self.0.unsigned_abs();
        let v_mag = rhs.0.unsigned_abs();

        // 128-bit x 128-bit unsigned multiplication into 256-bit parts
        // u = u_hi * 2^64 + u_lo, v = v_hi * 2^64 + v_lo
        let u_hi = (u_mag >> 64) as u64;
        let u_lo = u_mag as u64;
        let v_hi = (v_mag >> 64) as u64;
        let v_lo = v_mag as u64;

        let p0 = (u_lo as u128) * (v_lo as u128); // 128-bit
        let p1 = (u_lo as u128) * (v_hi as u128); // 128-bit
        let p2 = (u_hi as u128) * (v_lo as u128); // 128-bit
        let p3 = (u_hi as u128) * (v_hi as u128); // 128-bit

        // Sum components with shift:
        // total product = p0 + (p1 + p2) * 2^64 + p3 * 2^128
        // Dividing by 2^64:
        // result = (p0 >> 64) + (p1 + p2) + (p3 << 64)
        
        let p0_shifted = p0 >> 64;
        let mid_sum = p1.checked_add(p2).ok_or(MathError::Overflow)?;
        let low_plus_mid = p0_shifted.checked_add(mid_sum).ok_or(MathError::Overflow)?;

        if p3 != 0 {
            // p3 << 64 would require high 64 bits to be 0 for result to fit in u128
            if p3 > (u128::MAX >> 64) {
                return Err(MathError::Overflow);
            }
            let p3_shifted = p3 << 64;
            let total_mag = low_plus_mid.checked_add(p3_shifted).ok_or(MathError::Overflow)?;
            Self::apply_sign(total_mag, is_negative)
        } else {
            Self::apply_sign(low_plus_mid, is_negative)
        }
    }

    /// Checked exact Q64.64 division with intermediate 256-bit shift.
    /// Computes trunc((self * 2^64) / rhs)
    pub fn checked_div(self, rhs: Self) -> Result<Self, MathError> {
        if rhs.0 == 0 {
            return Err(MathError::DivisionByZero);
        }
        if self.0 == 0 {
            return Ok(Q64::ZERO);
        }

        let is_negative = (self.0 < 0) ^ (rhs.0 < 0);
        let num_mag = self.0.unsigned_abs();
        let den_mag = rhs.0.unsigned_abs();

        // Numerator * 2^64 is a 256-bit integer: (num_mag << 64)
        // High 128 bits = num_mag >> 64
        // Low 128 bits = num_mag << 64
        let hi = num_mag >> 64;
        let lo = num_mag << 64;

        let quotient = u256_div_u128(hi, lo, den_mag)?;
        Self::apply_sign(quotient, is_negative)
    }

    /// Deterministic integer square root of Q64.64.
    /// sqrt(X / 2^64) = sqrt(X * 2^64) / 2^64.
    /// Since the raw value of the result is (sqrt(X * 2^64) / 2^64) * 2^64 = sqrt(X * 2^64),
    /// we compute the integer square root of the 256-bit integer (self.raw * 2^64).
    pub fn checked_sqrt(self) -> Result<Self, MathError> {
        if self.0 < 0 {
            return Err(MathError::NegativeSquareRoot);
        }
        if self.0 == 0 {
            return Ok(Q64::ZERO);
        }

        let mag = self.0 as u128;
        // 256-bit number: (mag << 64)
        let hi = mag >> 64;
        let lo = mag << 64;

        let root = u256_isqrt(hi, lo);
        if root > i128::MAX as u128 {
            return Err(MathError::Overflow);
        }
        Ok(Q64(root as i128))
    }

    #[inline]
    fn apply_sign(mag: u128, is_negative: bool) -> Result<Self, MathError> {
        if is_negative {
            if mag > (i128::MAX as u128 + 1) {
                return Err(MathError::Overflow);
            }
            if mag == (i128::MAX as u128 + 1) {
                return Ok(Q64(i128::MIN));
            }
            Ok(Q64(-(mag as i128)))
        } else {
            if mag > i128::MAX as u128 {
                return Err(MathError::Overflow);
            }
            Ok(Q64(mag as i128))
        }
    }
}

impl fmt::Debug for Q64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Q64({})", self)
    }
}

impl fmt::Display for Q64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let mag = self.0.unsigned_abs();
        let whole = mag >> Self::FRACTIONAL_BITS;
        let frac = mag & ((1u128 << Self::FRACTIONAL_BITS) - 1);
        let frac_scaled = (frac.saturating_mul(10000)) >> Self::FRACTIONAL_BITS;
        write!(f, "{}{}.{:04}", sign, whole, frac_scaled)
    }
}

impl Add for Q64 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        self.checked_add(rhs).expect("Q64 add overflow")
    }
}

impl Sub for Q64 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        self.checked_sub(rhs).expect("Q64 sub overflow")
    }
}

impl Mul for Q64 {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        self.checked_mul(rhs).expect("Q64 mul overflow")
    }
}

impl Div for Q64 {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        self.checked_div(rhs).expect("Q64 div overflow")
    }
}

impl Neg for Q64 {
    type Output = Self;
    fn neg(self) -> Self {
        self.checked_neg().expect("Q64 neg overflow")
    }
}

/// Bitwise exact division of a 256-bit unsigned integer (hi: u128, lo: u128) by a 128-bit denominator.
fn u256_div_u128(hi: u128, lo: u128, den: u128) -> Result<u128, MathError> {
    if den == 0 {
        return Err(MathError::DivisionByZero);
    }
    if hi >= den {
        return Err(MathError::Overflow);
    }

    // Binary long division: 128 bits
    let mut quotient: u128 = 0;
    let mut rem_hi = hi;
    let rem_lo = lo;

    for i in (0..128).rev() {
        // Shift remainder left by 1 bit, bringing in bit i from lo
        let bit = (rem_lo >> i) & 1;
        rem_hi = (rem_hi << 1) | bit;

        if rem_hi >= den {
            rem_hi -= den;
            quotient |= 1 << i;
        }
    }

    Ok(quotient)
}

/// Exact multiplication of two u128 integers into a 256-bit product (hi, lo).
fn u128_mul_wide(u: u128, v: u128) -> (u128, u128) {
    let u_hi = (u >> 64) as u64 as u128;
    let u_lo = u as u64 as u128;
    let v_hi = (v >> 64) as u64 as u128;
    let v_lo = v as u64 as u128;

    let p0 = u_lo * v_lo;
    let p1 = u_lo * v_hi;
    let p2 = u_hi * v_lo;
    let p3 = u_hi * v_hi;

    let p0_hi = p0 >> 64;
    let p0_lo = p0 as u64 as u128;

    let mid1 = p1 + p0_hi;
    let mid_lo = mid1 as u64 as u128;
    let mid1_hi = mid1 >> 64;

    let mid2 = p2 + mid_lo;
    let mid2_lo = mid2 as u64 as u128;
    let mid2_hi = (mid2 >> 64) + mid1_hi;

    let lo = p0_lo | (mid2_lo << 64);
    let hi = p3 + mid2_hi;

    (hi, lo)
}

/// Exact integer square root of a 256-bit unsigned integer (hi: u128, lo: u128).
/// Returns floor(sqrt(N)) as u128.
fn u256_isqrt(hi: u128, lo: u128) -> u128 {
    if hi == 0 && lo == 0 {
        return 0;
    }

    let mut root: u128 = 0;
    for i in (0..128).rev() {
        let candidate = root | (1 << i);
        let (sq_hi, sq_lo) = u128_mul_wide(candidate, candidate);
        let fits = if sq_hi < hi {
            true
        } else if sq_hi == hi {
            sq_lo <= lo
        } else {
            false
        };

        if fits {
            root = candidate;
        }
    }

    root
}

/// 3D Vector with deterministic Q64.64 coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Vec3Fixed {
    pub x: Q64,
    pub y: Q64,
    pub z: Q64,
}

impl Vec3Fixed {
    pub const ZERO: Vec3Fixed = Vec3Fixed { x: Q64::ZERO, y: Q64::ZERO, z: Q64::ZERO };

    pub fn new(x: Q64, y: Q64, z: Q64) -> Self {
        Vec3Fixed { x, y, z }
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, MathError> {
        Ok(Vec3Fixed {
            x: self.x.checked_add(rhs.x)?,
            y: self.y.checked_add(rhs.y)?,
            z: self.z.checked_add(rhs.z)?,
        })
    }

    pub fn checked_sub(self, rhs: Self) -> Result<Self, MathError> {
        Ok(Vec3Fixed {
            x: self.x.checked_sub(rhs.x)?,
            y: self.y.checked_sub(rhs.y)?,
            z: self.z.checked_sub(rhs.z)?,
        })
    }

    pub fn dot(self, rhs: Self) -> Result<Q64, MathError> {
        let px = self.x.checked_mul(rhs.x)?;
        let py = self.y.checked_mul(rhs.y)?;
        let pz = self.z.checked_mul(rhs.z)?;
        px.checked_add(py)?.checked_add(pz)
    }

    pub fn length_sq(self) -> Result<Q64, MathError> {
        self.dot(self)
    }

    pub fn length(self) -> Result<Q64, MathError> {
        self.length_sq()?.checked_sqrt()
    }

    pub fn normalize(self) -> Result<Self, MathError> {
        let len = self.length()?;
        if len == Q64::ZERO {
            return Err(MathError::ZeroLengthVector);
        }
        Ok(Vec3Fixed {
            x: self.x.checked_div(len)?,
            y: self.y.checked_div(len)?,
            z: self.z.checked_div(len)?,
        })
    }

    pub fn distance(self, other: Self) -> Result<Q64, MathError> {
        self.checked_sub(other)?.length()
    }
}

/// Axis-Aligned Bounding Box (AABB) using Q64.64 coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AabbFixed {
    pub min: Vec3Fixed,
    pub max: Vec3Fixed,
}

impl AabbFixed {
    pub fn new(min: Vec3Fixed, max: Vec3Fixed) -> Self {
        AabbFixed { min, max }
    }

    pub fn intersects(&self, other: &Self) -> bool {
        self.min.x.0 <= other.max.x.0 && self.max.x.0 >= other.min.x.0 &&
        self.min.y.0 <= other.max.y.0 && self.max.y.0 >= other.min.y.0 &&
        self.min.z.0 <= other.max.z.0 && self.max.z.0 >= other.min.z.0
    }

    pub fn contains_point(&self, p: Vec3Fixed) -> bool {
        p.x.0 >= self.min.x.0 && p.x.0 <= self.max.x.0 &&
        p.y.0 >= self.min.y.0 && p.y.0 <= self.max.y.0 &&
        p.z.0 >= self.min.z.0 && p.z.0 <= self.max.z.0
    }

    pub fn expand(&self, margin: Q64) -> Result<Self, MathError> {
        if margin.0 < 0 {
            return Err(MathError::InvalidDomain);
        }
        Ok(AabbFixed {
            min: Vec3Fixed {
                x: self.min.x.checked_sub(margin)?,
                y: self.min.y.checked_sub(margin)?,
                z: self.min.z.checked_sub(margin)?,
            },
            max: Vec3Fixed {
                x: self.max.x.checked_add(margin)?,
                y: self.max.y.checked_add(margin)?,
                z: self.max.z.checked_add(margin)?,
            },
        })
    }
}

/// Swept Capsule Primitive for Continuous Collision Detection (CCD).
/// Solves boundary seam tunneling (F11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweptCapsule {
    pub start: Vec3Fixed,
    pub end: Vec3Fixed,
    pub radius: Q64,
}

impl SweptCapsule {
    /// Compute the conservative swept AABB bounding box for this trajectory.
    pub fn bounding_box(&self) -> Result<AabbFixed, MathError> {
        if self.radius.0 < 0 {
            return Err(MathError::InvalidDomain);
        }
        let min_x = if self.start.x.0 < self.end.x.0 { self.start.x } else { self.end.x };
        let max_x = if self.start.x.0 > self.end.x.0 { self.start.x } else { self.end.x };
        let min_y = if self.start.y.0 < self.end.y.0 { self.start.y } else { self.end.y };
        let max_y = if self.start.y.0 > self.end.y.0 { self.start.y } else { self.end.y };
        let min_z = if self.start.z.0 < self.end.z.0 { self.start.z } else { self.end.z };
        let max_z = if self.start.z.0 > self.end.z.0 { self.start.z } else { self.end.z };

        let aabb = AabbFixed {
            min: Vec3Fixed { x: min_x, y: min_y, z: min_z },
            max: Vec3Fixed { x: max_x, y: max_y, z: max_z },
        };
        aabb.expand(self.radius)
    }
}
