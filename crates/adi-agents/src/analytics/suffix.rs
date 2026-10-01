//! Exact repeated runs in a stream of token ids.
//!
//! Maximal repeats cannot extend left or right without losing an occurrence. Their candidates come
//! from a suffix array and LCP-interval walk; selection then counts only unclaimed, disjoint copies.
//! Algorithm references:
//!
//! - Suffix array by prefix doubling with a counting sort, and the LCP array —
//!   <https://cp-algorithms.com/string/suffix-array.html>. [`sort_cyclic_shifts`] is that article's
//!   construction transcribed into Rust, variable names and all.
//! - The LCP array in linear time — Kasai, Lee, Arimura, Arikawa, Park, *Linear-Time
//!   Longest-Common-Prefix Computation in Suffix Arrays and Its Applications*, CPM 2001,
//!   <https://doi.org/10.1007/3-540-48194-X_17>.
//! - Reading maximal repeats off LCP intervals with a stack — Abouelhoda, Kurtz, Ohlebusch,
//!   *Replacing suffix trees with enhanced suffix arrays*, J. Discrete Algorithms 2(1), 2004,
//!   <https://doi.org/10.1016/S1570-8667(03)00065-0>.
//!
//! Disjoint occurrence selection and the site cap are reporting rules, separate from those algorithms.

use std::collections::BinaryHeap;

/// Cap displayed locations without truncating the occurrence count.
const MAX_SITES: usize = 64;

/// A repeated run in token offsets; the caller maps it back to transcript locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawRepeat {
    /// Length of the repeated run, in tokens.
    pub len: usize,
    /// How many times it occurs, counting only non-overlapping occurrences.
    pub count: usize,
    /// Where it occurs (token offsets into the analyzed stream), ascending, at most [`MAX_SITES`].
    pub starts: Vec<usize>,
}

impl RawRepeat {
    /// Tokens in every counted occurrence after the first.
    pub(super) fn wasted(&self) -> usize {
        self.len * (self.count - 1)
    }
}

/// Up to `keep` maximal repeats of at least `min_len` tokens, greatest disjoint saving first.
///
/// The caller provides positive token ids and unique separators between distinct pieces of text.
/// Selected occurrences are disjoint within and across findings, so their savings can be summed.
pub(super) fn maximal_repeats(s: &[u32], min_len: usize, keep: usize) -> Vec<RawRepeat> {
    if min_len == 0 || min_len > s.len() / 2 || keep == 0 {
        return Vec::new();
    }
    let sa = suffix_array(s);
    let lcp = lcp_array(s, &sa);

    // Unique tokens (including segment separators) cannot belong to a repeat. Excluding them keeps
    // the copy-count bound tight when many individually separated blocks share the same content.
    let mut available = 0usize;
    let mut group = 0usize;
    for end in 1..=sa.len() {
        if end == sa.len() || s[sa[end] as usize] != s[sa[group] as usize] {
            if end - group > 1 {
                available += end - group;
            }
            group = end;
        }
    }

    // A prefix count of changes in preceding tokens makes left-maximality an O(1) interval query.
    // Scanning each interval instead is quadratic on a stream of one repeated token.
    let mut left_changes = vec![0usize; sa.len()];
    let preceding = |offset: u32| (offset as usize).checked_sub(1).map(|i| s[i]);
    for i in 1..sa.len() {
        left_changes[i] =
            left_changes[i - 1] + usize::from(preceding(sa[i - 1]) != preceding(sa[i]));
    }

    // LCP intervals are (saving upper bound, length, first suffix, last suffix). Bound the copy count
    // by available space as well as raw matches: overlapping matches can otherwise inflate it to n².
    let mut found: Vec<(usize, usize, usize, usize)> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for i in 0..=lcp.len() {
        let h = if i < lcp.len() { lcp[i] as usize } else { 0 };
        let mut lo = i;
        while let Some(&(top_h, top_lo)) = stack.last() {
            if top_h <= h {
                break;
            }
            stack.pop();
            // The interval spans sa[top_lo..=i]: that many suffixes share a prefix of `top_h` tokens.
            if top_h >= min_len && left_changes[i] != left_changes[top_lo] {
                let count = (i - top_lo + 1).min(available / top_h);
                if count >= 2 {
                    found.push((top_h * (count - 1), top_h, top_lo, i));
                }
            }
            lo = top_lo;
        }
        if h > 0 && stack.last().is_none_or(|&(top_h, _)| top_h < h) {
            stack.push((h, lo));
        }
    }

    // Materialize lazily. A candidate may only win when its actual saving beats every remaining
    // upper bound; limiting the old raw ranking before this step could discard the best finding.
    let mut pending = BinaryHeap::from(found);
    let mut out: Vec<RawRepeat> = Vec::new();
    let mut claimed: Vec<(usize, usize)> = Vec::new();
    while let Some((_, len, lo, hi)) = pending.pop() {
        if out.len() >= keep || available / 2 < min_len {
            break;
        }
        if len > available / 2 {
            continue;
        }
        let mut starts: Vec<usize> = sa[lo..=hi].iter().map(|&x| x as usize).collect();
        starts.sort_unstable();
        let starts = non_overlapping(&starts, len, &claimed);
        if starts.len() < 2 {
            continue;
        }
        let exact = (len * (starts.len() - 1), len, lo, hi);
        if pending.peek().is_some_and(|next| exact < *next) {
            pending.push(exact);
            continue;
        }
        available -= len * starts.len();
        claimed.extend(starts.iter().map(|&st| (st, st + len)));
        claimed.sort_unstable();
        out.push(RawRepeat {
            len,
            count: starts.len(),
            starts: starts.into_iter().take(MAX_SITES).collect(),
        });
    }
    out
}

/// Greedily take the earliest unclaimed copy; equal-length intervals make this a maximum-size set.
/// `starts` is sorted; `claimed` is sorted and disjoint.
fn non_overlapping(starts: &[usize], len: usize, claimed: &[(usize, usize)]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(starts.len());
    let mut end = 0usize;
    let mut claim = 0usize;
    for &st in starts {
        if st < end {
            continue;
        }
        while claim < claimed.len() && claimed[claim].1 <= st {
            claim += 1;
        }
        if claim < claimed.len() && claimed[claim].0 < st + len {
            continue;
        }
        out.push(st);
        end = st + len;
    }
    out
}

/// The suffix array of `s`: the start offsets of every suffix, in lexicographic order.
///
/// Prefix doubling with counting sort takes O(n log n). Cyclic shifts agree with suffixes after
/// appending a unique smallest terminator; the caller reserves `0` for this purpose.
fn suffix_array(s: &[u32]) -> Vec<u32> {
    let mut buf: Vec<u32> = Vec::with_capacity(s.len() + 1);
    buf.extend_from_slice(s);
    buf.push(0);
    // Compress sparse ids so counting-sort memory depends on input size, not the largest id.
    compress(&mut buf);
    let mut sa = sort_cyclic_shifts(&buf);
    // The first entry is the terminator's own suffix, which is not a suffix of `s`.
    sa.remove(0);
    sa
}

/// Replace values with order-preserving dense ranks.
fn compress(s: &mut [u32]) {
    let mut seen: Vec<u32> = s.to_vec();
    seen.sort_unstable();
    seen.dedup();
    for v in s.iter_mut() {
        *v = u32::try_from(seen.partition_point(|&x| x < *v)).unwrap_or(u32::MAX);
    }
}

/// Sort every cyclic shift of `s`, returning their start offsets in order.
///
/// Transcribed from <https://cp-algorithms.com/string/suffix-array.html>, whose names (`p` for the
/// permutation, `c` for equivalence classes, `pn`/`cn` for the next round) are kept so the two can be
/// read side by side. The `u32` narrowing and the dense-alphabet requirement are the only departures.
fn sort_cyclic_shifts(s: &[u32]) -> Vec<u32> {
    let n = s.len();
    let alphabet = s.iter().copied().max().unwrap_or(0) as usize + 1;
    let mut cnt = vec![0u32; alphabet.max(n)];
    let mut p = vec![0u32; n];
    let mut c = vec![0u32; n];

    for &x in s {
        cnt[x as usize] += 1;
    }
    for i in 1..alphabet {
        cnt[i] += cnt[i - 1];
    }
    for (i, &x) in s.iter().enumerate() {
        cnt[x as usize] -= 1;
        p[cnt[x as usize] as usize] = u32::try_from(i).unwrap_or(u32::MAX);
    }
    let mut classes = 1usize;
    for i in 1..n {
        if s[p[i] as usize] != s[p[i - 1] as usize] {
            classes += 1;
        }
        c[p[i] as usize] = u32::try_from(classes - 1).unwrap_or(u32::MAX);
    }

    let mut pn = vec![0u32; n];
    let mut cn = vec![0u32; n];
    let mut shift = 1usize;
    while shift < n {
        for i in 0..n {
            let moved = (p[i] as usize + n - shift) % n;
            pn[i] = u32::try_from(moved).unwrap_or(u32::MAX);
        }
        cnt[..classes].fill(0);
        for &x in &pn {
            cnt[c[x as usize] as usize] += 1;
        }
        for i in 1..classes {
            cnt[i] += cnt[i - 1];
        }
        for &x in pn.iter().rev() {
            let cls = c[x as usize] as usize;
            cnt[cls] -= 1;
            p[cnt[cls] as usize] = x;
        }
        cn[p[0] as usize] = 0;
        classes = 1;
        for i in 1..n {
            let cur = (c[p[i] as usize], c[(p[i] as usize + shift) % n]);
            let prev = (c[p[i - 1] as usize], c[(p[i - 1] as usize + shift) % n]);
            if cur != prev {
                classes += 1;
            }
            cn[p[i] as usize] = u32::try_from(classes - 1).unwrap_or(u32::MAX);
        }
        c.copy_from_slice(&cn);
        shift <<= 1;
    }
    p
}

/// The LCP array in Kasai's layout: `lcp[i]` is how many tokens `sa[i]` and `sa[i + 1]` share.
///
/// Dropping the first token shortens the match by at most one, so carrying `k` forward keeps this
/// linear (Kasai et al.; see module references).
fn lcp_array(s: &[u32], sa: &[u32]) -> Vec<u32> {
    let n = sa.len();
    if n == 0 {
        return Vec::new();
    }
    let mut rank = vec![0usize; n];
    for (i, &x) in sa.iter().enumerate() {
        rank[x as usize] = i;
    }
    let mut lcp = vec![0u32; n - 1];
    let mut k = 0usize;
    for i in 0..n {
        if rank[i] + 1 == n {
            k = 0;
            continue;
        }
        let j = sa[rank[i] + 1] as usize;
        while i + k < n && j + k < n && s[i + k] == s[j + k] {
            k += 1;
        }
        lcp[rank[i]] = u32::try_from(k).unwrap_or(u32::MAX);
        k = k.saturating_sub(1);
    }
    lcp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reserve zero for the terminator, as the production caller does.
    fn stream(text: &str) -> Vec<u32> {
        text.bytes().map(|b| u32::from(b) + 1).collect()
    }

    #[test]
    fn suffix_array_matches_a_plain_sort() {
        for text in ["banana", "mississippi", "aaaa", "abcabcabc", "a"] {
            let s = stream(text);
            let got = suffix_array(&s);
            let mut want: Vec<u32> = (0..u32::try_from(s.len()).unwrap()).collect();
            want.sort_by_key(|&i| s[i as usize..].to_vec());
            assert_eq!(got, want, "suffix array of {text:?}");
        }
    }

    #[test]
    fn lcp_matches_a_plain_comparison() {
        let s = stream("mississippi");
        let sa = suffix_array(&s);
        let lcp = lcp_array(&s, &sa);
        for i in 0..sa.len() - 1 {
            let a = &s[sa[i] as usize..];
            let b = &s[sa[i + 1] as usize..];
            let want = a.iter().zip(b).take_while(|(x, y)| x == y).count();
            assert_eq!(lcp[i] as usize, want, "lcp at {i}");
        }
    }

    #[test]
    fn finds_the_repeated_phrase_once_at_full_length() {
        let s = stream("the quick fox. XX. the quick fox. YY. the quick fox.");
        let repeats = maximal_repeats(&s, 8, 10);
        assert!(!repeats.is_empty(), "expected a repeat");
        let top = &repeats[0];
        assert_eq!(top.count, 3, "three occurrences");
        assert_eq!(
            top.len,
            "the quick fox.".len(),
            "the whole phrase, not a fragment"
        );
        assert_eq!(top.wasted(), top.len * 2);
    }

    #[test]
    fn overlapping_occurrences_do_not_count_as_waste() {
        let s = stream("aaaaaaaa");
        let repeats = maximal_repeats(&s, 3, 10);
        for r in &repeats {
            let mut sites = r.starts.clone();
            sites.sort_unstable();
            for w in sites.windows(2) {
                assert!(w[1] - w[0] >= r.len, "occurrences must not overlap");
            }
        }
    }

    #[test]
    fn a_stream_without_repeats_is_empty() {
        let s = stream("abcdefghijklmnop");
        assert!(maximal_repeats(&s, 4, 10).is_empty());
    }

    #[test]
    fn repeats_never_span_a_separator() {
        let mut s = stream("hello world hello");
        s.push(u32::MAX);
        s.extend(stream("hello world hello"));
        let repeats = maximal_repeats(&s, 5, 20);
        let sep = s.iter().position(|&x| x == u32::MAX).unwrap();
        for r in &repeats {
            for &st in &r.starts {
                assert!(
                    st + r.len <= sep || st > sep,
                    "repeat at {st} len {} crosses the separator at {sep}",
                    r.len
                );
            }
        }
    }
    #[test]
    fn oversized_minimum_returns_no_repeats() {
        assert!(maximal_repeats(&stream("abcabc"), usize::MAX, 10).is_empty());
    }

    #[test]
    fn keep_selects_the_largest_non_overlapping_saving() {
        let repeats = maximal_repeats(&stream("aaaaaaaaaaXbcdefghiYbcdefghi"), 2, 1);
        assert_eq!(repeats.len(), 1);
        assert_eq!(repeats[0].len, 8);
        assert_eq!(repeats[0].wasted(), 8);
    }

    #[test]
    fn different_findings_do_not_claim_the_same_tokens() {
        let repeats = maximal_repeats(&stream("abcdef#defghi$abcdefghi"), 4, 10);
        let mut claimed = Vec::new();
        for repeat in repeats {
            for start in repeat.starts {
                let end = start + repeat.len;
                assert!(claimed.iter().all(|&(a, b)| end <= a || start >= b));
                claimed.push((start, end));
            }
        }
    }

    #[test]
    fn independent_occurrences_survive_an_overlap_with_another_finding() {
        let s = stream("abcdefghijklmnopqrstuvwx#abcdefghijklmnopqrst$stuvwx%stuvwx");
        let repeats = maximal_repeats(&s, 6, 10);
        assert!(repeats.iter().any(|r| r.len == 20 && r.count == 2));
        assert!(repeats.iter().any(|r| r.len == 6 && r.count == 2));
    }

    #[test]
    fn site_cap_preserves_the_full_count() {
        let s: Vec<u32> = (0..100).flat_map(|i| [1, 2, i + 3]).collect();
        let repeats = maximal_repeats(&s, 2, 10);
        assert_eq!(repeats.len(), 1);
        assert_eq!(repeats[0].count, 100);
        assert_eq!(repeats[0].starts.len(), MAX_SITES);
        assert_eq!(repeats[0].wasted(), 198);
    }

    #[test]
    fn top_saving_matches_exhaustive_maximal_substrings() {
        for n in 2..=10 {
            for bits in 0..1usize << n {
                let s: Vec<u32> = (0..n).map(|i| 1 + ((bits >> i) & 1) as u32).collect();
                let mut best = 0;
                for len in 1..=n / 2 {
                    for start in 0..=n - len {
                        let pattern = &s[start..start + len];
                        let sites: Vec<usize> = s
                            .windows(len)
                            .enumerate()
                            .filter_map(|(i, window)| (window == pattern).then_some(i))
                            .collect();
                        let first = sites[0];
                        let left = first == 0 || sites.iter().any(|&i| s[i - 1] != s[first - 1]);
                        let right = sites.iter().any(|&i| i + len == n)
                            || sites.iter().any(|&i| s[i + len] != s[first + len]);
                        if !left || !right {
                            continue;
                        }
                        let mut count = 0;
                        let mut end = 0;
                        for i in sites {
                            if i >= end {
                                count += 1;
                                end = i + len;
                            }
                        }
                        best = best.max(len * (count - 1));
                    }
                }
                let repeats = maximal_repeats(&s, 1, 1);
                assert_eq!(repeats.first().map_or(0, RawRepeat::wasted), best, "{s:?}");
            }
        }
    }
}
