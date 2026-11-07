//! In-process inference with llama.cpp.
//!
//! Ownership mirrors llama.cpp's object graph: a [`Runtime`] loads [`Model`]s, and a model
//! creates [`Session`]s, each owning one context (KV cache). A session borrows its model, so the
//! intended use is a single worker thread that keeps the model and session alive across
//! requests and drops both when switching models.

use std::num::NonZeroU32;
use std::path::Path;
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

use crate::repetition::LoopDetector;
use crate::utf8::Utf8Stream;
use crate::{EngineError, Generation, Result, StopReason, TranslateStats, TranslationBackend};

/// Works around llama.cpp 0.8.0's Metal shaders, which do not compile on macOS 14 and 15: their
/// Metal compilers reject a `constexpr` in the BF16 code, and then the GPU cannot be used at all.
/// That code is compiled only with BF16 enabled, and the PLaMo GGUF files have no BF16 tensors.
/// llama.cpp fixed it after 0.8.0 (#31360); remove this once llama-cpp-rs ships the fix. A value
/// set beforehand is kept.
///
/// # Safety
///
/// Sets an environment variable, which the Metal backend reads when it starts: call this first
/// thing in `main`, while no other thread exists.
pub unsafe fn configure_metal() {
    #[cfg(target_os = "macos")]
    {
        const BF16_DISABLE: &str = "GGML_METAL_BF16_DISABLE";
        if std::env::var_os(BF16_DISABLE).is_none() {
            // SAFETY: the caller guarantees that no other thread exists.
            unsafe { std::env::set_var(BF16_DISABLE, "1") };
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeOptions {
    /// Forward llama.cpp logs to `tracing` instead of discarding them.
    pub logs: bool,
}

/// Process-wide llama.cpp state. Create at most one.
pub struct Runtime {
    backend: LlamaBackend,
}

impl Runtime {
    /// Starts llama.cpp.
    ///
    /// With `dynamic-backends`, the ggml backend modules are loaded from the directory of the
    /// current executable, which is where a packaged app keeps them. Only when nothing loads
    /// from there *and* the executable runs from its own cargo target directory (`cargo run`) is
    /// the build output directory tried as well; a packaged app never looks outside its own
    /// folder.
    pub fn init(options: RuntimeOptions) -> Result<Self> {
        llama_cpp_2::send_logs_to_tracing(LogOptions::default().with_logs_enabled(options.logs));
        #[cfg(feature = "dynamic-backends")]
        {
            let dir = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf));
            // llama-cpp-2 panics on a path that is not UTF-8; such a folder loads nothing.
            if let Some(dir) = dir.filter(|dir| dir.to_str().is_some()) {
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
        let backend = LlamaBackend::init()?;
        Ok(Self { backend })
    }

    /// Lists the compute devices llama.cpp can use.
    ///
    /// The OpenVINO backend (in packages for Intel NPUs) registers every OpenVINO device but
    /// computes only on the one it chose at startup, which is the NPU when the NPU plugin is the
    /// only OpenVINO plugin shipped. Only that NPU is listed, as [`DeviceKind::Npu`].
    pub fn devices(&self) -> Vec<Device> {
        llama_cpp_2::list_llama_ggml_backend_devices()
            .into_iter()
            .filter_map(|d| {
                let device = Device {
                    index: d.index,
                    name: d.name,
                    // CPU names come padded with spaces.
                    description: d.description.trim().to_owned(),
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
                };
                if device.backend == OPENVINO {
                    openvino_npu(device)
                } else {
                    Some(device)
                }
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
                .with_n_gpu_layers(layers.unwrap_or(u32::MAX))
                // With every layer on the GPU, a memory-mapped file would keep a second copy of
                // the weights in the process's memory for as long as the model is loaded. Read
                // it instead, so the host copy is freed once the weights are uploaded. Metal on
                // Apple silicon is the exception: with unified memory the GPU uses the mapped
                // pages themselves, and reading would put the weights in the process's own memory.
                .with_use_mmap(layers.is_some() || cfg!(target_os = "macos")),
            // No devices at all: by default llama.cpp also gives every GPU-type device large
            // operations of a CPU model (op offload), which would use a GPU the user chose not
            // to use, or the placeholder OpenVINO registers without an NPU, which fails.
            None => params.with_devices(&[])?.with_n_gpu_layers(0),
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

/// The backend name of ggml's OpenVINO backend.
const OPENVINO: &str = "OPENVINO";

/// The OpenVINO device as listed: the NPU the backend computes on, or `None` for any other.
///
/// The backend describes its devices as `GGML_OPENVINO_DEVICE=NPU (selected) - Intel(R) AI
/// Boost`: the OpenVINO device id, whether it is the one computed on, and the device's name.
/// Without any OpenVINO device it registers a placeholder `CPU`, which is left out too.
fn openvino_npu(mut device: Device) -> Option<Device> {
    let rest = device.description.strip_prefix("GGML_OPENVINO_DEVICE=")?;
    let (id, name) = rest.split_once(" - ").unwrap_or((rest, ""));
    let id = id.strip_suffix(" (selected)")?;
    if !id.starts_with("NPU") {
        return None;
    }
    device.description = if name.is_empty() { id } else { name }.to_owned();
    device.name = "NPU".to_owned();
    device.kind = DeviceKind::Npu;
    Some(device)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    Cpu,
    Gpu,
    IntegratedGpu,
    /// An NPU, through OpenVINO. Used only when chosen: it is not picked automatically.
    Npu,
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
    /// Keep the sliding-window layers' cache as long as the context instead of only the window.
    pub swa_full: bool,
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

    /// Settings for an NPU ([`DeviceKind::Npu`]), whose graphs have fixed shapes: every token
    /// costs as much as at the end of a full context, so the context is shorter (long texts are
    /// translated in chunks that fit, see `crate::document`). The OpenVINO backend also sizes
    /// every attention mask for the whole context, so the sliding-window cache must be as long.
    pub fn for_npu(mut self) -> Self {
        self.n_ctx = NPU_CONTEXT;
        self.swa_full = true;
        self
    }
}

/// Context size on an NPU (see [`SessionOptions::for_npu`]).
pub const NPU_CONTEXT: u32 = 8192;

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            // The maximum length the model was trained with (input and output together).
            n_ctx: 32768,
            n_batch: 2048,
            n_threads: None,
            n_threads_batch: None,
            swa_full: false,
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
        // layers only need the window instead of a full-length cache (unless `swa_full`). For
        // PLaMo 3 (7 of every 8 layers use a 2048-token window) this cuts the cache by about 5x
        // at 32k tokens.
        let mut params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(options.n_ctx))
            .with_n_batch(options.n_batch.min(options.n_ctx))
            .with_swa_full(options.swa_full);
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
        generation: &Generation,
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
        // On a GPU, decoding returns before the work is done; without waiting, the prompt's
        // time would count as the first generated token's.
        self.ctx.synchronize();
        let prompt_duration = prompt_start.elapsed();

        let vocab = self.model.model.vocab();
        let tag = self.model.specials.get(SpecialToken::Tag);
        let mut sampler = if generation.penalize_repetition {
            // llama.cpp's recommended DRY settings. The sampler only sees generated tokens, so
            // it penalizes the output repeating itself, not copying names from the source. The
            // window is the whole context: llama.cpp turns -1 into 0, which disables DRY.
            let window = i32::try_from(self.n_ctx()).unwrap_or(i32::MAX);
            LlamaSampler::chain_simple([
                LlamaSampler::dry(
                    &self.model.model,
                    0.8,
                    1.75,
                    2,
                    window,
                    ["\n", ":", "\"", "*"],
                ),
                LlamaSampler::greedy(),
            ])
        } else {
            LlamaSampler::greedy()
        };
        let mut loops = LoopDetector::for_input(tokens.iter().map(|t| t.0).collect());
        let mut utf8 = Utf8Stream::default();
        let mut piece = Vec::new();
        let mut pos = tokens.len();
        let mut generated = 0;
        // Bytes of output passed to `sink` after each generated token.
        let mut emitted = 0;
        let mut ends = Vec::new();
        let generation_start = Instant::now();

        let stop_reason = loop {
            if is_cancelled() {
                break StopReason::Cancelled;
            }
            // `sample` also accepts the token into the sampler's history (which DRY reads).
            let token = sampler.sample(&self.ctx, self.batch.n_tokens() - 1);
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
            emitted += text.len();
            ends.push(emitted);

            if let Some(found) = loops.push(token.0) {
                break StopReason::Repetition {
                    kept: ends[loops.keep(found) - 1],
                };
            }
            if generated >= generation.max_tokens {
                break StopReason::LengthLimit;
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
            retries: 0,
            cut: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Device, DeviceKind, openvino_npu};

    fn openvino(description: &str) -> Device {
        Device {
            index: 0,
            name: "OPENVINO0".into(),
            description: description.into(),
            backend: "OPENVINO".into(),
            memory_total: 0,
            memory_free: 0,
            kind: DeviceKind::Gpu,
        }
    }

    #[test]
    fn lists_only_the_selected_openvino_npu() {
        let npu = openvino_npu(openvino(
            "GGML_OPENVINO_DEVICE=NPU (selected) - Intel(R) AI Boost (NPU 5010)",
        ))
        .unwrap();
        assert_eq!(
            (npu.name.as_str(), npu.description.as_str(), npu.kind),
            ("NPU", "Intel(R) AI Boost (NPU 5010)", DeviceKind::Npu)
        );
        // The placeholder registered without any OpenVINO device, devices not computed on,
        // and other OpenVINO devices are left out.
        for description in [
            "GGML_OPENVINO_DEVICE=CPU (selected) - CPU",
            "GGML_OPENVINO_DEVICE=NPU - Intel(R) AI Boost",
            "GGML_OPENVINO_DEVICE=GPU.0 (selected) - Intel(R) Arc(TM) Graphics",
            "Intel(R) AI Boost",
        ] {
            assert!(
                openvino_npu(openvino(description)).is_none(),
                "{description}"
            );
        }
        let bare = openvino_npu(openvino("GGML_OPENVINO_DEVICE=NPU (selected)")).unwrap();
        assert_eq!(bare.description, "NPU");
    }
}
