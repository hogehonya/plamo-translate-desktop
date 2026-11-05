//! Headless translation CLI for testing the engine without the GUI.
//!
//! ```text
//! cargo run -p engine --release --example translate -- \
//!     --model path/to/plamo-3-translate-2b-Q4_K_M.gguf --to en "会議は来週の月曜日に変更されました。"
//! ```
//!
//! Reads the text from stdin when no positional argument is given.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, bail};
use engine::document::translate_document;
use engine::llama::{GpuOffload, ModelOptions, Runtime, RuntimeOptions, SessionOptions};
use engine::pfml::{Form, GlossaryEntry, LangCode, Prompt, TranslationOptions};

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
  --threads N         CPU threads
  --gpu INDEX         offload to the ggml device INDEX (see --list-devices)
  --gpu-layers N      number of layers to offload (default: all)
  --list-devices      list compute devices and exit
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
    threads: Option<i32>,
    gpu: Option<usize>,
    gpu_layers: Option<u32>,
    list_devices: bool,
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
            "--threads" => args.threads = Some(value()?.parse()?),
            "--gpu" => args.gpu = Some(value()?.parse()?),
            "--gpu-layers" => args.gpu_layers = Some(value()?.parse()?),
            "--list-devices" => args.list_devices = true,
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
    let runtime = Runtime::init(RuntimeOptions {
        logs: args.verbose,
        backends_dir: None,
    })?;

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
    if let Some(n) = args.n_ctx {
        session_options.n_ctx = n;
    }
    let mut session = model.session(&runtime, &session_options)?;

    // Same path as the app: long texts are split and translated with history.
    let n_ctx = session.n_ctx();
    let mut stdout = std::io::stdout();
    let stats = translate_document(
        &mut session,
        &|s| model.count_tokens(s),
        n_ctx,
        &options,
        &text,
        &mut |piece| {
            let _ = stdout.write_all(piece.as_bytes());
            let _ = stdout.flush();
        },
        &mut |index, count| {
            if count > 1 {
                eprintln!("\n[chunk {}/{count}]", index + 1);
            }
        },
        &|| false,
    )?;
    println!();
    eprintln!(
        "[{:?}: prompt {} tok in {:.2?}, generated {} tok in {:.2?} ({:.1} tok/s)]",
        stats.stop_reason,
        stats.prompt_tokens,
        stats.prompt_duration,
        stats.generated_tokens,
        stats.generation_duration,
        stats.generation_tokens_per_second(),
    );
    Ok(())
}
