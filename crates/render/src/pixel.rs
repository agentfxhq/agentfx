//! Exact byte quantization shared by the pixel passes.
/// Equivalent to `x.round() as u8`, including saturation and NaN -> 0.
/// After saturation x is in [0,255]. Its integer part is exact in f32;
/// subtraction is exact (Sterbenz for x >= 1, trivial below 1), so comparing
/// the fraction to 0.5 implements ties away from zero without an addition
/// rounding a value immediately below a half-integer up to the tie.
/// Negative results saturate to zero regardless of their rounded value.
#[inline(always)]
pub fn round_u8(x: f32) -> u8 {
    let x = x.max(0.0).min(255.0);
    let n = x as u32;
    (n + u32::from(x - n as f32 >= 0.5)) as u8
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn round8(x: std::arch::x86_64::__m256) -> std::arch::x86_64::__m256i {
    use std::arch::x86_64::*;
    let x = _mm256_min_ps(_mm256_max_ps(x, _mm256_setzero_ps()), _mm256_set1_ps(255.0));
    let n = _mm256_cvttps_epi32(x);
    let fraction = _mm256_sub_ps(x, _mm256_cvtepi32_ps(n));
    let up = _mm256_cmp_ps::<_CMP_GE_OQ>(fraction, _mm256_set1_ps(0.5));
    _mm256_sub_epi32(n, _mm256_castps_si256(up))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn simd_rounding_matches_original() {
        if !is_x86_feature_detected!("avx2") { return; }
        use std::arch::x86_64::*;
        let mut rng = Rng(9081);
        for i in 0..10000 {
            let values: [f32; 8] = if i < 256 {
                let bits = (i as f32 + 0.5).to_bits();
                std::array::from_fn(|j| f32::from_bits(bits - 3 + j as u32))
            } else {
                std::array::from_fn(|_| f32::from_bits(rng.next()))
            };
            let mut actual = [0i32; 8];
            unsafe {
                _mm256_storeu_si256(actual.as_mut_ptr().cast(), round8(_mm256_loadu_ps(values.as_ptr())));
            }
            assert_eq!(actual, values.map(|x| x.round() as u8 as i32));
        }
    }

    #[test]
    fn rounding_boundaries() {
        for i in 0..256 {
            let tie = (i as f32 + 0.5).to_bits();
            for bits in tie-2..=tie+2 {
                for x in [f32::from_bits(bits), -f32::from_bits(bits)] {
                    assert_eq!(round_u8(x), x.round() as u8, "{x:?}");
                }
            }
        }
        for x in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.0, 0.0, f32::MAX] {
            assert_eq!(round_u8(x), x.round() as u8);
        }
        let mut rng = 7u32;
        for _ in 0..100_000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = f32::from_bits(rng);
            assert_eq!(round_u8(x), x.round() as u8);
        }
    }
}

#[cfg(test)]
pub struct Rng(pub u32);
#[cfg(test)]
impl Rng {
    pub fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0
    }
    pub fn unit(&mut self) -> f32 { (self.next() >> 8) as f32 / 16777216.0 }
}
