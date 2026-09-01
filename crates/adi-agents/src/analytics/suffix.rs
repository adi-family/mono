//! Exact repeated runs in a stream of token ids.
//!
//! The question this answers is "what did we send more than once, and how much did saying it again
//! cost?" — so the unit is a *maximal repeat*: the longest run that occurs in at least two places and
//! cannot be extended left or right without losing one of them. Maximality is what keeps the report
//! readable. A 200-token block that appears three times also contains a 199-token block that appears
//! three times, and every suffix of it besides; without maximality the top of the list is one finding
//! written out two hundred times.
//!
//! The machinery is the textbook one — suffix array, LCP array, then the LCP-interval stack walk —
//! because the alternative (hash every window of every length) is the same answer computed worse.
//! Everything here is generic over `&[u32]`: it never learns that the ids came from a tokenizer, and
//! the caller never learns that a suffix array was involved.
//!
//! Where the textbook is, for anyone reading a line and wondering why it is written that way:
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
//! What is *not* from the textbook is everything after the walk — non-overlapping occurrences,
//! dropping a repeat that lies inside one already reported, the site cap. Those are about what makes
//! a readable finding rather than about what makes a correct repeat, and each says so where it sits.

/// The most occurrence offsets kept for one repeat. A repeat that shows up four thousand times is a
/// finding at ten occurrences and the same finding at four thousand; the count is reported in full,
/// only the list of *where* is bounded, and it is bounded because it is drawn.
const MAX_SITES: usize = 64;

/// One repeated run, as the suffix machinery sees it: a length, how often it occurs, and where. Token
/// offsets, not text — [`super`] owns the mapping back to what a human reads.
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
    /// What repeating it cost: every occurrence after the first is a run of tokens that carried no
    /// information the model had not already been given.
    pub(super) fn wasted(&self) -> usize {
        self.len * (self.count - 1)
    }
}

/// Every maximal repeat of at least `min_len` tokens, worst offender first.
///
/// `keep` bounds the answer, not the search: all repeats are found, and the `keep` most wasteful are
/// the ones described in full. The stream is expected to carry a unique separator between logically
/// distinct pieces (see [`super::SEPARATOR_BASE`]) — that is what stops a "repeat" from being an
/// artefact of two unrelated texts having been laid end to end.
pub(super) fn maximal_repeats(s: &[u32], min_len: usize, keep: usize) -> Vec<RawRepeat> {
    if s.len() < min_len * 2 || min_len == 0 || keep == 0 {
        return Vec::new();
    }
    let sa = suffix_array(s);
    let lcp = lcp_array(s, &sa);

    // Every LCP-interval is a candidate: the suffixes it spans share a prefix, and that shared prefix
    // is a repeat. The stack walk that enumerates the intervals in one pass is Abouelhoda–Kurtz–
    // Ohlebusch's (see the module docs). Collected as (wasted, len, lo, hi) rather than materialized,
    // because materializing every interval's occurrence list is quadratic and all but `keep` of them
    // are about to be thrown away.
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
            if top_h >= min_len && left_maximal(s, &sa[top_lo..=i]) {
                let count = i - top_lo + 1;
                found.push((top_h * (count - 1), top_h, top_lo, i));
            }
            lo = top_lo;
        }
        if h > 0 && stack.last().is_none_or(|&(top_h, _)| top_h < h) {
            stack.push((h, lo));
        }
    }

    // Worst first, then materialize only as far down as `keep` survivors take us: a repeat that lies
    // inside one already kept is the same waste counted twice (the long block and the path inside it),
    // and reporting both would have the rail say the conversation wasted more than it has.
    found.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<RawRepeat> = Vec::new();
    let mut claimed: Vec<(usize, usize)> = Vec::new();
    for (_, len, lo, hi) in found {
        if out.len() >= keep {
            break;
        }
        let mut starts: Vec<usize> = sa[lo..=hi].iter().map(|&x| x as usize).collect();
        starts.sort_unstable();
        let starts = non_overlapping(&starts, len);
        if starts.len() < 2 {
            continue;
        }
        if starts.iter().any(|&st| covered(&claimed, st, len)) {
            continue;
        }
        claimed.extend(starts.iter().map(|&st| (st, st + len)));
        out.push(RawRepeat {
            len,
            count: starts.len(),
            starts: starts.into_iter().take(MAX_SITES).collect(),
        });
    }
    // Materialization drops overlapping occurrences, so a repeat's cost is only final here.
    out.sort_unstable_by_key(|r| std::cmp::Reverse(r.wasted()));
    out
}

/// Whether the run shared by these suffixes is *left*-maximal — whether the token before it differs
/// somewhere. When every occurrence is preceded by the same token, this run is the tail of a longer
/// repeat that the walk will report on its own, and reporting this one too says the same thing twice.
fn left_maximal(s: &[u32], suffixes: &[u32]) -> bool {
    let mut prev: Option<u32> = None;
    for &start in suffixes {
        let start = start as usize;
        // A run at the very start of the stream has nothing before it to match, so nothing can extend
        // leftward through every occurrence.
        if start == 0 {
            return true;
        }
        let before = s[start - 1];
        match prev {
            None => prev = Some(before),
            Some(p) if p != before => return true,
            Some(_) => {}
        }
    }
    false
}

/// Thin `starts` down to occurrences that do not sit on top of each other.
///
/// A run of one repeated token matches itself at every offset; counted raw, "aaaa" would report three
/// copies of "aaa" and claim two of them were wasted. Only occurrences that occupy distinct tokens
/// were actually paid for twice.
fn non_overlapping(starts: &[usize], len: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(starts.len());
    let mut end = 0usize;
    for (i, &st) in starts.iter().enumerate() {
        if i == 0 || st >= end {
            out.push(st);
            end = st + len;
        }
    }
    out
}

/// Whether `[start, start + len)` lies mostly inside a range already spoken for by a repeat reported
/// earlier (which, given the ordering, wasted more). Mostly rather than wholly: a repeat that shares
/// all but a token or two with one already listed is not a second finding.
fn covered(claimed: &[(usize, usize)], start: usize, len: usize) -> bool {
    let end = start + len;
    let mut inside = 0usize;
    for &(cs, ce) in claimed {
        let lo = start.max(cs);
        let hi = end.min(ce);
        if lo < hi {
            inside += hi - lo;
        }
    }
    inside * 10 >= len * 7
}

/// The suffix array of `s`: the start offsets of every suffix, in lexicographic order.
///
/// Built by prefix doubling with a counting sort at each round — O(n log n), where the sort-based
/// spelling of the same algorithm is O(n log² n). At a few hundred thousand tokens that is the
/// difference between an endpoint that answers and one that is quietly given up on.
///
/// The algorithm sorts *cyclic shifts*, which agree with suffixes only when the stream ends in
/// something unique and smallest — so a `0` terminator is appended here, and the caller guarantees
/// (by construction, see [`super::TOKEN_BASE`]) that no real id is `0`.
fn suffix_array(s: &[u32]) -> Vec<u32> {
    let mut buf: Vec<u32> = Vec::with_capacity(s.len() + 1);
    buf.extend_from_slice(s);
    buf.push(0);
    // The counting sort inside allocates one slot per *value*, so it must be handed a dense alphabet:
    // ids that happen to be sparse — separators chosen near the top of the range, say — would
    // otherwise ask for a table the size of their largest element rather than of the data. Ranking is
    // order-preserving, so the terminator stays both smallest and unique.
    compress(&mut buf);
    let mut sa = sort_cyclic_shifts(&buf);
    // The first entry is the terminator's own suffix, which is not a suffix of `s`.
    sa.remove(0);
    sa
}

/// Replace every value with its rank among the distinct values present, so the alphabet is exactly as
/// large as the data is varied.
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
/// Linear because it walks the suffixes in *stream* order rather than sorted order: dropping the
/// first token of a suffix can shorten its overlap with its neighbour by at most one, so the match
/// length carries over between steps instead of being recomputed. That is the whole trick of
/// <https://doi.org/10.1007/3-540-48194-X_17>, and `k = k.saturating_sub(1)` at the bottom of the
/// loop is where it is spent.
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

    /// Turn a byte string into a token stream in the shape the real caller produces: ids offset off
    /// zero, so the terminator stays unique and smallest.
    fn stream(text: &str) -> Vec<u32> {
        text.bytes().map(|b| u32::from(b) + 1).collect()
    }

    /// The suffix array is the sorted suffixes — checked against the brute-force answer, which is the
    /// definition rather than a reimplementation.
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

    /// The whole point, end to end: a phrase said three times is found once, at its full length, with
    /// all three sites — not as a pile of its own suffixes.
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

    /// Overlapping matches are not two copies of anything: "aaaa" contains one "aaa" that was paid
    /// for, not two.
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

    /// A stream with nothing said twice reports nothing — the empty answer has to be reachable, or
    /// every clean conversation grows a finding.
    #[test]
    fn a_stream_without_repeats_is_empty() {
        let s = stream("abcdefghijklmnop");
        assert!(maximal_repeats(&s, 4, 10).is_empty());
    }

    /// A separator between two texts is unique, so no reported repeat may span one.
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
}
