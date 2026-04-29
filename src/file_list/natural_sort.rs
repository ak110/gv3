//! 自然順ソート用の軽量PRNG実装。

/// 軽量PRNG(xorshift64)。シャッフル用途のため暗号強度は不要。
pub(crate) struct SimpleRng(u64);

impl SimpleRng {
    pub(crate) fn new() -> Self {
        let mut buf = [0u8; 8];
        // OS 乱数源が取れない場合はシステム時刻ベースのシードにフォールバックする。
        // シャッフル用途のため暗号強度は不要で、決定的な再現を避けられれば十分。
        let seed = if getrandom::fill(&mut buf).is_ok() {
            u64::from_ne_bytes(buf)
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64)
        };
        Self(seed | 1) // 0シード回避
    }

    /// xorshift64ステップを実行し、次の状態を返す
    pub(crate) fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// [0, bound) の範囲で一様分布する乱数を返す (Lemire法)
    ///
    /// 参考: Daniel Lemire, "Fast Random Integer Generation in an Interval",
    /// ACM Trans. Model. Comput. Simul., 2019
    pub(crate) fn next_usize(&mut self, bound: usize) -> usize {
        let s = bound as u64;
        let mut m = self.step() as u128 * s as u128;
        let mut l = m as u64;
        if l < s {
            // rejection threshold: (2^64 - s) % s
            let t = s.wrapping_neg() % s;
            while l < t {
                m = self.step() as u128 * s as u128;
                l = m as u64;
            }
        }
        (m >> 64) as usize
    }
}
