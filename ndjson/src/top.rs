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

//! `top`: the first *k* records by some keys, in a bounded heap. The same
//! output as `sort -k ... | head -n k`, in O(k) memory.

use crate::{
    head::flush,
    records::{for_each_record, write_record, Flow},
    sort::{record_key, SortKey, SortSummary},
};
use ndjson_common::error::NdJsonSpatialError;
use std::{
    collections::BinaryHeap,
    io::{BufRead, Write},
};

/// Writes the `k` records of `input` that sort first by `keys` (last, with
/// `descending`), in order. Ties keep input order, as `sort` is stable.
/// Returns key evaluation errors; a failed key sorts as `null`.
pub fn top<I: BufRead, O: Write>(
    k: usize,
    keys: &[SortKey],
    descending: bool,
    input: &mut I,
    output: &mut O,
) -> Result<SortSummary, NdJsonSpatialError> {
    let mut summary = SortSummary::default();
    if k == 0 {
        return Ok(summary);
    }
    // a max-heap of the best k so far, so the worst of them is on top
    let mut heap: BinaryHeap<(Vec<u8>, u64, Vec<u8>)> = BinaryHeap::with_capacity(k.min(1024));
    let mut n = 0;
    for_each_record(input, |line| {
        n += 1;
        let mut key = record_key(keys, line, n, &mut summary)?;
        if descending {
            // keys are prefix-free, so inverting the whole tuple reverses it
            key.iter_mut().for_each(|b| *b = !*b);
        }
        if heap.len() < k {
            heap.push((key, n, line.to_vec()));
        } else if let Some(mut worst) = heap.peek_mut() {
            // a later record never beats an equal key
            if key < worst.0 {
                worst.0 = key;
                worst.1 = n;
                worst.2.clear();
                worst.2.extend_from_slice(line);
            }
        }
        Ok(Flow::Continue)
    })?;

    for (_, _, line) in heap.into_sorted_vec() {
        if !write_record(output, &line)? {
            break;
        }
    }
    flush(output)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmath_ast::FunctionRegistry;

    fn run(k: usize, keys: &[&str], descending: bool, input: &str) -> String {
        let registry = FunctionRegistry::default();
        let keys: Vec<_> = keys
            .iter()
            .map(|k| SortKey::parse(&registry, k).unwrap())
            .collect();
        let mut output = vec![];
        top(k, &keys, descending, &mut input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    const INPUT: &str = "{\"n\":3,\"s\":\"c\"}\n{\"n\":-1,\"s\":\"a\"}\n{\"n\":10,\"s\":\"b\"}\n{\"n\":2.5,\"s\":\"d\"}\n";

    #[test]
    fn test_top_smallest() {
        assert_eq!(
            run(2, &["d.n"], false, INPUT),
            "{\"n\":-1,\"s\":\"a\"}\n{\"n\":2.5,\"s\":\"d\"}\n"
        );
    }

    #[test]
    fn test_top_desc_flag_and_suffix_agree() {
        let want = "{\"n\":10,\"s\":\"b\"}\n{\"n\":3,\"s\":\"c\"}\n";
        assert_eq!(run(2, &["d.n"], true, INPUT), want);
        assert_eq!(run(2, &["d.n:d"], false, INPUT), want);
    }

    #[test]
    fn test_top_expression_key() {
        assert_eq!(
            run(1, &["0 - d.n"], false, INPUT),
            "{\"n\":10,\"s\":\"b\"}\n"
        );
    }

    #[test]
    fn test_top_ties_keep_input_order() {
        let input = "{\"k\":1,\"i\":0}\n{\"k\":0,\"i\":1}\n{\"k\":1,\"i\":2}\n{\"k\":1,\"i\":3}\n";
        assert_eq!(
            run(3, &["d.k"], false, input),
            "{\"k\":0,\"i\":1}\n{\"k\":1,\"i\":0}\n{\"k\":1,\"i\":2}\n"
        );
        assert_eq!(
            run(2, &["d.k"], true, input),
            "{\"k\":1,\"i\":0}\n{\"k\":1,\"i\":2}\n"
        );
    }

    #[test]
    fn test_top_composite_key_and_short_input() {
        let input = "{\"a\":1,\"b\":2}\n{\"a\":1,\"b\":1}\n{\"a\":0,\"b\":9}\n";
        assert_eq!(
            run(10, &["d.a:d", "d.b"], false, input),
            "{\"a\":1,\"b\":1}\n{\"a\":1,\"b\":2}\n{\"a\":0,\"b\":9}\n"
        );
        assert_eq!(run(0, &["d.a"], false, input), "");
    }

    #[test]
    fn test_top_matches_sort_head() {
        let registry = FunctionRegistry::default();
        let input: String = (0..500)
            .map(|i| format!("{{\"v\":{}}}\n", (i * 7919) % 101))
            .collect();
        let keys = vec![SortKey::parse(&registry, "d.v:d").unwrap()];
        let mut sorted = vec![];
        crate::sort::sort(
            &mut input.as_bytes(),
            &mut sorted,
            &crate::sort::SortOptions::new(keys),
        )
        .unwrap();
        let want: String = String::from_utf8(sorted)
            .unwrap()
            .lines()
            .take(25)
            .map(|l| format!("{}\n", l))
            .collect();
        assert_eq!(run(25, &["d.v:d"], false, &input), want);
    }
}
