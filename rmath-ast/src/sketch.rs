//! Constant-memory sketches behind the approximate aggregates.
//!
//! - [`HyperLogLog`] estimates the number of distinct items
//!   (`approx_count_distinct`). It keeps `2^p` one-byte registers and uses
//!   Ertl's improved estimator ("New cardinality estimation algorithms for
//!   HyperLogLog sketches", 2017), which is accurate from a handful of items
//!   up without HLL++'s bias tables. The standard error is `1.04 / sqrt(2^p)`.
//! - [`Kll`] estimates quantiles (`approx_quantile`, `median`). It is the KLL
//!   sketch of Karnin, Lang and Liberty ("Optimal quantile approximation in
//!   streams", 2016) with lazy compaction, keeping about `3k` items. With
//!   `k = 200` the rank error is around 1.5%; up to `k` items it is exact.
//!
//! Both merge, so sketches of separate partitions combine into the sketch of
//! the whole input. Both are deterministic: the same input in the same order
//! gives the same result on every run.

/// A HyperLogLog distinct-count sketch with `2^p` registers.
#[derive(Debug, Clone)]
pub struct HyperLogLog {
    p: u8,
    registers: Vec<u8>,
}

impl HyperLogLog {
    pub const MIN_PRECISION: u8 = 4;
    pub const MAX_PRECISION: u8 = 18;
    pub const DEFAULT_PRECISION: u8 = 14;

    /// # Panics
    ///
    /// If `p` is outside `MIN_PRECISION..=MAX_PRECISION`.
    pub fn new(p: u8) -> Self {
        assert!(
            (Self::MIN_PRECISION..=Self::MAX_PRECISION).contains(&p),
            "HyperLogLog precision {} out of range",
            p
        );
        HyperLogLog {
            p,
            registers: vec![0; 1 << p],
        }
    }

    pub fn precision(&self) -> u8 {
        self.p
    }

    /// Add an item, given as its bytes.
    pub fn insert(&mut self, item: &[u8]) {
        self.insert_hash(hash64(item));
    }

    fn insert_hash(&mut self, h: u64) {
        let p = u32::from(self.p);
        let index = (h >> (64 - p)) as usize;
        // The position of the first 1 bit in the remaining 64 - p bits,
        // capped at 64 - p + 1 when they are all zero.
        let rank = ((h << p).leading_zeros().min(64 - p) + 1) as u8;
        let register = &mut self.registers[index];
        *register = (*register).max(rank);
    }

    /// Fold in another sketch, as if its items had been inserted here.
    ///
    /// # Panics
    ///
    /// If the precisions differ.
    pub fn merge(&mut self, other: &HyperLogLog) {
        assert_eq!(
            self.p, other.p,
            "merging HyperLogLogs of different precision"
        );
        for (mine, theirs) in self.registers.iter_mut().zip(&other.registers) {
            *mine = (*mine).max(*theirs);
        }
    }

    /// The estimated number of distinct items inserted.
    pub fn estimate(&self) -> f64 {
        let q = 64 - usize::from(self.p);
        let mut counts = vec![0u32; q + 2];
        for &r in &self.registers {
            counts[usize::from(r)] += 1;
        }
        let m = self.registers.len() as f64;
        if f64::from(counts[0]) == m {
            return 0.0;
        }
        let mut z = m * tau(1.0 - f64::from(counts[q + 1]) / m);
        for k in (1..=q).rev() {
            z = 0.5 * (z + f64::from(counts[k]));
        }
        z += m * sigma(f64::from(counts[0]) / m);
        m * m / (2.0 * std::f64::consts::LN_2 * z)
    }
}

fn sigma(mut x: f64) -> f64 {
    if x == 1.0 {
        return f64::INFINITY;
    }
    let mut y = 1.0;
    let mut z = x;
    loop {
        x *= x;
        let previous = z;
        z += x * y;
        y += y;
        if z == previous {
            return z;
        }
    }
}

fn tau(mut x: f64) -> f64 {
    if x == 0.0 || x == 1.0 {
        return 0.0;
    }
    let mut y = 1.0;
    let mut z = 1.0 - x;
    loop {
        x = x.sqrt();
        let previous = z;
        y *= 0.5;
        z -= (1.0 - x).powi(2) * y;
        if z == previous {
            return z / 3.0;
        }
    }
}

/// FNV-1a followed by the MurmurHash3 finalizer, so that every input bit
/// affects every output bit. Fixed, unlike `std`'s hasher, so estimates are
/// the same on every run and Rust version.
fn hash64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// A KLL quantile sketch over `f64`s.
#[derive(Debug, Clone)]
pub struct Kll {
    k: usize,
    /// `levels[h]` holds items of weight `2^h`.
    levels: Vec<Vec<f64>>,
    /// Items held across all levels.
    size: usize,
    /// Items the levels may hold before one is compacted.
    max_size: usize,
    /// Items inserted.
    n: u64,
    /// State of the coin that picks which half of a level survives.
    coin: u64,
}

impl Kll {
    pub const DEFAULT_K: usize = 200;

    /// # Panics
    ///
    /// If `k < 2`.
    pub fn new(k: usize) -> Self {
        assert!(k >= 2, "KLL k must be at least 2");
        let mut sketch = Kll {
            k,
            levels: Vec::new(),
            size: 0,
            max_size: 0,
            n: 0,
            coin: 0x9e37_79b9_7f4a_7c15,
        };
        sketch.grow();
        sketch
    }

    /// The number of items inserted.
    pub fn len(&self) -> u64 {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn insert(&mut self, x: f64) {
        self.levels[0].push(x);
        self.size += 1;
        self.n += 1;
        if self.size >= self.max_size {
            self.compress();
        }
    }

    /// Fold in another sketch, as if its items had been inserted here.
    pub fn merge(&mut self, other: &Kll) {
        while self.levels.len() < other.levels.len() {
            self.grow();
        }
        for (mine, theirs) in self.levels.iter_mut().zip(&other.levels) {
            mine.extend_from_slice(theirs);
        }
        self.size += other.size;
        self.n += other.n;
        while self.size >= self.max_size {
            self.compress();
        }
    }

    /// The smallest retained item whose rank is at least `q * n` (the
    /// nearest-rank method: `quantile(0.5)` of `1, 2, 3, 4` is 2), or `None`
    /// when empty. `q` is clamped to `0..=1`.
    pub fn quantile(&self, q: f64) -> Option<f64> {
        if self.n == 0 {
            return None;
        }
        let mut weighted: Vec<(f64, u64)> = self
            .levels
            .iter()
            .enumerate()
            .flat_map(|(h, level)| level.iter().map(move |&x| (x, 1u64 << h)))
            .collect();
        weighted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let target = ((q.clamp(0.0, 1.0) * self.n as f64).ceil() as u64).max(1);
        let mut seen = 0;
        for &(x, weight) in &weighted {
            seen += weight;
            if seen >= target {
                return Some(x);
            }
        }
        weighted.last().map(|&(x, _)| x)
    }

    fn capacity(&self, h: usize) -> usize {
        let depth = (self.levels.len() - h - 1) as i32;
        (self.k as f64 * (2.0f64 / 3.0).powi(depth)).ceil() as usize + 1
    }

    fn grow(&mut self) {
        self.levels.push(Vec::new());
        self.max_size = (0..self.levels.len()).map(|h| self.capacity(h)).sum();
    }

    /// Compact the lowest full level: sort it and promote every other item,
    /// starting at a random one of the first two, to the level above with
    /// double weight. An odd item out stays behind, so the total weight stays
    /// equal to `n`.
    fn compress(&mut self) {
        for h in 0..self.levels.len() {
            if self.levels[h].len() < self.capacity(h) {
                continue;
            }
            if h + 1 == self.levels.len() {
                self.grow();
            }
            let mut level = std::mem::take(&mut self.levels[h]);
            level.sort_unstable_by(f64::total_cmp);
            let keep = level.len() % 2;
            let offset = keep + usize::from(self.flip());
            let promoted: Vec<f64> = level[offset..].iter().step_by(2).copied().collect();
            self.size -= level.len() - keep - promoted.len();
            level.truncate(keep);
            self.levels[h] = level;
            self.levels[h + 1].extend(promoted);
            return;
        }
    }

    /// A fair coin from xorshift64*.
    fn flip(&mut self) -> bool {
        self.coin ^= self.coin >> 12;
        self.coin ^= self.coin << 25;
        self.coin ^= self.coin >> 27;
        self.coin.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 63 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hll_of(n: u64, p: u8) -> HyperLogLog {
        let mut hll = HyperLogLog::new(p);
        for i in 0..n {
            hll.insert(&i.to_le_bytes());
        }
        hll
    }

    #[test]
    fn test_hll_small_counts_are_exact() {
        for n in [0, 1, 2, 3, 10, 100] {
            assert_eq!(hll_of(n, 14).estimate().round() as u64, n);
        }
        // duplicates don't count
        let mut hll = HyperLogLog::new(14);
        for _ in 0..1000 {
            hll.insert(b"same");
        }
        assert_eq!(hll.estimate().round(), 1.0);
    }

    #[test]
    fn test_hll_error_is_within_bounds() {
        for (n, p) in [(1_000, 10), (50_000, 14), (300_000, 12)] {
            let error = 1.04 / f64::from(1u32 << p).sqrt();
            let estimate = hll_of(n, p).estimate();
            let relative = (estimate - n as f64).abs() / n as f64;
            assert!(
                relative < 4.0 * error,
                "n {} p {}: estimate {} off by {}",
                n,
                p,
                estimate,
                relative
            );
        }
    }

    #[test]
    fn test_hll_merge_is_union() {
        let mut a = HyperLogLog::new(12);
        let mut b = HyperLogLog::new(12);
        for i in 0u32..6000 {
            a.insert(&i.to_le_bytes());
        }
        for i in 4000u32..10000 {
            b.insert(&i.to_le_bytes());
        }
        a.merge(&b);
        assert_eq!(a.registers, hll_of_u32(0..10000, 12).registers);
    }

    fn hll_of_u32(range: std::ops::Range<u32>, p: u8) -> HyperLogLog {
        let mut hll = HyperLogLog::new(p);
        for i in range {
            hll.insert(&i.to_le_bytes());
        }
        hll
    }

    /// The exact rank of `x` in `0..n`, as a fraction.
    fn rank_error(x: f64, q: f64, n: u64) -> f64 {
        ((x + 1.0) / n as f64 - q).abs()
    }

    #[test]
    fn test_kll_is_exact_up_to_k() {
        let mut kll = Kll::new(Kll::DEFAULT_K);
        assert_eq!(kll.quantile(0.5), None);
        for x in [4.0, 1.0, 3.0, 2.0] {
            kll.insert(x);
        }
        assert_eq!(kll.quantile(0.5), Some(2.0));
        assert_eq!(kll.quantile(0.0), Some(1.0));
        assert_eq!(kll.quantile(1.0), Some(4.0));
        assert_eq!(kll.quantile(0.75), Some(3.0));
        assert_eq!(kll.quantile(0.76), Some(4.0));

        let mut kll = Kll::new(Kll::DEFAULT_K);
        for x in (0..200).rev() {
            kll.insert(f64::from(x));
        }
        for q in [0.01, 0.1, 0.5, 0.9, 0.99] {
            assert_eq!(kll.quantile(q), Some((q * 200.0).ceil() - 1.0));
        }
    }

    #[test]
    fn test_kll_rank_error_is_small() {
        let n = 200_000u64;
        let mut kll = Kll::new(Kll::DEFAULT_K);
        // a scrambled order of 0..n
        for i in 0..n {
            kll.insert(((i * 7919) % n) as f64);
        }
        assert_eq!(kll.len(), n);
        let total: u64 = kll
            .levels
            .iter()
            .enumerate()
            .map(|(h, l)| (l.len() as u64) << h)
            .sum();
        assert_eq!(total, n);
        assert!(kll.size < 4 * Kll::DEFAULT_K, "{} items kept", kll.size);
        for q in [0.0, 0.01, 0.25, 0.5, 0.9, 0.99, 1.0] {
            let x = kll.quantile(q).unwrap();
            assert!(rank_error(x, q, n) < 0.02, "q {}: {}", q, x);
        }
    }

    #[test]
    fn test_kll_merge() {
        let n = 100_000u64;
        let mut parts: Vec<Kll> = (0..4).map(|_| Kll::new(Kll::DEFAULT_K)).collect();
        for i in 0..n {
            parts[(i % 4) as usize].insert(i as f64);
        }
        let mut whole = parts.remove(0);
        for part in &parts {
            whole.merge(part);
        }
        assert_eq!(whole.len(), n);
        assert!(whole.size < 4 * Kll::DEFAULT_K);
        for q in [0.1, 0.5, 0.95] {
            let x = whole.quantile(q).unwrap();
            assert!(rank_error(x, q, n) < 0.02, "q {}: {}", q, x);
        }
    }
}
