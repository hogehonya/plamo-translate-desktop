//! Translation engine for PLaMo Translate Desktop.
//!
//! The engine is GUI-agnostic. [`llama`] runs PLaMo 3 Translate GGUF models in-process with
//! llama.cpp.

pub mod discovery;
pub mod document;
pub mod download;
pub mod llama;
pub mod models;
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

/// Why generation stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model finished the translation.
    Finished,
    /// The caller cancelled the request.
    Cancelled,
    /// The context window filled up before the translation finished.
    ContextFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslateStats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub prompt_duration: Duration,
    pub generation_duration: Duration,
    pub stop_reason: StopReason,
}

impl TranslateStats {
    pub fn generation_tokens_per_second(&self) -> f64 {
        let secs = self.generation_duration.as_secs_f64();
        if secs > 0.0 {
            self.generated_tokens as f64 / secs
        } else {
            0.0
        }
    }
}

/// Something that can translate a single PFML prompt, streaming the output.
pub trait TranslationBackend {
    /// Translates `prompt`, passing decoded text to `sink` as it is generated.
    ///
    /// `is_cancelled` is polled before every generated token.
    fn translate(
        &mut self,
        prompt: &pfml::Prompt<'_>,
        sink: &mut dyn FnMut(&str),
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranslateStats>;
}
