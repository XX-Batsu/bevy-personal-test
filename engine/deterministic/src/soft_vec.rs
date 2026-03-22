//! SoftVec2 / SoftVec3 確定性向量型別。
//! 所有算術運算透過 SoftF32 軟體浮點實作，確保跨平台 bit-exact 一致性。

use crate::soft_float::SoftF32;
use serde::{Deserialize, Serialize};
use std::ops::{Add, Div, Mul, Neg, Sub};

// ── SoftVec2 ─────────────────────────────────────────────────────────────

/// 確定性二維向量 — 各分量為 SoftF32，所有運算跨平台 bit-exact。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SoftVec2 {
    pub x: SoftF32,
    pub y: SoftF32,
}

impl SoftVec2 {
    /// 從兩個 SoftF32 建構
    pub fn new(x: SoftF32, y: SoftF32) -> Self {
        Self { x, y }
    }

    /// 零向量 (0, 0)
    pub fn zero() -> Self {
        Self {
            x: SoftF32::ZERO,
            y: SoftF32::ZERO,
        }
    }

    /// 內積：self.x * rhs.x + self.y * rhs.y
    pub fn dot(self, rhs: Self) -> SoftF32 {
        self.x * rhs.x + self.y * rhs.y
    }

    /// 長度平方：dot(self, self)
    pub fn length_squared(self) -> SoftF32 {
        self.dot(self)
    }

    /// 長度：sqrt(length_squared())
    pub fn length(self) -> SoftF32 {
        self.length_squared().sqrt()
    }

    /// 純量縮放：各分量乘以 s
    pub fn scale(self, s: SoftF32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
        }
    }

    /// 單位化。零向量回傳零向量（安全除法，不產生 NaN/Inf）。
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len == SoftF32::ZERO {
            Self::zero()
        } else {
            let inv = SoftF32::from_f32(1.0) / len;
            self.scale(inv)
        }
    }
}

impl Add for SoftVec2 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for SoftVec2 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl Neg for SoftVec2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl Mul for SoftVec2 {
    type Output = Self;
    /// Component-wise 乘法（非內積）
    fn mul(self, rhs: Self) -> Self {
        Self {
            x: self.x * rhs.x,
            y: self.y * rhs.y,
        }
    }
}

impl Mul<SoftF32> for SoftVec2 {
    type Output = Self;
    /// Scalar 乘法：各分量乘以純量（等同 scale()，提供 operator 語法）
    fn mul(self, rhs: SoftF32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
        }
    }
}

impl Div for SoftVec2 {
    type Output = Self;
    /// Component-wise 除法
    fn div(self, rhs: Self) -> Self {
        Self {
            x: self.x / rhs.x,
            y: self.y / rhs.y,
        }
    }
}

// ── SoftVec3 ─────────────────────────────────────────────────────────────

/// 確定性三維向量 — 各分量為 SoftF32，所有運算跨平台 bit-exact。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SoftVec3 {
    pub x: SoftF32,
    pub y: SoftF32,
    pub z: SoftF32,
}

impl SoftVec3 {
    /// 從三個 SoftF32 建構
    pub fn new(x: SoftF32, y: SoftF32, z: SoftF32) -> Self {
        Self { x, y, z }
    }

    /// 零向量 (0, 0, 0)
    pub fn zero() -> Self {
        Self {
            x: SoftF32::ZERO,
            y: SoftF32::ZERO,
            z: SoftF32::ZERO,
        }
    }

    /// 內積：self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    pub fn dot(self, rhs: Self) -> SoftF32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    /// 長度平方：dot(self, self)
    pub fn length_squared(self) -> SoftF32 {
        self.dot(self)
    }

    /// 長度：sqrt(length_squared())
    pub fn length(self) -> SoftF32 {
        self.length_squared().sqrt()
    }

    /// 純量縮放：各分量乘以 s
    pub fn scale(self, s: SoftF32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }

    /// 單位化。零向量回傳零向量（安全除法，不產生 NaN/Inf）。
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len == SoftF32::ZERO {
            Self::zero()
        } else {
            let inv = SoftF32::from_f32(1.0) / len;
            self.scale(inv)
        }
    }

    /// 向量外積（cross product）。
    ///
    /// ```text
    /// result.x = self.y * rhs.z - self.z * rhs.y
    /// result.y = self.z * rhs.x - self.x * rhs.z
    /// result.z = self.x * rhs.y - self.y * rhs.x
    /// ```
    pub fn cross(self, rhs: Self) -> Self {
        Self {
            x: self.y * rhs.z - self.z * rhs.y,
            y: self.z * rhs.x - self.x * rhs.z,
            z: self.x * rhs.y - self.y * rhs.x,
        }
    }
}

impl Add for SoftVec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl Sub for SoftVec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl Neg for SoftVec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

impl Mul for SoftVec3 {
    type Output = Self;
    /// Component-wise 乘法（非內積）
    fn mul(self, rhs: Self) -> Self {
        Self {
            x: self.x * rhs.x,
            y: self.y * rhs.y,
            z: self.z * rhs.z,
        }
    }
}

impl Mul<SoftF32> for SoftVec3 {
    type Output = Self;
    /// Scalar 乘法：各分量乘以純量（等同 scale()，提供 operator 語法）
    fn mul(self, rhs: SoftF32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
        }
    }
}

impl Div for SoftVec3 {
    type Output = Self;
    /// Component-wise 除法
    fn div(self, rhs: Self) -> Self {
        Self {
            x: self.x / rhs.x,
            y: self.y / rhs.y,
            z: self.z / rhs.z,
        }
    }
}

// ── 測試模組 ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soft_float::SoftF32;

    // === SoftVec2 ===

    #[test]
    fn test_vec2_addition() {
        let a = SoftVec2::new(SoftF32::from_f32(1.0), SoftF32::from_f32(2.0));
        let b = SoftVec2::new(SoftF32::from_f32(3.0), SoftF32::from_f32(4.0));
        let r = a + b;
        assert_eq!(r.x.to_native(), 4.0);
        assert_eq!(r.y.to_native(), 6.0);
    }

    #[test]
    fn test_vec2_subtraction() {
        let a = SoftVec2::new(SoftF32::from_f32(5.0), SoftF32::from_f32(7.0));
        let b = SoftVec2::new(SoftF32::from_f32(2.0), SoftF32::from_f32(3.0));
        let r = a - b;
        assert_eq!(r.x.to_native(), 3.0);
        assert_eq!(r.y.to_native(), 4.0);
    }

    #[test]
    fn test_vec2_negation() {
        let a = SoftVec2::new(SoftF32::from_f32(1.0), SoftF32::from_f32(-2.0));
        let r = -a;
        assert_eq!(r.x.to_native(), -1.0);
        assert_eq!(r.y.to_native(), 2.0);
    }

    #[test]
    fn test_vec2_component_mul() {
        let a = SoftVec2::new(SoftF32::from_f32(2.0), SoftF32::from_f32(3.0));
        let b = SoftVec2::new(SoftF32::from_f32(4.0), SoftF32::from_f32(5.0));
        let r = a * b;
        assert_eq!(r.x.to_native(), 8.0);
        assert_eq!(r.y.to_native(), 15.0);
    }

    #[test]
    fn test_vec2_scalar_mul() {
        let v = SoftVec2::new(SoftF32::from_f32(2.0), SoftF32::from_f32(3.0));
        let r = v * SoftF32::from_f32(4.0);
        assert_eq!(r.x.to_native(), 8.0);
        assert_eq!(r.y.to_native(), 12.0);
    }

    #[test]
    fn test_vec2_component_div() {
        let a = SoftVec2::new(SoftF32::from_f32(10.0), SoftF32::from_f32(6.0));
        let b = SoftVec2::new(SoftF32::from_f32(2.0), SoftF32::from_f32(3.0));
        let r = a / b;
        assert_eq!(r.x.to_native(), 5.0);
        assert_eq!(r.y.to_native(), 2.0);
    }

    #[test]
    fn test_vec2_dot_orthogonal() {
        let a = SoftVec2::new(SoftF32::from_f32(1.0), SoftF32::from_f32(0.0));
        let b = SoftVec2::new(SoftF32::from_f32(0.0), SoftF32::from_f32(1.0));
        assert_eq!(a.dot(b).to_native(), 0.0); // 正交向量內積為零
    }

    #[test]
    fn test_vec2_length_squared() {
        let v = SoftVec2::new(SoftF32::from_f32(3.0), SoftF32::from_f32(4.0));
        assert_eq!(v.length_squared().to_native(), 25.0);
    }

    #[test]
    fn test_vec2_length() {
        let v = SoftVec2::new(SoftF32::from_f32(3.0), SoftF32::from_f32(4.0));
        assert_eq!(v.length().to_native(), 5.0);
    }

    #[test]
    fn test_vec2_scale() {
        let v = SoftVec2::new(SoftF32::from_f32(1.0), SoftF32::from_f32(2.0));
        let r = v.scale(SoftF32::from_f32(3.0));
        assert_eq!(r.x.to_native(), 3.0);
        assert_eq!(r.y.to_native(), 6.0);
    }

    #[test]
    fn test_vec2_normalize() {
        let v = SoftVec2::new(SoftF32::from_f32(3.0), SoftF32::from_f32(4.0));
        let n = v.normalize();
        // 3/5 = 0.6, 4/5 = 0.8
        let tolerance = 0.0001_f32;
        assert!((n.x.to_native() - 0.6).abs() < tolerance);
        assert!((n.y.to_native() - 0.8).abs() < tolerance);
    }

    #[test]
    fn test_vec2_normalize_zero() {
        let v = SoftVec2::zero();
        let n = v.normalize();
        assert_eq!(n.x.to_native(), 0.0);
        assert_eq!(n.y.to_native(), 0.0);
    }

    // === SoftVec3 ===

    #[test]
    fn test_vec3_addition() {
        let a = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
        );
        let b = SoftVec3::new(
            SoftF32::from_f32(4.0),
            SoftF32::from_f32(5.0),
            SoftF32::from_f32(6.0),
        );
        let r = a + b;
        assert_eq!(r.x.to_native(), 5.0);
        assert_eq!(r.y.to_native(), 7.0);
        assert_eq!(r.z.to_native(), 9.0);
    }

    #[test]
    fn test_vec3_subtraction() {
        let a = SoftVec3::new(
            SoftF32::from_f32(5.0),
            SoftF32::from_f32(7.0),
            SoftF32::from_f32(9.0),
        );
        let b = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
        );
        let r = a - b;
        assert_eq!(r.x.to_native(), 4.0);
        assert_eq!(r.y.to_native(), 5.0);
        assert_eq!(r.z.to_native(), 6.0);
    }

    #[test]
    fn test_vec3_negation() {
        let a = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(-2.0),
            SoftF32::from_f32(3.0),
        );
        let r = -a;
        assert_eq!(r.x.to_native(), -1.0);
        assert_eq!(r.y.to_native(), 2.0);
        assert_eq!(r.z.to_native(), -3.0);
    }

    #[test]
    fn test_vec3_component_mul() {
        let a = SoftVec3::new(
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
            SoftF32::from_f32(4.0),
        );
        let b = SoftVec3::new(
            SoftF32::from_f32(5.0),
            SoftF32::from_f32(6.0),
            SoftF32::from_f32(7.0),
        );
        let r = a * b;
        assert_eq!(r.x.to_native(), 10.0);
        assert_eq!(r.y.to_native(), 18.0);
        assert_eq!(r.z.to_native(), 28.0);
    }

    #[test]
    fn test_vec3_scalar_mul() {
        let v = SoftVec3::new(
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
            SoftF32::from_f32(4.0),
        );
        let r = v * SoftF32::from_f32(5.0);
        assert_eq!(r.x.to_native(), 10.0);
        assert_eq!(r.y.to_native(), 15.0);
        assert_eq!(r.z.to_native(), 20.0);
    }

    #[test]
    fn test_vec3_component_div() {
        let a = SoftVec3::new(
            SoftF32::from_f32(10.0),
            SoftF32::from_f32(12.0),
            SoftF32::from_f32(21.0),
        );
        let b = SoftVec3::new(
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
            SoftF32::from_f32(7.0),
        );
        let r = a / b;
        assert_eq!(r.x.to_native(), 5.0);
        assert_eq!(r.y.to_native(), 4.0);
        assert_eq!(r.z.to_native(), 3.0);
    }

    #[test]
    fn test_vec3_dot_orthogonal() {
        let a = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(0.0),
        );
        let b = SoftVec3::new(
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(0.0),
        );
        assert_eq!(a.dot(b).to_native(), 0.0); // 正交向量內積為零
    }

    #[test]
    fn test_vec3_length_squared() {
        let v = SoftVec3::new(
            SoftF32::from_f32(3.0),
            SoftF32::from_f32(4.0),
            SoftF32::from_f32(0.0),
        );
        assert_eq!(v.length_squared().to_native(), 25.0);
    }

    #[test]
    fn test_vec3_length() {
        let v = SoftVec3::new(
            SoftF32::from_f32(3.0),
            SoftF32::from_f32(4.0),
            SoftF32::from_f32(0.0),
        );
        assert_eq!(v.length().to_native(), 5.0);
    }

    #[test]
    fn test_vec3_scale() {
        let v = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(2.0),
            SoftF32::from_f32(3.0),
        );
        let r = v.scale(SoftF32::from_f32(2.0));
        assert_eq!(r.x.to_native(), 2.0);
        assert_eq!(r.y.to_native(), 4.0);
        assert_eq!(r.z.to_native(), 6.0);
    }

    #[test]
    fn test_vec3_normalize() {
        let v = SoftVec3::new(
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(3.0),
            SoftF32::from_f32(4.0),
        );
        let n = v.normalize();
        let tolerance = 0.0001_f32;
        assert!(n.x.to_native().abs() < tolerance);
        assert!((n.y.to_native() - 0.6).abs() < tolerance);
        assert!((n.z.to_native() - 0.8).abs() < tolerance);
    }

    #[test]
    fn test_vec3_normalize_zero() {
        let v = SoftVec3::zero();
        let n = v.normalize();
        assert_eq!(n.x.to_native(), 0.0);
        assert_eq!(n.y.to_native(), 0.0);
        assert_eq!(n.z.to_native(), 0.0);
    }

    #[test]
    fn test_vec3_cross() {
        // i x j = k（標準基向量外積）
        let a = SoftVec3::new(
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(0.0),
        );
        let b = SoftVec3::new(
            SoftF32::from_f32(0.0),
            SoftF32::from_f32(1.0),
            SoftF32::from_f32(0.0),
        );
        let r = a.cross(b);
        assert_eq!(r.x.to_native(), 0.0);
        assert_eq!(r.y.to_native(), 0.0);
        assert_eq!(r.z.to_native(), 1.0);
    }
}
