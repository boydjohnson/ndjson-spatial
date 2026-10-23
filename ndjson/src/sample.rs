/*
 * Copyright 2026 Boyd Johnson
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! `sample`: a uniform random sample of the stream, either exactly *k*
//! records (reservoir sampling, Algorithm L) or each record with
//! probability *p* (Bernoulli). Records pass through unparsed.

use crate::{
    head::flush,
    records::{for_each_record, write_record, Flow},
};
use ndjson_common::error::NdJsonSpatialError;
use std::io::{BufRead, Write};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sampling {
    /// Exactly this many records (all of them if the input is shorter),
    /// written in input order. Memory holds these records.
    Reservoir(usize),
    /// Each record independently with this probability, in `[0, 1]`.
    Rate(f64),
}

/// Writes a sample of `input` to `output`. The same `seed` and input give
/// the same sample.
pub fn sample<I: BufRead, O: Write>(
    sampling: Sampling,
    seed: u64,
    input: &mut I,
    output: &mut O,
) -> Result<(), NdJsonSpatialError> {
    let mut rng = Rng::new(seed);
    match sampling {
        Sampling::Reservoir(k) => reservoir(k, &mut rng, input, output),
        Sampling::Rate(p) => {
            for_each_record(input, |record| {
                if rng.open01() < p && !write_record(output, record)? {
                    return Ok(Flow::Stop);
                }
                Ok(Flow::Continue)
            })?;
            flush(output)
        }
    }
}

/// Algorithm L (Li, 1994): after the reservoir fills, jump straight to the
/// next record that replaces one, so the work is O(k (1 + log(n / k))).
fn reservoir<I: BufRead, O: Write>(
    k: usize,
    rng: &mut Rng,
    input: &mut I,
    output: &mut O,
) -> Result<(), NdJsonSpatialError> {
    if k == 0 {
        return Ok(());
    }
    let mut kept: Vec<(u64, Vec<u8>)> = Vec::with_capacity(k.min(1024));
    let mut index: u64 = 0;
    let mut w = 1.0;
    let mut next = u64::MAX;
    for_each_record(input, |record| {
        if kept.len() < k {
            kept.push((index, record.to_vec()));
            if kept.len() == k {
                w = rng.weight(k);
                next = index.saturating_add(rng.gap(w)).saturating_add(1);
            }
        } else if index == next {
            let slot = &mut kept[rng.below(k)];
            slot.0 = index;
            slot.1.clear();
            slot.1.extend_from_slice(record);
            w *= rng.weight(k);
            next = index.saturating_add(rng.gap(w)).saturating_add(1);
        }
        index += 1;
        Ok(Flow::Continue)
    })?;

    kept.sort_unstable_by_key(|(i, _)| *i);
    for (_, record) in &kept {
        if !write_record(output, record)? {
            return Ok(());
        }
    }
    flush(output)
}

/// A seed that differs from run to run, for when none is given.
pub fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut rng = Rng::new(nanos ^ u64::from(std::process::id()).rotate_left(32));
    rng.next_u64()
}

/// SplitMix64: small, fast, and good enough for sampling.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in the open interval (0, 1).
    fn open01(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Uniform in `0..n`.
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// The factor Algorithm L multiplies its weight by: `U^(1/k)`.
    fn weight(&mut self, k: usize) -> f64 {
        (self.open01().ln() / k as f64).exp()
    }

    /// How many records to pass over before the next replacement.
    fn gap(&mut self, w: f64) -> u64 {
        // a float to int cast saturates, so a huge gap means "never"
        (self.open01().ln() / (-w).ln_1p()).floor() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(n: usize) -> String {
        (0..n).map(|i| format!("{{\"i\":{}}}\n", i)).collect()
    }

    fn run(sampling: Sampling, seed: u64, input: &str) -> Vec<usize> {
        let mut output = vec![];
        sample(sampling, seed, &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["i"].as_u64().unwrap() as usize
            })
            .collect()
    }

    #[test]
    fn test_reservoir_keeps_k_in_input_order() {
        let input = records(1000);
        for seed in 0..20 {
            let got = run(Sampling::Reservoir(10), seed, &input);
            assert_eq!(got.len(), 10);
            assert!(got.windows(2).all(|w| w[0] < w[1]), "{:?}", got);
        }
    }

    #[test]
    fn test_reservoir_short_input_keeps_everything() {
        assert_eq!(run(Sampling::Reservoir(10), 1, &records(3)), vec![0, 1, 2]);
        assert_eq!(
            run(Sampling::Reservoir(0), 1, &records(3)),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn test_same_seed_same_sample() {
        let input = records(500);
        assert_eq!(
            run(Sampling::Reservoir(7), 42, &input),
            run(Sampling::Reservoir(7), 42, &input)
        );
        assert_eq!(
            run(Sampling::Rate(0.1), 42, &input),
            run(Sampling::Rate(0.1), 42, &input)
        );
        assert_ne!(
            run(Sampling::Reservoir(7), 42, &input),
            run(Sampling::Reservoir(7), 43, &input)
        );
    }

    #[test]
    fn test_reservoir_is_uniform() {
        // each of 20 records should be picked ~ trials * k / n = 1000 times
        let input = records(20);
        let mut hits = [0u32; 20];
        for seed in 0..10_000 {
            for i in run(Sampling::Reservoir(2), seed, &input) {
                hits[i] += 1;
            }
        }
        for (i, h) in hits.iter().enumerate() {
            assert!((850..1150).contains(h), "record {} picked {} times", i, h);
        }
    }

    #[test]
    fn test_rate() {
        let input = records(10_000);
        assert_eq!(run(Sampling::Rate(0.0), 3, &input), Vec::<usize>::new());
        assert_eq!(run(Sampling::Rate(1.0), 3, &input).len(), 10_000);
        let got = run(Sampling::Rate(0.25), 3, &input);
        assert!((2300..2700).contains(&got.len()), "{}", got.len());
        assert!(got.windows(2).all(|w| w[0] < w[1]));
    }
}
