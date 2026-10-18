//! Seeded randomness in the style the game uses: a xorshift128 generator seeded the way Unity's
//! `Random.InitState` is, with the same range conventions (integer ranges exclusive at the top, float
//! ranges inclusive). Missions with a fixed seed (the tutorial's is 452) depend on it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rng {
    s: [u32; 4],
}

impl Rng {
    pub fn new(seed: i32) -> Rng {
        let s0 = seed as u32;
        let s1 = s0.wrapping_mul(1_812_433_253).wrapping_add(1);
        let s2 = s1.wrapping_mul(1_812_433_253).wrapping_add(1);
        let s3 = s2.wrapping_mul(1_812_433_253).wrapping_add(1);
        Rng { s: [s0, s1, s2, s3] }
    }

    /// A seed from the clock, for missions without a fixed one.
    pub fn any_seed() -> i32 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(1);
        (nanos as u64 ^ (nanos >> 64) as u64) as i32
    }

    pub fn next_u32(&mut self) -> u32 {
        let [x, y, z, w] = self.s;
        let t = x ^ (x << 11);
        let w2 = w ^ (w >> 19) ^ t ^ (t >> 8);
        self.s = [y, z, w, w2];
        w2
    }

    /// In [0, 1].
    pub fn value(&mut self) -> f32 {
        (self.next_u32() & 0x7F_FFFF) as f32 / 8_388_607.0
    }

    /// Integer in [min, max); `min` when the range is empty.
    pub fn range(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + (self.next_u32() % (max - min) as u32) as i32
    }

    /// Float in [min, max].
    pub fn range_f(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.value()
    }

    pub fn shuffle<T>(&mut self, list: &mut [T]) {
        for i in 0..list.len() {
            let j = self.range(i as i32, list.len() as i32) as usize;
            list.swap(i, j);
        }
    }

    pub fn pick<'a, T>(&mut self, list: &'a [T]) -> Option<&'a T> {
        if list.is_empty() {
            return None;
        }
        list.get(self.range(0, list.len() as i32) as usize)
    }

    pub fn chance(&mut self, percent: f32) -> bool {
        self.range_f(0.0, 100.0) < percent
    }
}

/// Unity's `Mathf.RoundToInt`: halves round to even.
pub fn round(x: f32) -> i32 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && (r as i32) % 2 != 0 { (r - x.signum()) as i32 } else { r as i32 }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}
