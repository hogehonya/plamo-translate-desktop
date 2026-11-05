//! Splitting long texts into chunks that fit the context window.
//!
//! Text is split at blank lines (paragraphs). Short neighbouring paragraphs are packed into one
//! chunk, and a paragraph that alone exceeds the budget is split again at sentence ends.
//! Separators between chunks are kept verbatim so the translation preserves the layout.
//!
//! [`translate_document`] translates the chunks in order and passes the most recent
//! source/translation pairs as history, so terminology and style stay consistent.

use std::time::Duration;

use pfml::{HistoryPair, Prompt, TranslationOptions};

use crate::{Result, StopReason, TranslateStats, TranslationBackend};

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

/// Splits `text` so each chunk measures at most `max_tokens` with `count_tokens`, where possible.
/// A single sentence longer than the budget is kept whole.
pub fn split<'a>(
    text: &'a str,
    max_tokens: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> Split<'a> {
    let body_start = text.len() - text.trim_start().len();
    let leading = &text[..body_start];

    // Paragraphs as (start, end, separator_end) byte ranges.
    let mut pieces = Vec::new();
    for (start, end, sep_end) in paragraphs(text, body_start) {
        let paragraph = &text[start..end];
        if count_tokens(paragraph) <= max_tokens {
            pieces.push((start, end, sep_end));
        } else {
            pieces.extend(sentences(text, start, end, sep_end));
        }
    }

    // Pack neighbouring pieces while they fit.
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let (start, mut end, mut sep_end) = pieces[i];
        let mut j = i + 1;
        while j < pieces.len() {
            let (_, next_end, next_sep_end) = pieces[j];
            if count_tokens(&text[start..next_end]) > max_tokens {
                break;
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

/// Sentence ranges within `text[start..end]`, the last one taking `sep_end`.
fn sentences(text: &str, start: usize, end: usize, sep_end: usize) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let mut sentence_start = start;
    let mut chars = text[start..end].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        let is_terminal = matches!(c, '。' | '！' | '？' | '．');
        let is_latin_terminal = matches!(c, '.' | '!' | '?')
            && chars.peek().is_none_or(|&(_, next)| next.is_whitespace());
        let is_newline = c == '\n';
        if !(is_terminal || is_latin_terminal || is_newline) {
            continue;
        }
        let sentence_end = start + offset + c.len_utf8();
        // Absorb closing quotes and brackets right after the terminator.
        let mut after = sentence_end;
        while let Some(&(o, next)) = chars.peek() {
            if matches!(next, '」' | '』' | '）' | ')' | '"' | '\'' | '”' | '’') {
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
/// the chunk, and the output. Of what remains after the fixed parts, a chunk gets a fifth and
/// history half: a history pair is about twice a chunk, and the output needs room too.
/// `on_chunk(index, count)` is called before each chunk.
#[allow(clippy::too_many_arguments)]
pub fn translate_document(
    backend: &mut dyn TranslationBackend,
    count_tokens: &dyn Fn(&str) -> usize,
    n_ctx: usize,
    options: &TranslationOptions,
    text: &str,
    sink: &mut dyn FnMut(&str),
    on_chunk: &mut dyn FnMut(usize, usize),
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
    let chunk_budget = available / 5;
    let history_budget = available / 2;

    let split = split(text, chunk_budget, count_tokens);
    let mut total = TranslateStats {
        prompt_tokens: 0,
        generated_tokens: 0,
        prompt_duration: Duration::ZERO,
        generation_duration: Duration::ZERO,
        stop_reason: StopReason::Finished,
    };
    if split.chunks.is_empty() {
        return Ok(total);
    }
    sink(split.leading);

    let mut history: Vec<HistoryPair> = Vec::new();
    let count = split.chunks.len();
    for (index, chunk) in split.chunks.iter().enumerate() {
        on_chunk(index, count);
        let recent = recent_history(&history, history_budget, count_tokens);
        let prompt = Prompt::new(options, chunk.text).with_history(recent);
        let mut translation = String::new();
        let stats = backend.translate(
            &prompt,
            &mut |piece| {
                translation.push_str(piece);
                sink(piece);
            },
            is_cancelled,
        )?;
        total.prompt_tokens += stats.prompt_tokens;
        total.generated_tokens += stats.generated_tokens;
        total.prompt_duration += stats.prompt_duration;
        total.generation_duration += stats.generation_duration;
        total.stop_reason = stats.stop_reason;
        if stats.stop_reason != StopReason::Finished {
            break;
        }
        sink(chunk.separator);
        let translation = translation.trim();
        if !translation.is_empty() {
            history.push(HistoryPair {
                source: chunk.text.to_owned(),
                translation: translation.to_owned(),
            });
        }
    }
    Ok(total)
}

/// The longest suffix of `history` that fits in `budget` tokens.
fn recent_history<'h>(
    history: &'h [HistoryPair],
    budget: usize,
    count_tokens: &dyn Fn(&str) -> usize,
) -> &'h [HistoryPair] {
    let mut used = 0;
    let mut start = history.len();
    for (i, pair) in history.iter().enumerate().rev() {
        used += count_tokens(&pair.source) + count_tokens(&pair.translation) + 16;
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
        let text = "  First paragraph here.\n\n\nSecond one.\n  \nThird.";
        let s = split(text, 15, &chars);
        assert_eq!(s.leading, "  ");
        let texts: Vec<_> = s.chunks.iter().map(|c| c.text).collect();
        assert_eq!(texts, ["First paragraph here.", "Second one.", "Third."]);
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
        let s = split(text, 16, &chars);
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

    /// Uppercases its input and records the prompts it received.
    struct Upper {
        prompts: Vec<String>,
    }

    impl TranslationBackend for Upper {
        fn translate(
            &mut self,
            prompt: &Prompt<'_>,
            sink: &mut dyn FnMut(&str),
            _: &dyn Fn() -> bool,
        ) -> Result<TranslateStats> {
            self.prompts.push(prompt.render()?);
            sink(&prompt.text.to_uppercase());
            Ok(TranslateStats {
                prompt_tokens: 1,
                generated_tokens: 1,
                prompt_duration: Duration::ZERO,
                generation_duration: Duration::ZERO,
                stop_reason: StopReason::Finished,
            })
        }
    }

    #[test]
    fn translates_chunks_with_history() {
        let options = TranslationOptions::new(pfml::LangCode::new("en").unwrap());
        let mut backend = Upper { prompts: vec![] };
        let mut out = String::new();
        let mut chunks = vec![];
        let text = "\nalpha beta gamma.\n\ndelta epsilon zeta.\n";
        // A tiny window forces one paragraph per chunk.
        let stats = translate_document(
            &mut backend,
            // 3 words = 180 tokens. With 1000 tokens available, a chunk gets 200 (so the two
            // paragraphs are separate) and history 500 (so the first pair fits).
            &|s| s.split_whitespace().count() * 60,
            1064,
            &options,
            text,
            &mut |s| out.push_str(s),
            &mut |i, n| chunks.push((i, n)),
            &|| false,
        )
        .unwrap();
        assert_eq!(out, "\nALPHA BETA GAMMA.\n\nDELTA EPSILON ZETA.\n");
        assert_eq!(chunks, [(0, 2), (1, 2)]);
        assert_eq!(stats.prompt_tokens, 2);
        // The second prompt carries the first pair as history.
        assert!(backend.prompts[1].contains("alpha beta gamma.<|plamo:tag|>output"));
        assert!(backend.prompts[1].contains("<|plamo:msg|>ALPHA BETA GAMMA."));
    }

    #[test]
    fn history_keeps_the_most_recent_pairs() {
        let pair = |s: &str| HistoryPair {
            source: s.into(),
            translation: s.into(),
        };
        let history = [pair("aaaa"), pair("bb"), pair("c")];
        // "c" costs 18 and "bb" 20, so a budget of 40 keeps exactly those two.
        let recent = recent_history(&history, 40, &chars);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].source, "bb");
    }

    #[test]
    fn whitespace_only_text_has_no_chunks() {
        let s = split(" \n\n ", 10, &chars);
        assert!(s.chunks.is_empty());
    }
}
