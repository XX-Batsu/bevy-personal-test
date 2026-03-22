//! SoftF32 property-based 確定性測試。
//! 透過 proptest 產生隨機輸入，驗證軟體浮點的數學性質與 bit-exact 一致性。

use deterministic::SoftF32;
use proptest::prelude::*;

/// 產生有限 SoftF32（排除 NaN / Inf）。
fn arb_soft_f32() -> impl Strategy<Value = SoftF32> {
    prop::num::f32::NORMAL.prop_map(SoftF32::from_f32)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn addition_commutativity(a in arb_soft_f32(), b in arb_soft_f32()) {
        // 加法交換律：bit-level 嚴格相等
        prop_assert_eq!(
            (a + b).to_bits(),
            (b + a).to_bits(),
            "加法交換律失敗: a={:?}, b={:?}", a, b
        );
    }

    #[test]
    fn multiplication_commutativity(a in arb_soft_f32(), b in arb_soft_f32()) {
        // 乘法交換律：bit-level 嚴格相等
        prop_assert_eq!(
            (a * b).to_bits(),
            (b * a).to_bits(),
            "乘法交換律失敗: a={:?}, b={:?}", a, b
        );
    }

    #[test]
    fn bit_exact_reproducibility(
        a_bits in proptest::num::u32::ANY,
        b_bits in proptest::num::u32::ANY
    ) {
        // 同一 bit pattern 輸入，重複加法運算結果必須 bit-identical
        let a = SoftF32::from_bits(a_bits);
        let b = SoftF32::from_bits(b_bits);
        let r1 = (a + b).to_bits();
        let r2 = (a + b).to_bits();
        prop_assert_eq!(
            r1, r2,
            "Bit-exact 重現性失敗: a_bits=0x{:08X}, b_bits=0x{:08X}", a_bits, b_bits
        );
    }

    #[test]
    fn from_f32_round_trip(bits in proptest::num::u32::ANY) {
        // from_bits → to_bits 必須完全保留原始 bits
        let sf = SoftF32::from_bits(bits);
        prop_assert_eq!(
            sf.to_bits(),
            bits,
            "Round-trip 失敗: 輸入 bits=0x{:08X}, 輸出=0x{:08X}",
            bits,
            sf.to_bits()
        );
    }
}
