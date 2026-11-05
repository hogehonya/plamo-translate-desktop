//! Splitting long texts into chunks, and translating them in order.
//!
//! Text is split at blank lines (paragraphs). Short neighbouring paragraphs are packed into one
//! chunk, and a paragraph that alone exceeds the budget is split again at the most natural
//! boundary that makes it fit (see [`Boundary`]). Separators between chunks are kept verbatim so
//! the translation preserves the layout.
//!
//! [`translate_document`] translates the chunks in order and passes the most recent
//! source/translation pairs as history, so terminology and style stay consistent.
//!
//! Chunks are kept far below what the context window allows. PLaMo 3 attends to only the last
//! 2048 tokens in most layers, and small models decoding greedily tend to lose their place and
//! loop when one output runs much longer than that. When an output still runs away (see
//! [`crate::repetition`]), it is withdrawn ([`Output::Truncate`]) and the chunk is translated
//! again in pieces of a few sentences; a piece that runs away again is retried sentence by
//! sentence, and a sentence with repetition penalized, cut if even that loops. Output that ran
//! away never enters the history, so one loop cannot seed the next.

use std::time::Duration;

use pfml::{HistoryPair, Prompt, TranslationOptions};

use crate::{Generation, Result, StopReason, TranslateStats, TranslationBackend};

/// How a long text is split, and how much history accompanies each chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentOptions {
    /// Upper bound for a chunk, in tokens. Chunks are also limited by the context window.
    pub max_chunk_tokens: usize,
    /// Upper bound for the history passed with each chunk, in tokens.
    pub max_history_tokens: usize,
    /// Text appended where output was cut ([`Progress::Cut`]), so the gap shows in the
    /// translation. Empty for none.
    pub cut_marker: String,
}

impl Default for DocumentOptions {
    fn default() -> Self {
        Self {
            max_chunk_tokens: 1024,
            max_history_tokens: 4096,
            cut_marker: String::new(),
        }
    }
}

/// Output of [`translate_document`], to be applied in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output<'a> {
    /// Text to append.
    Text(&'a str),
    /// Keep only the first `len` bytes of the output so far: output that ran away is being
    /// translated again.
    Truncate(usize),
}

/// Progress reports of [`translate_document`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Chunk `index` of `count` is starting. `source` is its byte range in the text.
    Chunk {
        index: usize,
        count: usize,
        source: (usize, usize),
    },
    /// The output for chunk `index`, or for a piece of it being retried, ran away (`reason`) and
    /// is translated again in `pieces` smaller pieces.
    Retry {
        index: usize,
        reason: StopReason,
        pieces: usize,
    },
    /// The output for a piece of chunk `index` that could not be split further (usually one
    /// sentence) ran away even with repetition penalized, and was cut. `source` is the piece's
    /// byte range in the text, and `output` the range of what was kept of its translation in the
    /// output, including the cut marker.
    Cut {
        index: usize,
        reason: StopReason,
        source: (usize, usize),
        output: (usize, usize),
    },
}

/// Size of the pieces a chunk is translated in again after its output ran away. Small enough
/// that loops are rare, large enough to keep some context and need only a few prompts: on the
/// CPU, retrying a chunk sentence by sentence took minutes, mostly processing the history again
/// for each sentence.
const RETRY_PIECE_TOKENS: usize = 256;

/// Output length limit for a source of `source_tokens` tokens. Translations are rarely more than
/// twice as long as the source in tokens, so this only stops output that ran away.
fn max_output_tokens(source_tokens: usize) -> usize {
    3 * source_tokens + 128
}

/// Whether a finished translation is so much shorter than its source that the model must have
/// ended early and skipped the rest. Large chunks did this silently; translations are rarely
/// under half the source's length in tokens.
fn looks_incomplete(source_tokens: usize, output_tokens: usize) -> bool {
    source_tokens >= 200 && output_tokens * 4 < source_tokens
}

/// A piece of the source: text to translate, followed by whitespace to copy unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk<'a> {
    pub text: &'a str,
    pub separator: &'a str,
}

/// Leading whitespace of a document, then its chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split<'a> {
    pub leading: &'a str,
    pub chunks: Vec<Chunk<'a>>,
}

/// Splits `text` so each chunk measures at most `max_tokens` with `count_tokens`.
pub fn split<'a>(
    text: &'a str,
    max_tokens: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Split<'a> {
    let body_start = text.len() - text.trim_start().len();
    let leading = &text[..body_start];

    // Paragraphs as (start, end, separator_end) byte ranges, split further where too large.
    let mut pieces = Vec::new();
    for range in paragraphs(text, body_start) {
        refine(
            text,
            range,
            Boundary::LineAfterSentence,
            max_tokens,
            count_tokens,
            &mut pieces,
        );
    }

    // Pack neighbouring pieces while they fit. Token counts are nearly additive, so the pieces
    // are counted once and the packed text is measured only near the limit.
    let counts: Vec<usize> = pieces
        .iter()
        .map(|&(start, end, _)| count_tokens(&text[start..end]))
        .collect();
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let (start, mut end, mut sep_end) = pieces[i];
        let mut estimate = counts[i];
        let mut j = i + 1;
        while j < pieces.len() {
            let (next_start, next_end, next_sep_end) = pieces[j];
            let next_estimate = estimate + count_tokens(&text[end..next_start]) + counts[j];
            if next_estimate > max_tokens - max_tokens / 16 {
                let exact = count_tokens(&text[start..next_end]);
                if exact > max_tokens {
                    break;
                }
                estimate = exact;
            } else {
                estimate = next_estimate;
            }
            end = next_end;
            sep_end = next_sep_end;
            j += 1;
        }
        chunks.push(Chunk {
            text: &text[start..end],
            separator: &text[end..sep_end],
        });
        i = j;
    }
    Split { leading, chunks }
}

/// Where a piece too large for a chunk is split, from the most to the least natural.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boundary {
    /// A line break after the end of a sentence: a paragraph break in text that separates
    /// paragraphs with a single newline, as Japanese prose usually does.
    LineAfterSentence,
    /// The end of a sentence. Other line breaks are ordinary whitespace, as in hard-wrapped prose.
    Sentence,
    /// Any line break.
    Line,
    /// After a comma or other clause punctuation, or at whitespace.
    Clause,
    /// Anywhere: the last resort, for text such as minified data without any of the above.
    Characters,
}

/// Appends `range` to `out` if it fits in `max_tokens`, or else its parts split at `boundary`,
/// each refined further at the next boundary.
fn refine(
    text: &str,
    range: (usize, usize, usize),
    boundary: Boundary,
    max_tokens: usize,
    count_tokens: &dyn Fn(&str) -> usize,
    out: &mut Vec<(usize, usize, usize)>,
) {
    let (start, end, sep_end) = range;
    if count_tokens(&text[start..end]) <= max_tokens {
        out.push(range);
        return;
    }
    let (parts, next) = match boundary {
        Boundary::LineAfterSentence => (
            lines_after_sentences(text, start, end, sep_end),
            Some(Boundary::Sentence),
        ),
        Boundary::Sentence => (
            sentences(text, start, end, sep_end, false),
            Some(Boundary::Line),
        ),
        Boundary::Line => (
            sentences(text, start, end, sep_end, true),
            Some(Boundary::Clause),
        ),
        Boundary::Clause => (
            clauses(text, start, end, sep_end),
            Some(Boundary::Characters),
        ),
        Boundary::Characters => (
            by_characters(text, start, end, sep_end, max_tokens, count_tokens),
            None,
        ),
    };
    for part in parts {
        match next {
            Some(next) => refine(text, part, next, max_tokens, count_tokens, out),
            None => out.push(part),
        }
    }
}

/// Sentence-ending punctuation that ends a sentence wherever it occurs.
fn is_terminal(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '．')
}

/// Sentence-ending punctuation that ends a sentence only before whitespace (`1.5` does not).
fn is_latin_terminal(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// Closing quotes and brackets, which belong to the sentence they close.
fn is_closing(c: char) -> bool {
    matches!(c, '」' | '』' | '）' | ')' | '"' | '\'' | '”' | '’')
}

/// Whether a line ending with `line` (trailing whitespace removed) ends a sentence: terminal
/// punctuation, possibly inside closing quotes or brackets, or a Japanese closing quote.
fn ends_sentence(line: &str) -> bool {
    let core = line.trim_end_matches(is_closing);
    let japanese_quote = line[core.len()..].contains(['」', '』']);
    japanese_quote
        || core
            .chars()
            .last()
            .is_some_and(|c| is_terminal(c) || is_latin_terminal(c))
}

/// Ranges within `text[start..end]` split after clause punctuation and at whitespace, the last
/// one taking `sep_end`.
fn clauses(text: &str, start: usize, end: usize, sep_end: usize) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut piece_start = start;
    let mut chars = text[start..end].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        let at = start + offset;
        let (piece_end, mut separator_end) = if c.is_whitespace() {
            (at, at + c.len_utf8())
        } else if matches!(c, '、' | '，' | ',' | ';' | '；' | ':' | '：') {
            (at + c.len_utf8(), at + c.len_utf8())
        } else {
            continue;
        };
        while let Some(&(next, c)) = chars.peek()
            && c.is_whitespace()
        {
            separator_end = start + next + c.len_utf8();
            chars.next();
        }
        if piece_end > piece_start && separator_end < end {
            out.push((piece_start, piece_end, separator_end));
            piece_start = separator_end;
        }
    }
    out.push((piece_start, end, sep_end));
    out
}

/// `text[start..end]` cut into pieces of equal character counts that fit in `max_tokens`, the
/// last one taking `sep_end`.
fn by_characters(
    text: &str,
    start: usize,
    end: usize,
    sep_end: usize,
    max_tokens: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Vec<(usize, usize, usize)> {
    let slice = &text[start..end];
    let chars = slice.chars().count();
    let tokens = count_tokens(slice).max(1);
    // A little under the average, as tokens are not spread evenly.
    let per_piece = (chars * max_tokens * 7 / 8 / tokens).max(1);
    let mut out = Vec::new();
    let mut piece_start = start;
    for (n, (offset, _)) in slice.char_indices().enumerate() {
        if n > 0 && n % per_piece == 0 {
            out.push((piece_start, start + offset, start + offset));
            piece_start = start + offset;
        }
    }
    out.push((piece_start, end, sep_end));
    out
}

/// Ranges within `text[start..end]` split at line breaks that follow the end of a sentence, the
/// last one taking `sep_end`.
fn lines_after_sentences(
    text: &str,
    start: usize,
    end: usize,
    sep_end: usize,
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut piece_start = start;
    let mut i = start;
    while let Some(offset) = text[i..end].find('\n') {
        let newline = i + offset;
        let line = text[piece_start..newline].trim_end();
        let rest = &text[newline..end];
        let next_start = newline + (rest.len() - rest.trim_start().len());
        if ends_sentence(line) && next_start < end && !line.is_empty() {
            out.push((piece_start, piece_start + line.len(), next_start));
            piece_start = next_start;
        }
        i = next_start.max(newline + 1);
    }
    out.push((piece_start, end, sep_end));
    out
}

/// Paragraph ranges in `text[from..]`: (start, end of text, end of following whitespace).
fn paragraphs(text: &str, from: usize) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut start = from;
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // A blank line: a newline followed by optional spaces and another newline.
            let mut j = i + 1;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\r') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'\n' {
                let end = trim_end(text, start, i);
                let mut sep_end = j;
                while sep_end < bytes.len() && bytes[sep_end].is_ascii_whitespace() {
                    sep_end += 1;
                }
                if end > start {
                    out.push((start, end, sep_end));
                }
                start = sep_end;
                i = sep_end;
                continue;
            }
        }
        i += 1;
    }
    if start < text.len() {
        let end = trim_end(text, start, text.len());
        if end > start {
            out.push((start, end, text.len()));
        }
    }
    out
}

/// Splits `text` into its sentences without packing them, to translate a chunk piece by piece.
pub fn sentences_of(text: &str) -> Split<'_> {
    let body_start = text.len() - text.trim_start().len();
    let mut chunks = Vec::new();
    for (start, end, sep_end) in paragraphs(text, body_start) {
        for (s, e, se) in sentences(text, start, end, sep_end, false) {
            chunks.push(Chunk {
                text: &text[s..e],
                separator: &text[e..se],
            });
        }
    }
    Split {
        leading: &text[..body_start],
        chunks,
    }
}

/// Sentence ranges within `text[start..end]`, the last one taking `sep_end`.
///
/// Sentences end at terminal punctuation. With `at_newlines`, every line break ends one as well;
/// otherwise a line break is ordinary whitespace, as in hard-wrapped prose.
fn sentences(
    text: &str,
    start: usize,
    end: usize,
    sep_end: usize,
    at_newlines: bool,
) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut sentence_start = start;
    let mut chars = text[start..end].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        let is_terminal = is_terminal(c);
        let is_latin_terminal =
            is_latin_terminal(c) && chars.peek().is_none_or(|&(_, next)| next.is_whitespace());
        let is_newline = at_newlines && c == '\n';
        if !(is_terminal || is_latin_terminal || is_newline) {
            continue;
        }
        let sentence_end = start + offset + c.len_utf8();
        // Absorb closing quotes and brackets right after the terminator.
        let mut after = sentence_end;
        while let Some(&(o, next)) = chars.peek() {
            if is_closing(next) {
                chars.next();
                after = start + o + next.len_utf8();
            } else {
                break;
            }
        }
        let text_end = trim_end(text, sentence_start, after);
        let mut ws_end = after;
        while let Some(&(o, next)) = chars.peek() {
            if next.is_whitespace() {
                chars.next();
                ws_end = start + o + next.len_utf8();
            } else {
                break;
            }
        }
        if ws_end >= end {
            break;
        }
        if text_end > sentence_start {
            out.push((sentence_start, text_end, ws_end));
        }
        sentence_start = ws_end;
    }
    if sentence_start < end {
        out.push((sentence_start, end, sep_end));
    } else if let Some(last) = out.last_mut() {
        last.2 = sep_end;
    }
    out
}

fn trim_end(text: &str, start: usize, end: usize) -> usize {
    start + text[start..end].trim_end().len()
}

/// Translates `text` chunk by chunk, streaming the output (including separators) to `sink`.
///
/// The context window `n_ctx` is shared between fixed parts (context, glossary), history,
/// the chunk, and the output. Of what remains after the fixed parts, a chunk gets at most a sixth
/// and history a third, which leaves the output room for about three times the chunk;
/// `document` caps both further.
#[allow(clippy::too_many_arguments)]
pub fn translate_document(
    backend: &mut dyn TranslationBackend,
    count_tokens: &dyn Fn(&str) -> usize,
    n_ctx: usize,
    document: &DocumentOptions,
    options: &TranslationOptions,
    text: &str,
    sink: &mut dyn FnMut(Output<'_>),
    on_progress: &mut dyn FnMut(Progress),
    is_cancelled: &dyn Fn() -> bool,
) -> Result<TranslateStats> {
    let fixed = count_tokens(&options.context)
        + options
            .glossary
            .iter()
            .map(|e| count_tokens(&e.source) + count_tokens(&e.target) + 2)
            .sum::<usize>()
        + 64;
    let available = n_ctx.saturating_sub(fixed).max(384);
    let chunk_budget = (available / 6).min(document.max_chunk_tokens);
    let history_budget = (available / 3).min(document.max_history_tokens);

    let split = split(text, chunk_budget, count_tokens);
    let mut translator = Translator {
        text,
        cut_marker: &document.cut_marker,
        backend,
        count_tokens,
        options,
        sink,
        on_progress,
        is_cancelled,
        history: Vec::new(),
        history_costs: Vec::new(),
        history_budget,
        emitted: 0,
        total: TranslateStats {
            prompt_tokens: 0,
            generated_tokens: 0,
            prompt_duration: Duration::ZERO,
            generation_duration: Duration::ZERO,
            stop_reason: StopReason::Finished,
            retries: 0,
            cut: 0,
        },
    };
    if split.chunks.is_empty() {
        return Ok(translator.total);
    }
    translator.emit(split.leading);

    let count = split.chunks.len();
    for (index, chunk) in split.chunks.iter().enumerate() {
        // Chunks are slices of `text`.
        let start = chunk.text.as_ptr() as usize - text.as_ptr() as usize;
        (translator.on_progress)(Progress::Chunk {
            index,
            count,
            source: (start, start + chunk.text.len()),
        });
        let reason = translator.chunk(index, chunk.text)?;
        translator.total.stop_reason = reason;
        if reason != StopReason::Finished {
            break;
        }
        translator.emit(chunk.separator);
    }
    Ok(translator.total)
}

struct Translator<'a> {
    /// The whole text; every piece translated is a slice of it.
    text: &'a str,
    cut_marker: &'a str,
    backend: &'a mut dyn TranslationBackend,
    count_tokens: &'a dyn Fn(&str) -> usize,
    options: &'a TranslationOptions,
    sink: &'a mut dyn FnMut(Output<'_>),
    on_progress: &'a mut dyn FnMut(Progress),
    is_cancelled: &'a dyn Fn() -> bool,
    history: Vec<HistoryPair>,
    /// Tokens each history pair takes in a prompt, counted once.
    history_costs: Vec<usize>,
    history_budget: usize,
    /// Bytes of output so far.
    emitted: usize,
    total: TranslateStats,
}

impl Translator<'_> {
    fn emit(&mut self, text: &str) {
        if !text.is_empty() {
            self.emitted += text.len();
            (self.sink)(Output::Text(text));
        }
    }

    fn truncate(&mut self, len: usize) {
        if len < self.emitted {
            self.emitted = len;
            (self.sink)(Output::Truncate(len));
        }
    }

    /// Translates `source` once with the recent history, streaming the output. Returns the
    /// translation, why it stopped, and the number of tokens generated.
    fn generate(
        &mut self,
        source: &str,
        generation: Generation,
    ) -> Result<(String, StopReason, usize)> {
        let recent = recent_history(&self.history, &self.history_costs, self.history_budget);
        let prompt = Prompt::new(self.options, source).with_history(recent);
        let mut translation = String::new();
        let emitted = &mut self.emitted;
        let sink = &mut self.sink;
        let stats = self.backend.translate(
            &prompt,
            &generation,
            &mut |piece| {
                if !piece.is_empty() {
                    translation.push_str(piece);
                    *emitted += piece.len();
                    sink(Output::Text(piece));
                }
            },
            self.is_cancelled,
        )?;
        self.total.prompt_tokens += stats.prompt_tokens;
        self.total.generated_tokens += stats.generated_tokens;
        self.total.prompt_duration += stats.prompt_duration;
        self.total.generation_duration += stats.generation_duration;
        Ok((translation, stats.stop_reason, stats.generated_tokens))
    }

    fn remember(&mut self, source: &str, translation: &str) {
        let translation = translation.trim();
        if !translation.is_empty() {
            let cost = (self.count_tokens)(source) + (self.count_tokens)(translation) + 16;
            self.history.push(HistoryPair {
                source: source.to_owned(),
                translation: translation.to_owned(),
            });
            self.history_costs.push(cost);
        }
    }

    /// Translates chunk `index`, retrying it in smaller pieces if its output runs away.
    /// Returns `Finished`, `Cancelled`, or `ContextFull`.
    fn chunk(&mut self, index: usize, text: &str) -> Result<StopReason> {
        self.piece(index, text, Level::Chunk)
    }

    /// Translates `text`. If the output runs away, it is withdrawn and `text` is translated again
    /// in smaller pieces: a chunk in pieces of up to [`RETRY_PIECE_TOKENS`], such a piece sentence
    /// by sentence, and a single sentence with repetition penalized, cut if even that loops.
    fn piece(&mut self, index: usize, text: &str, level: Level) -> Result<StopReason> {
        let start = self.emitted;
        let source_tokens = (self.count_tokens)(text);
        let max_tokens = max_output_tokens(source_tokens);
        let (translation, mut reason, generated) =
            self.generate(text, Generation::greedy(max_tokens))?;
        if reason == StopReason::Finished && looks_incomplete(source_tokens, generated) {
            reason = StopReason::Incomplete;
        }
        if !reason.is_runaway() {
            if reason == StopReason::Finished {
                self.remember(text, &translation);
            }
            return Ok(reason);
        }

        self.truncate(start);
        self.total.retries += 1;
        // The next level that actually splits `text`: translating the same input again with the
        // same history would only repeat the same output.
        let mut next = level;
        let pieces = loop {
            next = match next.smaller() {
                Some(next) => next,
                None => return self.penalized(index, text, max_tokens),
            };
            let pieces = match next {
                // `smaller` never returns `Chunk`.
                Level::Chunk | Level::Piece => split(text, RETRY_PIECE_TOKENS, self.count_tokens),
                Level::Sentence => sentences_of(text),
            };
            if pieces.chunks.len() > 1 {
                break pieces;
            }
        };
        (self.on_progress)(Progress::Retry {
            index,
            reason,
            pieces: pieces.chunks.len(),
        });
        self.emit(pieces.leading);
        for piece in &pieces.chunks {
            let reason = self.piece(index, piece.text, next)?;
            if reason != StopReason::Finished {
                return Ok(reason);
            }
            self.emit(piece.separator);
        }
        Ok(StopReason::Finished)
    }

    /// Translates `text`, which cannot be split further, with repetition penalized. Output that
    /// still runs away is cut.
    fn penalized(&mut self, index: usize, text: &str, max_tokens: usize) -> Result<StopReason> {
        let start = self.emitted;
        let generation = Generation {
            max_tokens,
            penalize_repetition: true,
        };
        let (translation, reason, _) = self.generate(text, generation)?;
        match reason {
            StopReason::Finished => {
                self.remember(text, &translation);
                return Ok(reason);
            }
            StopReason::Cancelled | StopReason::ContextFull => return Ok(reason),
            // Keep the text worth keeping (see `StopReason::Repetition`).
            StopReason::Repetition { kept } => self.truncate(start + kept),
            // Keep the output, which ends wherever the limit cut it. (Backends never report
            // Incomplete.)
            StopReason::LengthLimit | StopReason::Incomplete => {}
        }
        self.emit(self.cut_marker);
        self.total.cut += 1;
        let source_start = text.as_ptr() as usize - self.text.as_ptr() as usize;
        (self.on_progress)(Progress::Cut {
            index,
            reason,
            source: (source_start, source_start + text.len()),
            output: (start, self.emitted),
        });
        Ok(StopReason::Finished)
    }
}

/// How finely a retried text is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Chunk,
    /// Pieces of up to [`RETRY_PIECE_TOKENS`] tokens, at the same boundaries as chunks.
    Piece,
    Sentence,
}

impl Level {
    fn smaller(self) -> Option<Self> {
        match self {
            Self::Chunk => Some(Self::Piece),
            Self::Piece => Some(Self::Sentence),
            Self::Sentence => None,
        }
    }
}

/// The longest suffix of `history` whose `costs` (tokens per pair) fit in `budget`.
fn recent_history<'h>(
    history: &'h [HistoryPair],
    costs: &[usize],
    budget: usize,
) -> &'h [HistoryPair] {
    let mut used = 0;
    let mut start = history.len();
    for (i, cost) in costs.iter().enumerate().rev() {
        used += cost;
        if used > budget {
            break;
        }
        start = i;
    }
    &history[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> usize {
        s.chars().count()
    }

    fn rejoin(split: &Split<'_>) -> String {
        let mut out = split.leading.to_owned();
        for c in &split.chunks {
            out.push_str(c.text);
            out.push_str(c.separator);
        }
        out
    }

    #[test]
    fn short_text_is_one_chunk() {
        let s = split("Hello.\n\nWorld.\n", 100, &chars);
        assert_eq!(
            s.chunks,
            vec![Chunk {
                text: "Hello.\n\nWorld.",
                separator: "\n"
            }]
        );
    }

    #[test]
    fn splits_paragraphs_and_keeps_layout() {
        let text = "  First one here.\n\n\nSecond one.\n  \nThird.";
        let s = split(text, 15, &chars);
        assert_eq!(s.leading, "  ");
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(texts, ["First one here.", "Second one.", "Third."]);
        assert_eq!(rejoin(&s), text);
    }

    #[test]
    fn packs_small_paragraphs() {
        let s = split("a.\n\nb.\n\nc.\n\nd.", 7, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(texts, ["a.\n\nb.", "c.\n\nd."]);
    }

    #[test]
    fn splits_long_paragraphs_at_sentences() {
        let text = "これは一文目です。これは二文目です。「三文目！」最後の文。";
        let s = split(text, 10, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            [
                "これは一文目です。",
                "これは二文目です。",
                "「三文目！」",
                "最後の文。"
            ]
        );
        assert_eq!(rejoin(&s), text);

        let text = "One sentence. Two sentences! Version 1.5 is out? Yes.";
        let s = split(text, 20, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            [
                "One sentence.",
                "Two sentences!",
                "Version 1.5 is out?",
                "Yes."
            ]
        );
        assert_eq!(rejoin(&s), text);
    }

    /// Stats for a fake generation of `output`, one token per character.
    fn stats(stop_reason: StopReason, output: &str) -> TranslateStats {
        TranslateStats {
            prompt_tokens: 1,
            generated_tokens: output.chars().count(),
            prompt_duration: Duration::ZERO,
            generation_duration: Duration::ZERO,
            stop_reason,
            retries: 0,
            cut: 0,
        }
    }

    /// Uppercases its input and records the prompts it received. Inputs containing "LOOP" loop
    /// when decoded greedily, and also with repetition penalized if `stubborn`. A multi-sentence
    /// input containing "SHORT" is answered with just "A".
    struct Upper {
        prompts: Vec<String>,
        stubborn: bool,
    }

    impl Upper {
        fn new() -> Self {
            Self {
                prompts: vec![],
                stubborn: false,
            }
        }
    }

    impl TranslationBackend for Upper {
        fn translate(
            &mut self,
            prompt: &Prompt<'_>,
            generation: &Generation,
            sink: &mut dyn FnMut(&str),
            _: &dyn Fn() -> bool,
        ) -> Result<TranslateStats> {
            self.prompts.push(prompt.render()?);
            if prompt.text.contains("LOOP") && (self.stubborn || !generation.penalize_repetition) {
                sink("la la la la");
                return Ok(stats(StopReason::Repetition { kept: 3 }, "la la la la"));
            }
            if prompt.text.contains("SHORT") && prompt.text.contains('b') {
                sink("A");
                return Ok(stats(StopReason::Finished, "A"));
            }
            let output = prompt.text.to_uppercase();
            sink(&output);
            Ok(stats(StopReason::Finished, &output))
        }
    }

    /// Runs [`translate_document`] and applies its output, returning the text, the stats, and
    /// the progress reports.
    fn run(
        backend: &mut Upper,
        count_tokens: &dyn Fn(&str) -> usize,
        document: DocumentOptions,
        text: &str,
    ) -> (String, TranslateStats, Vec<Progress>) {
        let options = TranslationOptions::new(pfml::LangCode::new("en").unwrap());
        let mut out = String::new();
        let mut progress = vec![];
        let stats = translate_document(
            backend,
            count_tokens,
            100_000,
            &document,
            &options,
            text,
            &mut |o| match o {
                Output::Text(s) => out.push_str(s),
                Output::Truncate(len) => out.truncate(len),
            },
            &mut |p| progress.push(p),
            &|| false,
        )
        .unwrap();
        (out, stats, progress)
    }

    #[test]
    fn translates_chunks_with_history() {
        let mut backend = Upper::new();
        // 3 words = 180 tokens: with chunks of at most 200 tokens the two paragraphs are
        // separate, and 500 tokens of history hold the first pair.
        let document = DocumentOptions {
            max_chunk_tokens: 200,
            max_history_tokens: 500,
            ..DocumentOptions::default()
        };
        let text = "\nalpha beta gamma.\n\ndelta epsilon zeta.\n";
        let words = |s: &str| s.split_whitespace().count() * 60;
        let (out, stats, progress) = run(&mut backend, &words, document, text);
        assert_eq!(out, "\nALPHA BETA GAMMA.\n\nDELTA EPSILON ZETA.\n");
        assert_eq!(
            progress,
            [
                Progress::Chunk {
                    index: 0,
                    count: 2,
                    source: (1, 18)
                },
                Progress::Chunk {
                    index: 1,
                    count: 2,
                    source: (20, 39)
                }
            ]
        );
        assert_eq!(stats.prompt_tokens, 2);
        assert_eq!((stats.retries, stats.cut), (0, 0));
        // The second prompt carries the first pair as history.
        assert!(backend.prompts[1].contains("alpha beta gamma.<|plamo:tag|>output"));
        assert!(backend.prompts[1].contains("<|plamo:msg|>ALPHA BETA GAMMA."));
    }

    #[test]
    fn retries_a_runaway_chunk_sentence_by_sentence() {
        let mut backend = Upper::new();
        let document = DocumentOptions {
            max_chunk_tokens: 25,
            max_history_tokens: 1000,
            ..DocumentOptions::default()
        };
        let text = "Good one. Bad LOOP one.\n\nNext.";
        let (out, stats, progress) = run(&mut backend, &chars, document, text);
        assert_eq!(out, "GOOD ONE. BAD LOOP ONE.\n\nNEXT.");
        // The chunk, then its second sentence.
        assert_eq!((stats.retries, stats.cut), (2, 0));
        assert_eq!(
            progress[1],
            Progress::Retry {
                index: 0,
                reason: StopReason::Repetition { kept: 3 },
                pieces: 2
            }
        );
        // Prompts: chunk, sentence 1, sentence 2, sentence 2 penalized, next chunk.
        assert_eq!(backend.prompts.len(), 5);
        // Output that looped never becomes history.
        assert!(backend.prompts.iter().all(|p| !p.contains("la la")));
        assert!(backend.prompts[4].contains("<|plamo:msg|>BAD LOOP ONE."));
    }

    #[test]
    fn retries_a_chunk_that_ended_early() {
        let mut backend = Upper::new();
        let text = format!("{} SHORT. {}.", "a".repeat(150), "b".repeat(100));
        let (out, stats, progress) = run(&mut backend, &chars, DocumentOptions::default(), &text);
        assert_eq!(out, text.to_uppercase());
        assert_eq!((stats.retries, stats.cut), (1, 0));
        assert_eq!(
            progress[1],
            Progress::Retry {
                index: 0,
                reason: StopReason::Incomplete,
                pieces: 2
            }
        );
        // A short source may well have a short translation.
        assert!(!looks_incomplete(150, 10));
        assert!(!looks_incomplete(400, 100));
        assert!(looks_incomplete(400, 99));
    }

    #[test]
    fn retries_in_pieces_then_sentences() {
        let mut backend = Upper::new();
        let text = format!(
            "{}.\n\n{}. LOOP {}.\n\n{}.",
            "a".repeat(200),
            "b".repeat(100),
            "c".repeat(80),
            "d".repeat(200)
        );
        let (out, stats, progress) = run(&mut backend, &chars, DocumentOptions::default(), &text);
        assert_eq!(out, text.to_uppercase());
        // The chunk, its second piece, and that piece's second sentence.
        assert_eq!((stats.retries, stats.cut), (3, 0));
        let retries: Vec<_> = progress
            .iter()
            .filter_map(|p| match p {
                Progress::Retry { pieces, .. } => Some(*pieces),
                _ => None,
            })
            .collect();
        assert_eq!(retries, [3, 2]);
        // Chunk, piece 1, piece 2, its sentences 1 and 2, sentence 2 penalized, piece 3.
        assert_eq!(backend.prompts.len(), 7);
    }

    #[test]
    fn marks_where_output_was_cut() {
        let mut backend = Upper {
            prompts: vec![],
            stubborn: true,
        };
        let document = DocumentOptions {
            max_chunk_tokens: 6,
            max_history_tokens: 1000,
            cut_marker: "[CUT]".into(),
        };
        let (out, _, progress) = run(&mut backend, &chars, document, "LOOP.\n\nNext.");
        assert_eq!(out, "la [CUT]\n\nNEXT.");
        // The reported output range covers the marker.
        assert!(
            progress
                .iter()
                .any(|p| matches!(p, Progress::Cut { output: (0, 8), .. }))
        );
    }

    #[test]
    fn cuts_output_that_keeps_looping() {
        let mut backend = Upper {
            prompts: vec![],
            stubborn: true,
        };
        // Chunks of one paragraph each.
        let document = DocumentOptions {
            max_chunk_tokens: 6,
            max_history_tokens: 1000,
            ..DocumentOptions::default()
        };
        let (out, stats, progress) = run(&mut backend, &chars, document, "LOOP.\n\nNext.");
        // The single sentence goes straight to the penalized retry, and its first cycle is kept.
        assert_eq!(out, "la \n\nNEXT.");
        assert_eq!((stats.retries, stats.cut), (1, 1));
        assert_eq!(backend.prompts.len(), 3);
        assert!(progress.contains(&Progress::Cut {
            index: 0,
            reason: StopReason::Repetition { kept: 3 },
            source: (0, 5),
            output: (0, 3),
        }));
    }

    #[test]
    fn history_keeps_the_most_recent_pairs() {
        let pair = |s: &str| HistoryPair {
            source: s.into(),
            translation: s.into(),
        };
        let history = [pair("aaaa"), pair("bb"), pair("c")];
        // "c" costs 18 and "bb" 20, so a budget of 40 keeps exactly those two.
        let recent = recent_history(&history, &[24, 20, 18], 40);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].source, "bb");
    }

    #[test]
    fn hard_wrapped_sentences_stay_whole() {
        let text = "This is a long\nsentence that wraps. And a\nsecond one.";
        let s = split(text, 40, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            ["This is a long\nsentence that wraps.", "And a\nsecond one."]
        );
        // A sentence longer than the budget is split at its line breaks.
        let s = split(text, 20, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            [
                "This is a long",
                "sentence that wraps.",
                "And a\nsecond one."
            ]
        );
        assert_eq!(rejoin(&s), text);
    }

    #[test]
    fn single_newline_paragraphs_split_at_line_ends() {
        // Japanese prose: one paragraph per line, no blank lines, indented with U+3000.
        let text = "メロスは激怒した。王を除かねばならぬ。\n　メロスは村を出た。\n「待て」\n　そう言った。";
        let s = split(text, 25, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(
            texts,
            [
                "メロスは激怒した。王を除かねばならぬ。",
                "メロスは村を出た。\n「待て」\n　そう言った。"
            ]
        );
        assert_eq!(s.chunks[0].separator, "\n　");
        assert_eq!(rejoin(&s), text);
        // A line break inside a sentence is not a paragraph break.
        assert!(!ends_sentence("This is a long"));
        assert!(ends_sentence("He said \"no.\""));
        assert!(ends_sentence("「待て」"));
        assert!(!ends_sentence("(see above)"));
    }

    #[test]
    fn long_sentences_split_at_clauses_and_whitespace() {
        let text = "これは、とても長い、一文です";
        let s = split(text, 8, &chars);
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(texts, ["これは、", "とても長い、", "一文です"]);
        assert_eq!(rejoin(&s), text);

        let text = "one two three four five";
        let s = split(text, 10, &chars);
        assert!(s.chunks.iter().all(|c| chars(c.text) <= 10));
        assert_eq!(rejoin(&s), text);
    }

    #[test]
    fn text_without_boundaries_is_cut_to_fit() {
        let text = "x".repeat(100);
        let s = split(&text, 30, &chars);
        assert!(s.chunks.len() >= 4);
        assert!(s.chunks.iter().all(|c| chars(c.text) <= 30));
        assert_eq!(rejoin(&s), text);
    }

    #[test]
    fn sentences_of_does_not_pack() {
        let s = sentences_of("A b.\nC d. E.\n\nF.");
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(texts, ["A b.", "C d.", "E.", "F."]);
        assert_eq!(rejoin(&s), "A b.\nC d. E.\n\nF.");
    }

    #[test]
    fn whitespace_only_text_has_no_chunks() {
        let s = split(" \n\n ", 10, &chars);
        assert!(s.chunks.is_empty());
    }
}
