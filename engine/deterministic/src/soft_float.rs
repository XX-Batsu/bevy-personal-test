//! 確定性 f32 — 內部儲存 IEEE 754 bits，所有算術透過 softfloat 軟體實作。
//!
//! # 設計決策
//!
//! - 內部型別為 `u32`（IEEE 754 binary32 bit pattern）
//! - 所有算術使用 TiesToEven（softfloat 1.0 硬編碼捨入模式）
//! - `Eq` + `Hash` 基於 bit pattern（`u32` 的 derive），非數值語義
//!   → 同一個 NaN bit pattern 會被視為相等（符合確定性需求）
//! - `PartialOrd` 透過 softfloat 比較 API，NaN 回傳 `None`

use serde::{Deserialize, Serialize};
use softfloat::F32;
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// 確定性 f32 — 內部儲存 IEEE 754 bits，所有算術透過 softfloat 軟體實作。
///
/// # 設計決策
/// - 內部型別為 `u32`（IEEE 754 binary32 bit pattern）
/// - 所有算術使用 TiesToEven（softfloat 1.0 硬編碼捨入模式）
/// - `Eq` + `Hash` 基於 bit pattern（`u32` 的 derive），非數值語義
///   → 同一個 NaN bit pattern 會被視為相等（符合確定性需求）
/// - `PartialOrd` 透過 softfloat 比較 API，NaN 回傳 `None`
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(C)]
pub struct SoftF32(pub u32);

impl SoftF32 {
    /// IEEE 754 正零（0x0000_0000）
    pub const ZERO: Self = Self(0);

    /// IEEE 754 binary32 的 1.0
    pub const ONE: Self = Self(0x3F80_0000);

    /// IEEE 754 binary32 的 -1.0
    pub const NEG_ONE: Self = Self(0xBF80_0000);

    /// 從原生 f32 建立 SoftF32。
    /// 僅在初始化常數或從外部資料轉入時使用。
    pub fn from_f32(v: f32) -> Self {
        Self(v.to_bits())
    }

    /// 從 IEEE 754 bit pattern 建立 SoftF32。
    /// 用於反序列化或 State Hash 還原。
    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// 轉回原生 f32。
    /// **僅限渲染層與 Debug 使用**，遊戲邏輯中禁止呼叫。
    pub fn to_native(self) -> f32 {
        f32::from_bits(self.0)
    }

    /// 取得 IEEE 754 bit pattern。
    /// 用於 State Hash 計算與序列化。
    pub fn to_bits(self) -> u32 {
        self.0
    }

    /// 平方根（softfloat 實作，TiesToEven 硬編碼）。
    pub fn sqrt(self) -> Self {
        let f = F32::from_bits(self.0);
        Self(f.sqrt().to_bits())
    }

    /// 檢查是否為 NaN。
    /// 使用 IEEE 754 bit pattern 判斷：
    /// exponent 全 1（bits 30..23）、mantissa 非 0（bits 22..0）。
    pub fn is_nan(self) -> bool {
        let bits = self.0;
        (bits & 0x7F80_0000) == 0x7F80_0000 && (bits & 0x007F_FFFF) != 0
    }

    /// 檢查是否為 ±Infinity。
    /// 使用 IEEE 754 bit pattern 判斷：exponent 全 1、mantissa 全 0。
    pub fn is_inf(self) -> bool {
        (self.0 & 0x7FFF_FFFF) == 0x7F80_0000
    }

    /// 絕對值（清除 sign bit）
    pub fn abs(self) -> Self {
        Self(self.0 & 0x7FFF_FFFF)
    }

    /// 從 f64 轉換（透過 softfloat F64→F32 降精度，TiesToEven 硬編碼）
    pub fn from_f64(v: f64) -> Self {
        let f64_val = softfloat::F64::from_bits(v.to_bits());
        Self(F32::from_f64(f64_val).to_bits())
    }

    /// 轉為 f64（透過 softfloat F32→F64 升精度）
    pub fn to_f64(self) -> f64 {
        let f32_val = F32::from_bits(self.0);
        f64::from_bits(f32_val.to_f64().to_bits())
    }

    /// 正弦（softfloat 1.0 F32 內建 sin()）
    pub fn sin(self) -> Self {
        let f = F32::from_bits(self.0);
        Self(f.sin().to_bits())
    }

    /// 餘弦（softfloat 1.0 F32 內建 cos()）
    pub fn cos(self) -> Self {
        let f = F32::from_bits(self.0);
        Self(f.cos().to_bits())
    }

    /// 便利比較：self < rhs
    pub fn lt(self, rhs: Self) -> bool {
        matches!(self.partial_cmp(&rhs), Some(Ordering::Less))
    }

    /// 便利比較：self > rhs
    pub fn gt(self, rhs: Self) -> bool {
        matches!(self.partial_cmp(&rhs), Some(Ordering::Greater))
    }

    /// 便利比較：self <= rhs
    pub fn le(self, rhs: Self) -> bool {
        matches!(
            self.partial_cmp(&rhs),
            Some(Ordering::Less | Ordering::Equal)
        )
    }

    /// 便利比較：self >= rhs
    pub fn ge(self, rhs: Self) -> bool {
        matches!(
            self.partial_cmp(&rhs),
            Some(Ordering::Greater | Ordering::Equal)
        )
    }
}

// ── 算術 Trait ──────────────────────────────────────────────────────────

impl Add for SoftF32 {
    type Output = Self;

    /// softfloat F32 `+` 運算子
    fn add(self, rhs: Self) -> Self {
        let a = F32::from_bits(self.0);
        let b = F32::from_bits(rhs.0);
        Self(a.add(b).to_bits())
    }
}

impl Sub for SoftF32 {
    type Output = Self;

    /// softfloat F32 `-` 運算子
    fn sub(self, rhs: Self) -> Self {
        let a = F32::from_bits(self.0);
        let b = F32::from_bits(rhs.0);
        Self(a.sub(b).to_bits())
    }
}

impl Mul for SoftF32 {
    type Output = Self;

    /// softfloat F32 `*` 運算子
    fn mul(self, rhs: Self) -> Self {
        let a = F32::from_bits(self.0);
        let b = F32::from_bits(rhs.0);
        Self(a.mul(b).to_bits())
    }
}

impl Div for SoftF32 {
    type Output = Self;

    /// softfloat F32 `/` 運算子
    fn div(self, rhs: Self) -> Self {
        let a = F32::from_bits(self.0);
        let b = F32::from_bits(rhs.0);
        Self(a.div(b).to_bits())
    }
}

impl Neg for SoftF32 {
    type Output = Self;

    /// IEEE 754: 翻轉 sign bit 即可取反
    fn neg(self) -> Self {
        Self(self.0 ^ 0x8000_0000)
    }
}

// ── 比較 Trait ──────────────────────────────────────────────────────────

impl PartialOrd for SoftF32 {
    /// 透過 softfloat F32 比較實作。NaN 比較回傳 `None`（符合 IEEE 754 語義）。
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let a = F32::from_bits(self.0);
        let b = F32::from_bits(other.0);
        a.partial_cmp(&b)
    }
}

// ── 格式化 Trait ────────────────────────────────────────────────────────

impl fmt::Debug for SoftF32 {
    /// 格式：`SoftF32(3.140000)` — 六位小數，方便除錯時觀察精度
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SoftF32({:.6})", self.to_native())
    }
}

impl fmt::Display for SoftF32 {
    /// 格式：原生 f32 的預設 Display — 與 `format!("{}", 3.14_f32)` 一致
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_native())
    }
}

// ── 測試模組 ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 3.1 基本轉換 ──

    #[test]
    fn test_from_f32_to_bits_round_trip() {
        let v = SoftF32::from_f32(3.14);
        let restored = SoftF32::from_bits(v.to_bits());
        assert_eq!(v, restored);
    }

    #[test]
    fn test_to_native() {
        let v = SoftF32::from_f32(42.0);
        assert_eq!(v.to_native(), 42.0_f32);
    }

    // ── 3.2 四則運算 ──

    #[test]
    fn test_addition() {
        let r = SoftF32::from_f32(1.5) + SoftF32::from_f32(2.5);
        assert_eq!(r.to_native(), 4.0);
    }

    #[test]
    fn test_subtraction() {
        let r = SoftF32::from_f32(5.0) - SoftF32::from_f32(3.0);
        assert_eq!(r.to_native(), 2.0);
    }

    #[test]
    fn test_multiplication() {
        let r = SoftF32::from_f32(3.0) * SoftF32::from_f32(4.0);
        assert_eq!(r.to_native(), 12.0);
    }

    #[test]
    fn test_division() {
        let r = SoftF32::from_f32(10.0) / SoftF32::from_f32(4.0);
        assert_eq!(r.to_native(), 2.5);
    }

    #[test]
    fn test_negation() {
        let r = -SoftF32::from_f32(3.0);
        assert_eq!(r.to_native(), -3.0);
    }

    // ── 3.3 跨平台 Bit-Exact 一致性 ──

    #[test]
    fn test_cross_platform_bit_consistency() {
        let r = SoftF32::from_f32(1.0) / SoftF32::from_f32(3.0);
        assert_eq!(
            r.to_bits(),
            0x3EAAAAAB,
            "1.0/3.0 的 softfloat 結果必須是 0x3EAAAAAB（跨平台一致）"
        );
    }

    // ── 3.4 比較運算（PartialOrd）──

    #[test]
    fn test_partial_ord_less_than() {
        assert!(SoftF32::from_f32(1.0) < SoftF32::from_f32(2.0));
    }

    #[test]
    fn test_partial_ord_greater_than() {
        assert!(SoftF32::from_f32(5.0) > SoftF32::from_f32(3.0));
    }

    #[test]
    fn test_partial_ord_equal() {
        let a = SoftF32::from_f32(2.5);
        let b = SoftF32::from_f32(2.5);
        assert!(!(a < b) && !(a > b));
    }

    #[test]
    fn test_partial_ord_negative() {
        assert!(SoftF32::from_f32(-1.0) < SoftF32::from_f32(0.0));
    }

    // ── 3.5 數學函數 ──

    #[test]
    fn test_sqrt() {
        let r = SoftF32::from_f32(25.0).sqrt();
        assert_eq!(r.to_native(), 5.0);
    }

    #[test]
    fn test_sqrt_zero() {
        let r = SoftF32::from_f32(0.0).sqrt();
        assert_eq!(r.to_native(), 0.0);
    }

    // ── 3.6 特殊值檢查（NaN / Inf）──

    #[test]
    fn test_is_nan() {
        let nan = SoftF32::from_f32(0.0) / SoftF32::from_f32(0.0);
        assert!(nan.is_nan(), "0.0 / 0.0 應產生 NaN");
        assert!(!SoftF32::from_f32(1.0).is_nan(), "正常值不應為 NaN");
    }

    #[test]
    fn test_is_inf() {
        let inf = SoftF32::from_f32(1.0) / SoftF32::from_f32(0.0);
        assert!(inf.is_inf(), "1.0 / 0.0 應產生 Inf");
        assert!(!SoftF32::from_f32(1.0).is_inf(), "正常值不應為 Inf");
    }

    // ── 3.7 格式化輸出 ──

    #[test]
    fn test_display() {
        let v = SoftF32::from_f32(3.14);
        let s = format!("{}", v);
        assert!(s.contains("3.14"), "Display 輸出 '{}' 應包含 '3.14'", s);
    }

    // ── 3.8 常數 ──

    #[test]
    fn test_zero_const() {
        assert_eq!(SoftF32::ZERO.to_native(), 0.0_f32);
        assert_eq!(SoftF32::ZERO.to_bits(), 0x0000_0000);
    }

    // ── 3.9 邊界強化測試 ──

    #[test]
    fn test_negative_zero() {
        let nz = SoftF32::from_f32(-0.0);
        assert_eq!(
            nz.to_bits(),
            0x8000_0000,
            "負零的 bit pattern 必須為 0x80000000"
        );
    }

    #[test]
    fn test_serde_round_trip() {
        let v = SoftF32::from_f32(3.14);
        let bytes = bincode::serialize(&v).unwrap();
        let restored: SoftF32 = bincode::deserialize(&bytes).unwrap();
        assert_eq!(v.to_bits(), restored.to_bits());
    }

    #[test]
    fn test_sqrt_negative() {
        let r = SoftF32::from_f32(-1.0).sqrt();
        assert!(r.is_nan(), "負數開根號應產生 NaN");
    }

    #[test]
    fn test_determinism_double_run() {
        let r1 = SoftF32::from_f32(1.0) / SoftF32::from_f32(3.0);
        let r2 = SoftF32::from_f32(1.0) / SoftF32::from_f32(3.0);
        assert_eq!(
            r1.to_bits(),
            r2.to_bits(),
            "相同輸入的重複運算必須產生相同 bits"
        );
    }

    // ── 3.10 擴充方法測試 ──

    #[test]
    fn test_one_const() {
        assert_eq!(SoftF32::ONE.to_native(), 1.0_f32);
        assert_eq!(SoftF32::ONE.to_bits(), 0x3F80_0000);
    }

    #[test]
    fn test_neg_one_const() {
        assert_eq!(SoftF32::NEG_ONE.to_native(), -1.0_f32);
        assert_eq!(SoftF32::NEG_ONE.to_bits(), 0xBF80_0000);
    }

    #[test]
    fn test_abs_positive() {
        assert_eq!(SoftF32::from_f32(3.0).abs().to_native(), 3.0);
    }

    #[test]
    fn test_abs_negative() {
        assert_eq!(SoftF32::from_f32(-5.0).abs().to_native(), 5.0);
    }

    #[test]
    fn test_abs_negative_zero() {
        assert_eq!(SoftF32::from_f32(-0.0).abs().to_bits(), 0x0000_0000);
    }

    #[test]
    fn test_from_f64_round_trip() {
        let v = SoftF32::from_f64(3.14);
        let diff = (v.to_f64() - 3.14_f64).abs();
        assert!(diff < 0.001, "f64 round-trip 誤差過大: {}", diff);
    }

    #[test]
    fn test_sin_zero() {
        let r = SoftF32::ZERO.sin();
        assert_eq!(r.to_native(), 0.0, "sin(0) 應為 0");
    }

    #[test]
    fn test_cos_zero() {
        let r = SoftF32::ZERO.cos();
        assert_eq!(r.to_native(), 1.0, "cos(0) 應為 1");
    }

    #[test]
    fn test_lt_method() {
        assert!(SoftF32::from_f32(1.0).lt(SoftF32::from_f32(2.0)));
        assert!(!SoftF32::from_f32(2.0).lt(SoftF32::from_f32(1.0)));
    }

    #[test]
    fn test_ge_method() {
        assert!(SoftF32::from_f32(2.0).ge(SoftF32::from_f32(2.0)));
        assert!(SoftF32::from_f32(3.0).ge(SoftF32::from_f32(2.0)));
        assert!(!SoftF32::from_f32(1.0).ge(SoftF32::from_f32(2.0)));
    }
}
