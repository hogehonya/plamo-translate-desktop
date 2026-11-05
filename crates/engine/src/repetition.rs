//! Detection of generation loops.
//!
//! A small model decoding greedily can fall into a cycle and emit the same few tokens until the
//! context is full. Which inputs trigger it depends on near-ties between candidate tokens, so it
//! varies with the backend, hardware, and thread count, and cannot be ruled out in advance.
//!
//! [`LoopDetector`] watches the generated tokens. For every period up to [`MAX_PERIOD`] it keeps
//! the length of the run in which each token equals the one a period earlier, which is updated
//! in constant time per period and token.

/// The longest cycle detected, in tokens. Longer cycles are caught by the output length limit.
pub const MAX_PERIOD: usize = 128;

/// A cycle must repeat at least this many times to count as a loop.
const MIN_REPEATS: usize = 3;

/// The shortest repeated span that counts as a loop, in tokens, for text without repetition of
/// its own.
pub const MIN_LOOP_SPAN: usize = 64;

/// A loop at the end of the generated tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loop {
    /// Length of the cycle in tokens.
    pub period: usize,
    /// Index of the first token of the repeated span.
    pub start: usize,
}

impl Loop {
    /// Number of tokens up to the end of the cycle's first occurrence.
    pub fn first_end(&self) -> usize {
        self.start + self.period
    }
}

#[derive(Debug, Clone)]
pub struct LoopDetector {
    /// The prompt, for [`LoopDetector::keep`].
    input: Vec<i32>,
    tokens: Vec<i32>,
    /// `runs[p - 1]`: how many tokens at the end equal the token `p` positions earlier.
    runs: Vec<usize>,
    min_span: usize,
}

impl LoopDetector {
    /// A detector that reports periodic spans of at least `min_span` tokens.
    pub fn new(min_span: usize) -> Self {
        Self {
            input: Vec::new(),
            tokens: Vec::new(),
            runs: vec![0; MAX_PERIOD],
            min_span,
        }
    }

    /// A detector calibrated to the prompt: a span counts as a loop only if it is well beyond
    /// any repetition the input itself contains, such as a ruler of dashes or a refrain.
    pub fn for_input(input: Vec<i32>) -> Self {
        let min_span = MIN_LOOP_SPAN.max(2 * longest_periodic_span(&input));
        Self {
            input,
            ..Self::new(min_span)
        }
    }

    /// Number of generated tokens worth keeping when the output ends in `found`.
    ///
    /// That is the cycle's first occurrence, unless the input contains the same cycle: text the
    /// model copies, such as repeated lines of code, legitimately repeats as often as in the
    /// input, and only the repeats beyond that are the loop.
    pub fn keep(&self, found: Loop) -> usize {
        let cycle = &self.tokens[found.start..found.first_end()];
        let repeats = max_consecutive_repeats(&self.input, cycle).max(1);
        (found.start + repeats * found.period).min(self.tokens.len())
    }

    /// Adds a generated token. Returns the loop if the output now ends in one.
    pub fn push(&mut self, token: i32) -> Option<Loop> {
        let n = self.tokens.len();
        self.tokens.push(token);
        let mut found = None;
        for period in 1..=MAX_PERIOD.min(n) {
            let run = &mut self.runs[period - 1];
            if self.tokens[n - period] == token {
                *run += 1;
            } else {
                *run = 0;
            }
            let span = *run + period;
            if found.is_none() && span >= self.min_span && span >= MIN_REPEATS * period {
                found = Some(Loop {
                    period,
                    start: n + 1 - span,
                });
            }
        }
        found
    }
}

/// The largest number of back-to-back occurrences of `cycle` in `tokens`.
fn max_consecutive_repeats(tokens: &[i32], cycle: &[i32]) -> usize {
    let period = cycle.len();
    let mut best = 0;
    for start in 0..tokens.len() {
        let mut repeats = 0;
        while tokens[start + repeats * period..].starts_with(cycle) {
            repeats += 1;
        }
        best = best.max(repeats);
    }
    best
}

/// The longest span of `tokens` that repeats a cycle of at most [`MAX_PERIOD`] tokens at least
/// three times, or 0.
pub fn longest_periodic_span(tokens: &[i32]) -> usize {
    let mut runs = vec![0usize; MAX_PERIOD];
    let mut longest = 0;
    for (n, &token) in tokens.iter().enumerate() {
        for period in 1..=MAX_PERIOD.min(n) {
            let run = &mut runs[period - 1];
            if tokens[n - period] == token {
                *run += 1;
            } else {
                *run = 0;
            }
            let span = *run + period;
            if span >= MIN_REPEATS * period {
                longest = longest.max(span);
            }
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(detector: &mut LoopDetector, tokens: impl IntoIterator<Item = i32>) -> Option<Loop> {
        tokens.into_iter().find_map(|t| detector.push(t))
    }

    #[test]
    fn detects_a_short_cycle() {
        let mut d = LoopDetector::new(64);
        // Ten distinct tokens, then a 12-token cycle.
        let prefix = 100..110;
        let cycle: Vec<i32> = (1..=12).collect();
        let found = feed(
            &mut d,
            prefix.chain(cycle.iter().copied().cycle().take(200)),
        );
        // Detected once the periodic span reaches 64 tokens.
        assert_eq!(
            found,
            Some(Loop {
                period: 12,
                start: 10
            })
        );
        assert_eq!(d.tokens.len(), 10 + 64);
        assert_eq!(found.unwrap().first_end(), 22);
    }

    #[test]
    fn detects_a_long_cycle_after_three_repeats() {
        let mut d = LoopDetector::new(64);
        let cycle: Vec<i32> = (1..=50).collect();
        let found = feed(&mut d, cycle.iter().copied().cycle().take(500));
        assert_eq!(
            found,
            Some(Loop {
                period: 50,
                start: 0
            })
        );
        assert_eq!(d.tokens.len(), 150);
    }

    #[test]
    fn ignores_text_without_cycles() {
        let mut d = LoopDetector::new(64);
        // A pseudo-random sequence over a small alphabet still has no long periodic span.
        let mut x = 12345u32;
        let tokens = (0..5000).map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((x >> 16) % 50) as i32
        });
        assert_eq!(feed(&mut d, tokens), None);
    }

    #[test]
    fn short_repetition_is_allowed() {
        let mut d = LoopDetector::new(64);
        // "* * * * *" and a ruler of 40 dashes are far below the threshold.
        let tokens = [1, 2, 1, 2, 1, 2, 1, 2, 1, 3]
            .into_iter()
            .chain(std::iter::repeat_n(7, 40))
            .chain(10..20);
        assert_eq!(feed(&mut d, tokens), None);
    }

    #[test]
    fn calibrates_to_repetition_in_the_input() {
        let input: Vec<i32> = (0..100).map(|i| i % 4).collect();
        assert_eq!(longest_periodic_span(&input), 100);
        let mut d = LoopDetector::for_input(input.clone());
        // The output may repeat as much as the input does, but not without bound.
        assert_eq!(feed(&mut d, (0..150).map(|i| i % 4)), None);
        assert!(feed(&mut d, (150..300).map(|i| i % 4)).is_some());
    }

    #[test]
    fn keeps_as_many_repeats_as_the_input_has() {
        // The input repeats a 5-token line 30 times; the output copies it and does not stop.
        let line = [10, 11, 12, 13, 14];
        let input: Vec<i32> = std::iter::once(1)
            .chain(line.iter().copied().cycle().take(5 * 30))
            .chain([2])
            .collect();
        let mut d = LoopDetector::for_input(input.clone());
        let output = std::iter::once(7).chain(line.iter().copied().cycle().take(5 * 100));
        let found = feed(&mut d, output).unwrap();
        assert_eq!(found.period, 5);
        assert_eq!(d.keep(found), 1 + 5 * 30);

        // A cycle the input does not contain keeps its first occurrence only.
        let mut d = LoopDetector::for_input(input.clone());
        let found = feed(&mut d, [3, 4, 5, 6].iter().copied().cycle().take(400)).unwrap();
        assert_eq!(d.keep(found), found.first_end());
    }

    #[test]
    fn plain_input_uses_the_default_threshold() {
        assert_eq!(longest_periodic_span(&[1, 2, 3, 4, 5]), 0);
        assert_eq!(
            LoopDetector::for_input(vec![1, 2, 3]).min_span,
            MIN_LOOP_SPAN
        );
    }
}
