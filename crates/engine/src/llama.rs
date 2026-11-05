//! In-process inference with llama.cpp.
//!
//! Ownership mirrors llama.cpp's object graph: a [`Runtime`] loads [`Model`]s, and a model
//! creates [`Session`]s, each owning one context (KV cache). A session borrows its model, so the
//! intended use is a single worker thread that keeps the model and session alive across
//! requests and drops both when switching models.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Instant;

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LlamaBackendDeviceType, LogOptions};
use pfml::{Prompt, Segment, SpecialToken};

use crate::utf8::Utf8Stream;
use crate::{EngineError, Result, StopReason, TranslateStats, TranslationBackend};

#[derive(Debug, Clone, Default)]
pub struct RuntimeOptions {
    /// Forward llama.cpp logs to `tracing` instead of discarding them.
    pub logs: bool,
    /// Directory holding ggml backend modules (`dynamic-backends` builds only).
    ///
    /// Defaults to the directory of the current executable, which is where a packaged app keeps
    /// them. Only when nothing loads from there *and* the executable runs from its own cargo
    /// target directory (`cargo run`) is the build output directory tried as well; a packaged
    /// app never looks outside its own folder.
    pub backends_dir: Option<PathBuf>,
}

/// Process-wide llama.cpp state. Create at most one.
pub struct Runtime {
    backend: LlamaBackend,
}

impl Runtime {
    pub fn init(options: RuntimeOptions) -> Result<Self> {
        llama_cpp_2::send_logs_to_tracing(LogOptions::default().with_logs_enabled(options.logs));
        #[cfg(feature = "dynamic-backends")]
        {
            let dir = options.backends_dir.or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(Path::to_path_buf))
            });
            if let Some(dir) = dir {
                llama_cpp_2::llama_backend::load_backends_from_path(&dir);
            }
            if llama_cpp_2::list_llama_ggml_backend_devices().is_empty()
                && let Some(dir) = dev_backends_dir()
            {
                llama_cpp_2::llama_backend::load_backends_from_path(dir);
            }
            // With no backend registered, `llama_backend_init` would search ggml's default paths,
            // which include the build machine's directory and the current directory. Refuse
            // instead, so modules are only ever loaded from the application's own folder.
            if llama_cpp_2::list_llama_ggml_backend_devices().is_empty() {
                return Err(EngineError::NoBackends);
            }
        }
        #[cfg(not(feature = "dynamic-backends"))]
        let _ = options.backends_dir;
        let backend = LlamaBackend::init()?;
        Ok(Self { backend })
    }

    /// Lists the compute devices llama.cpp can use.
    pub fn devices(&self) -> Vec<Device> {
        llama_cpp_2::list_llama_ggml_backend_devices()
            .into_iter()
            .map(|d| Device {
                index: d.index,
                name: d.name,
                description: d.description,
                backend: d.backend,
                memory_total: d.memory_total as u64,
                memory_free: d.memory_free as u64,
                kind: match d.device_type {
                    LlamaBackendDeviceType::Cpu => DeviceKind::Cpu,
                    LlamaBackendDeviceType::Gpu => DeviceKind::Gpu,
                    LlamaBackendDeviceType::IntegratedGpu => DeviceKind::IntegratedGpu,
                    LlamaBackendDeviceType::Accelerator => DeviceKind::Accelerator,
                    LlamaBackendDeviceType::Unknown => DeviceKind::Unknown,
                },
            })
            .collect()
    }

    /// Loads a GGUF model. `progress` receives values in `0.0..=1.0`; returning `false` aborts.
    pub fn load_model(
        &self,
        path: &Path,
        options: &ModelOptions,
        progress: impl FnMut(f32) -> bool + 'static,
    ) -> Result<Model> {
        let mut params = LlamaModelParams::default().with_progress_callback(progress);
        params = match options.gpu {
            Some(GpuOffload { device, layers }) => params
                .with_devices(&[device])?
                .with_n_gpu_layers(layers.unwrap_or(u32::MAX)),
            None => params.with_n_gpu_layers(0),
        };
        let model = LlamaModel::load_from_file(&self.backend, path, &params)?;
        let specials = SpecialTokens::resolve(&model)?;
        Ok(Model { model, specials })
    }
}

/// The build output directory of the backend modules, if this executable was built into the
/// same cargo target directory (that is, it runs via `cargo run` or from `target/<profile>`).
///
/// The path is baked in at build time. On another machine it is just a path that someone else
/// might create, so it must not be searched there.
#[cfg(feature = "dynamic-backends")]
fn dev_backends_dir() -> Option<&'static Path> {
    let dir = Path::new(llama_cpp_sys_2::BACKENDS_DIR?);
    // <target>/<profile>/build/llama-cpp-sys-2-<hash>/out/backends
    let profile_dir = dir.ancestors().nth(4)?;
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let profile_dir = profile_dir.canonicalize().ok()?;
    exe.starts_with(&profile_dir).then_some(dir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Cpu,
    Gpu,
    IntegratedGpu,
    Accelerator,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// ggml device index, as accepted by [`GpuOffload::device`].
    pub index: usize,
    /// Backend-specific name such as `Vulkan0`.
    pub name: String,
    /// Human-readable description such as the GPU model.
    pub description: String,
    /// Backend name such as `CPU` or `Vulkan`.
    pub backend: String,
    pub memory_total: u64,
    pub memory_free: u64,
    pub kind: DeviceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuOffload {
    /// ggml device index from [`Runtime::devices`].
    pub device: usize,
    /// Number of layers to offload. `None` offloads all of them.
    pub layers: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelOptions {
    /// GPU offload. `None` runs entirely on the CPU.
    pub gpu: Option<GpuOffload>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionOptions {
    /// Context window in tokens (prompt plus generated output).
    pub n_ctx: u32,
    /// Maximum number of prompt tokens decoded per batch.
    pub n_batch: u32,
    /// CPU threads for generation. `None` uses llama.cpp's default.
    pub n_threads: Option<i32>,
    /// CPU threads for prompt processing. `None` uses llama.cpp's default.
    pub n_threads_batch: Option<i32>,
}

impl SessionOptions {
    /// Thread counts suited to this machine: prompt processing scales with physical cores,
    /// while generation is memory-bound and stops improving after a few threads.
    pub fn with_auto_threads(mut self) -> Self {
        let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
        let physical = (logical / 2).max(1) as i32;
        self.n_threads = Some(physical.min(8));
        self.n_threads_batch = Some(physical);
        self
    }
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            // The maximum length the model was trained with (input and output together).
            n_ctx: 32768,
            n_batch: 2048,
            n_threads: None,
            n_threads_batch: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SpecialTokens([LlamaToken; SpecialToken::ALL.len()]);

impl SpecialTokens {
    fn resolve(model: &LlamaModel) -> Result<Self> {
        let vocab = model.vocab();
        let mut ids = [LlamaToken(0); SpecialToken::ALL.len()];
        for token in SpecialToken::ALL {
            match vocab.tokenize(token.as_str().as_bytes(), false, true)[..] {
                [id] => ids[token as usize] = id,
                _ => return Err(EngineError::MissingSpecialToken(token.as_str())),
            }
        }
        Ok(Self(ids))
    }

    fn get(&self, token: SpecialToken) -> LlamaToken {
        self.0[token as usize]
    }
}

pub struct Model {
    model: LlamaModel,
    specials: SpecialTokens,
}

impl Model {
    /// Maximum context length the model was trained with, as recorded in the GGUF file.
    pub fn n_ctx_train(&self) -> u32 {
        self.model.n_ctx_train()
    }

    /// Size of the model weights in bytes.
    pub fn size(&self) -> u64 {
        self.model.size()
    }

    pub fn n_params(&self) -> u64 {
        self.model.n_params()
    }

    /// Number of tokens `text` occupies when used as prompt text.
    pub fn count_tokens(&self, text: &str) -> usize {
        self.model
            .vocab()
            .tokenize(text.as_bytes(), false, false)
            .len()
    }

    /// Tokenizes a prompt, including the leading BOS token.
    ///
    /// Special tokens map directly to their ids; user text is tokenized without special-token
    /// parsing, so it cannot inject control tokens.
    pub fn tokenize_prompt(&self, prompt: &Prompt<'_>) -> Result<Vec<LlamaToken>> {
        let vocab = self.model.vocab();
        let mut tokens = vec![self.specials.get(SpecialToken::Bos)];
        for segment in prompt.segments()? {
            match segment {
                Segment::Special(t) => tokens.push(self.specials.get(t)),
                Segment::Text(s) => {
                    vocab.tokenize_into(s.as_bytes(), &mut tokens, false, false);
                }
            }
        }
        Ok(tokens)
    }

    /// Tokenizes a raw string with special-token parsing, as llama.cpp would tokenize a
    /// handwritten prompt. Used to cross-check [`Model::tokenize_prompt`].
    pub fn tokenize_raw(&self, text: &str) -> Vec<LlamaToken> {
        self.model.vocab().tokenize(text.as_bytes(), false, true)
    }

    pub fn session(&self, runtime: &Runtime, options: &SessionOptions) -> Result<Session<'_>> {
        // Each request starts from an empty KV cache and never rewinds, so sliding-window
        // layers only need the window instead of a full-length cache. For PLaMo 3 (7 of every 8
        // layers use a 2048-token window) this cuts the cache by about 5x at 32k tokens.
        let mut params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(options.n_ctx))
            .with_n_batch(options.n_batch.min(options.n_ctx))
            .with_swa_full(false);
        if let Some(n) = options.n_threads {
            params = params.with_n_threads(n);
        }
        if let Some(n) = options.n_threads_batch.or(options.n_threads) {
            params = params.with_n_threads_batch(n);
        }
        let ctx = self.model.new_context(&runtime.backend, params)?;
        let n_batch = ctx.n_batch() as usize;
        Ok(Session {
            model: self,
            ctx,
            batch: LlamaBatch::new(n_batch, 1),
            n_batch,
        })
    }
}

/// A llama.cpp context bound to a model. Requests run one at a time.
pub struct Session<'m> {
    model: &'m Model,
    ctx: LlamaContext<'m>,
    batch: LlamaBatch<'static>,
    n_batch: usize,
}

impl Session<'_> {
    pub fn n_ctx(&self) -> usize {
        self.ctx.n_ctx() as usize
    }

    fn decode_prompt(&mut self, tokens: &[LlamaToken]) -> Result<()> {
        let last = tokens.len() - 1;
        for (chunk_index, chunk) in tokens.chunks(self.n_batch).enumerate() {
            self.batch.clear();
            let base = chunk_index * self.n_batch;
            for (i, &token) in chunk.iter().enumerate() {
                let pos = base + i;
                self.batch.add(token, pos as i32, &[0], pos == last)?;
            }
            self.ctx.decode(&mut self.batch)?;
        }
        Ok(())
    }
}

impl TranslationBackend for Session<'_> {
    fn translate(
        &mut self,
        prompt: &Prompt<'_>,
        sink: &mut dyn FnMut(&str),
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TranslateStats> {
        let tokens = self.model.tokenize_prompt(prompt)?;
        let n_ctx = self.n_ctx();
        if tokens.len() >= n_ctx {
            return Err(EngineError::PromptTooLong {
                prompt_tokens: tokens.len(),
                n_ctx,
            });
        }

        self.ctx.clear_kv_cache();
        let prompt_start = Instant::now();
        self.decode_prompt(&tokens)?;
        let prompt_duration = prompt_start.elapsed();

        let vocab = self.model.model.vocab();
        let tag = self.model.specials.get(SpecialToken::Tag);
        let mut sampler = LlamaSampler::greedy();
        let mut utf8 = Utf8Stream::default();
        let mut piece = Vec::new();
        let mut pos = tokens.len();
        let mut generated = 0;
        let generation_start = Instant::now();

        let stop_reason = loop {
            if is_cancelled() {
                break StopReason::Cancelled;
            }
            let token = sampler.sample(&self.ctx, self.batch.n_tokens() - 1);
            sampler.accept(token);
            if token == tag || vocab.is_eog(token) {
                break StopReason::Finished;
            }
            generated += 1;

            piece.clear();
            vocab.token_to_piece_into(token, &mut piece, false, None);
            let text = utf8.push(&piece);
            if !text.is_empty() {
                sink(&text);
            }

            if pos >= n_ctx {
                break StopReason::ContextFull;
            }
            self.batch.clear();
            self.batch.add(token, pos as i32, &[0], true)?;
            self.ctx.decode(&mut self.batch)?;
            pos += 1;
        };

        let rest = utf8.finish();
        if !rest.is_empty() {
            sink(&rest);
        }

        Ok(TranslateStats {
            prompt_tokens: tokens.len(),
            generated_tokens: generated,
            prompt_duration,
            generation_duration: generation_start.elapsed(),
            stop_reason,
        })
    }
}
