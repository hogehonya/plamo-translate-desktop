//! Headless translation CLI for testing the engine without the GUI.
//!
//! ```text
//! cargo run -p engine --release --example translate -- \
//!     --model path/to/plamo-3-translate-2b-Q4_K_M.gguf --to en "会議は来週の月曜日に変更されました。"
//! ```
//!
//! Reads the text from stdin when no positional argument is given. The translation streams to
//! stdout; output that runs away and is retried cannot be taken back there, so use `--output` to
//! get the final text.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, bail};
use engine::document::{DocumentOptions, Output, Progress, translate_document};
use engine::llama::{
    DeviceKind, GpuOffload, ModelOptions, Runtime, RuntimeOptions, SessionOptions,
};
use engine::pfml::{Form, GlossaryEntry, LangCode, Prompt, TranslationOptions};
use engine::worker::{Event, MeasureRequest, Worker};

const USAGE: &str = "\
usage: translate [options] [TEXT]

options:
  --model PATH        GGUF model file (required unless --list-devices)
  --to LANG           target language code (default: ja)
  --from LANG         source language code (default: auto-detect)
  --form plain|polite Japanese style (target ja only)
  --context TEXT      reference context
  --glossary SRC=TGT  glossary entry (repeatable)
  --ctx N             context size in tokens (default: 32768)
  --chunk-tokens N    upper bound for a chunk of a long text (default: 1024)
  --history-tokens N  upper bound for the history sent with a chunk (default: 4096)
  --output PATH       write the final translation to PATH instead of streaming it
  --chunks PATH       write each chunk's source and translation to PATH (JSON lines)
  --threads N         CPU threads
  --gpu INDEX         offload to the ggml device INDEX (see --list-devices)
  --gpu-layers N      number of layers to offload (default: all)
  --npu-session       session settings for an NPU (8192-token context, full sliding-window
                      cache), as used with --gpu on an NPU; for testing OpenVINO's static path
                      elsewhere with GGML_OPENVINO_FORCE_STATIC=1
  --list-devices      list compute devices and exit
  --measure           measure how fast the CPU and each GPU translate with --model, as the
                      app's Measure and choose button does, and exit
  --check-tokenizer   verify segment tokenization matches raw prompt tokenization
  --verbose           show llama.cpp logs
";

#[derive(Default)]
struct Args {
    model: Option<PathBuf>,
    to: Option<String>,
    from: Option<String>,
    form: Option<Form>,
    context: String,
    glossary: Vec<GlossaryEntry>,
    n_ctx: Option<u32>,
    chunk_tokens: Option<usize>,
    history_tokens: Option<usize>,
    output: Option<PathBuf>,
    chunks: Option<PathBuf>,
    threads: Option<i32>,
    gpu: Option<usize>,
    gpu_layers: Option<u32>,
    npu_session: bool,
    list_devices: bool,
    measure: bool,
    check_tokenizer: bool,
    verbose: bool,
    text: Option<String>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--model" => args.model = Some(value()?.into()),
            "--to" => args.to = Some(value()?),
            "--from" => args.from = Some(value()?),
            "--form" => {
                args.form = Some(match value()?.as_str() {
                    "plain" => Form::Plain,
                    "polite" => Form::Polite,
                    other => bail!("unknown form {other:?}"),
                })
            }
            "--context" => args.context = value()?,
            "--glossary" => {
                let v = value()?;
                let (source, target) = v.split_once('=').context("--glossary expects SRC=TGT")?;
                args.glossary.push(GlossaryEntry {
                    source: source.into(),
                    target: target.into(),
                });
            }
            "--ctx" => args.n_ctx = Some(value()?.parse()?),
            "--chunk-tokens" => args.chunk_tokens = Some(value()?.parse()?),
            "--history-tokens" => args.history_tokens = Some(value()?.parse()?),
            "--output" => args.output = Some(value()?.into()),
            "--chunks" => args.chunks = Some(value()?.into()),
            "--threads" => args.threads = Some(value()?.parse()?),
            "--gpu" => args.gpu = Some(value()?.parse()?),
            "--gpu-layers" => args.gpu_layers = Some(value()?.parse()?),
            "--npu-session" => args.npu_session = true,
            "--list-devices" => args.list_devices = true,
            "--measure" => args.measure = true,
            "--check-tokenizer" => args.check_tokenizer = true,
            "--verbose" => args.verbose = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with("--") => bail!("unknown option {s}\n\n{USAGE}"),
            _ => args.text = Some(arg),
        }
    }
    Ok(args)
}

fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    if args.verbose {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .init();
    }
    if args.measure {
        let model = args.model.context("--model is required")?;
        return measure(model, args.verbose);
    }
    let runtime = Runtime::init(RuntimeOptions { logs: args.verbose })?;

    if args.list_devices {
        for d in runtime.devices() {
            println!(
                "{:>2}  {:<10} {:<8} {:?}  {} ({} / {} MiB free)",
                d.index,
                d.name,
                d.backend,
                d.kind,
                d.description,
                d.memory_free >> 20,
                d.memory_total >> 20,
            );
        }
        return Ok(());
    }

    let model_path = args.model.context("--model is required")?;
    let text = match args.text {
        Some(t) => t,
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
    };

    let mut options = TranslationOptions::new(LangCode::new(args.to.as_deref().unwrap_or("ja"))?);
    options.source = args.from.as_deref().map(LangCode::new).transpose()?;
    options.form = args.form;
    options.context = args.context;
    options.glossary = args.glossary;
    let prompt = Prompt::new(&options, &text);

    let model_options = ModelOptions {
        gpu: args.gpu.map(|device| GpuOffload {
            device,
            layers: args.gpu_layers,
        }),
    };
    let load_start = std::time::Instant::now();
    let model = runtime.load_model(&model_path, &model_options, |_| true)?;
    eprintln!(
        "[loaded {} ({:.2} B params, {} MiB) in {:.2?}, n_ctx_train={}]",
        model_path.display(),
        model.n_params() as f64 / 1e9,
        model.size() >> 20,
        load_start.elapsed(),
        model.n_ctx_train(),
    );

    if args.check_tokenizer {
        let segmented = model.tokenize_prompt(&prompt)?;
        let raw = model.tokenize_raw(&format!("<|plamo:bos|>{}", prompt.render()?));
        if segmented == raw {
            eprintln!("[tokenizer check OK: {} tokens]", raw.len());
        } else {
            bail!(
                "tokenizer mismatch:\n  segmented: {:?}\n  raw:       {:?}",
                segmented,
                raw
            );
        }
    }

    let mut session_options = SessionOptions {
        n_threads: args.threads,
        ..SessionOptions::default()
    };
    let on_npu = args.gpu.is_some_and(|index| {
        runtime
            .devices()
            .iter()
            .any(|d| d.index == index && d.kind == DeviceKind::Npu)
    });
    if args.npu_session || on_npu {
        session_options = session_options.for_npu();
    }
    if let Some(n) = args.n_ctx {
        session_options.n_ctx = n;
    }
    let mut session = model.session(&runtime, &session_options)?;

    // Same path as the app: long texts are split and translated with history.
    let n_ctx = session.n_ctx();
    let defaults = DocumentOptions::default();
    let document = DocumentOptions {
        max_chunk_tokens: args.chunk_tokens.unwrap_or(defaults.max_chunk_tokens),
        max_history_tokens: args.history_tokens.unwrap_or(defaults.max_history_tokens),
        cut_marker: " [translation cut off: it kept repeating]".into(),
    };
    let streaming = args.output.is_none();
    // Shared by the output and progress callbacks.
    let translation = std::cell::RefCell::new(String::new());
    // (source range, output offset) at the start of each chunk.
    let mut chunk_starts: Vec<((usize, usize), usize)> = Vec::new();
    let mut stdout = std::io::stdout();
    let started = std::time::Instant::now();
    let stats = translate_document(
        &mut session,
        &|s| model.count_tokens(s),
        n_ctx,
        &document,
        &options,
        &text,
        &mut |output| match output {
            Output::Text(piece) => {
                translation.borrow_mut().push_str(piece);
                if streaming {
                    let _ = stdout.write_all(piece.as_bytes());
                    let _ = stdout.flush();
                }
            }
            Output::Truncate(len) => {
                translation.borrow_mut().truncate(len);
                if streaming {
                    eprintln!("\n[output withdrawn back to byte {len}]");
                }
            }
        },
        &mut |progress| match progress {
            Progress::Chunk {
                index,
                count,
                source,
            } => {
                chunk_starts.push((source, translation.borrow().len()));
                if count > 1 {
                    eprintln!("[chunk {}/{count}, {:.0?}]", index + 1, started.elapsed());
                }
            }
            Progress::Retry {
                index,
                reason,
                pieces,
            } => eprintln!(
                "[chunk {}: {reason:?}, retrying in {pieces} pieces]",
                index + 1
            ),
            Progress::Cut {
                index,
                reason,
                source: (start, end),
                ..
            } => eprintln!(
                "[chunk {}: {reason:?} again, output cut; source bytes {start}..{end}]",
                index + 1
            ),
        },
        &|| false,
    )?;
    let translation = translation.into_inner();
    match &args.output {
        Some(path) => std::fs::write(path, &translation)?,
        None => println!(),
    }
    if let Some(path) = &args.chunks {
        let mut lines = String::new();
        for (i, &((start, end), from)) in chunk_starts.iter().enumerate() {
            let to = chunk_starts
                .get(i + 1)
                .map_or(translation.len(), |&(_, offset)| offset);
            let line = serde_json::json!({
                "index": i,
                "source": &text[start..end],
                "translation": translation[from..to].trim(),
            });
            lines.push_str(&line.to_string());
            lines.push('\n');
        }
        std::fs::write(path, lines)?;
    }
    eprintln!(
        "[{:?}: prompt {} tok in {:.2?}, generated {} tok in {:.2?} ({:.1} tok/s), \
         {} retried, {} cut]",
        stats.stop_reason,
        stats.prompt_tokens,
        stats.prompt_duration,
        stats.generated_tokens,
        stats.generation_duration,
        stats.generation_tokens_per_second(),
        stats.retries,
        stats.cut,
    );
    Ok(())
}

/// Measures the CPU and each GPU with the worker, as the app does, and prints the speeds.
fn measure(model: PathBuf, logs: bool) -> anyhow::Result<()> {
    let (events, received) = std::sync::mpsc::channel();
    let worker = Worker::spawn(RuntimeOptions { logs }, move |event| {
        let _ = events.send(event);
    });
    let devices = loop {
        match received.recv()? {
            Event::Devices(devices) => break devices,
            Event::RuntimeFailed(error) => bail!(error),
            _ => {}
        }
    };
    // The app's candidates (appdata::devices::candidates): the CPU and the GPUs.
    let candidates: Vec<Option<usize>> = std::iter::once(None)
        .chain(
            devices
                .iter()
                .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
                .map(|d| Some(d.index)),
        )
        .collect();
    worker.measure(MeasureRequest {
        path: model,
        devices: candidates,
    });
    loop {
        match received.recv()? {
            Event::Measuring { index, count } => eprintln!("[measuring {}/{count}]", index + 1),
            Event::Measured(results) => {
                for m in results {
                    let name = m
                        .device
                        .and_then(|index| devices.iter().find(|d| d.index == index))
                        .map_or("CPU", |d| d.name.as_str());
                    match m.result {
                        Ok(speed) => println!(
                            "{name}: prompt {:.0} tok/s, generation {:.1} tok/s, a typical piece {:.2} s",
                            speed.prompt_tokens_per_second,
                            speed.generation_tokens_per_second,
                            speed.typical_seconds()
                        ),
                        Err(error) => println!("{name}: failed: {error}"),
                    }
                }
                return Ok(());
            }
            Event::RuntimeFailed(error) => bail!(error),
            _ => {}
        }
    }
}
