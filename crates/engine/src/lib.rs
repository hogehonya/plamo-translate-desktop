//! Translation engine for PLaMo Translate Desktop.
//!
//! The engine is GUI-agnostic. [`llama`] runs PLaMo 3 Translate GGUF models in-process with
//! llama.cpp.

pub mod discovery;
pub mod document;
pub mod download;
pub mod llama;
pub mod models;
pub mod repetition;
mod utf8;
pub mod worker;

use std::time::Duration;

use thiserror::Error;

pub use pfml;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Prompt(#[from] pfml::PromptError),
    #[error("llama.cpp: {0}")]
    Llama(#[from] llama_cpp_2::LlamaCppError),
    #[error("failed to load model: {0}")]
    ModelLoad(#[from] llama_cpp_2::LlamaModelLoadError),
    #[error("failed to create context: {0}")]
    ContextLoad(#[from] llama_cpp_2::LlamaContextLoadError),
    #[error("decode failed: {0}")]
    Decode(#[from] llama_cpp_2::DecodeError),
    #[error("batch error: {0}")]
    Batch(#[from] llama_cpp_2::llama_batch::BatchAddError),
    #[error("not a PLaMo 3 Translate model: special token {0} is missing")]
    MissingSpecialToken(&'static str),
    #[error("no compute backend found: the ggml backend modules must be next to the executable")]
    NoBackends,
    #[error("prompt is too long: {prompt_tokens} tokens for a context of {n_ctx}")]
    PromptTooLong { prompt_tokens: usize, n_ctx: usize },
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

/// User agent for requests to Hugging Face.
pub(crate) const USER_AGENT: &str = concat!("plamo-translate-desktop/", env!("CARGO_PKG_VERSION"));

/// Why generation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model finished the translation.
    Finished,
    /// The caller cancelled the request.
    Cancelled,
    /// The context window filled up before the translation finished.
    ContextFull,
    /// The output fell into a loop (see [`repetition`]). `kept` is the length in bytes of the
    /// output worth keeping: up to the end of the cycle's first occurrence, or, for a cycle
    /// copied from the input, as many repeats as the input has.
    Repetition { kept: usize },
    /// The output reached [`Generation::max_tokens`].
    LengthLimit,
    /// Document translation only: the model ended a chunk with an output far shorter than its
    /// source, having skipped the rest of it.
    Incomplete,
}

impl StopReason {
    /// Whether the output went wrong: it looped, grew far longer than the input, or ended far
    /// too early.
    pub fn is_runaway(self) -> bool {
        matches!(
            self,
            Self::Repetition { .. } | Self::LengthLimit | Self::Incomplete
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslateStats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub prompt_duration: Duration,
    pub generation_duration: Duration,
    pub stop_reason: StopReason,
    /// Document translation: pieces translated again because the first output ran away.
    pub retries: usize,
    /// Document translation: pieces whose output still ran away after retrying and was cut.
    pub cut: usize,
}

impl TranslateStats {
    /// The stats of a job cancelled before it started.
    pub fn cancelled() -> Self {
        Self {
            prompt_tokens: 0,
            generated_tokens: 0,
            prompt_duration: Duration::ZERO,
            generation_duration: Duration::ZERO,
            stop_reason: StopReason::Cancelled,
            retries: 0,
            cut: 0,
        }
    }

    pub fn generation_tokens_per_second(&self) -> f64 {
        let secs = self.generation_duration.as_secs_f64();
        if secs > 0.0 {
            self.generated_tokens as f64 / secs
        } else {
            0.0
        }
    }
}

/// Settings for generating one translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation {
    /// Stop with [`StopReason::LengthLimit`] after this many tokens.
    pub max_tokens: usize,
    /// Penalize repeating earlier output (llama.cpp's DRY sampler) instead of decoding purely
    /// greedily. This also discourages legitimate repetition, so it is only used for the last
    /// retry of output that ran away.
    pub penalize_repetition: bool,
}

impl Generation {
    pub fn greedy(max_tokens: usize) -> Self {
        Self {
            max_tokens,
            penalize_repetition: false,
        }
    }
}

/// Something that can translate a single PFML prompt, streaming the output.
pub trait TranslationBackend {
    /// Translates `prompt`, passing decoded text to `sink` as it is generated.
    ///
    /// Generation stops early with [`StopReason::Repetition`] when the output loops.
    /// `is_cancelled` is polled before every generated token.
    fn translate(
        &mut self,
        prompt: &pfml::Prompt<'_>,
        generation: &Generation,
        sink: &mut dyn FnMut(&str),
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranslateStats>;
}
