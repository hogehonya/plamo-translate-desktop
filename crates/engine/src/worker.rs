//! Inference worker thread.
//!
//! All llama.cpp state lives on one dedicated thread. Callers send [`Command`]s through a
//! [`Worker`] handle and receive [`Event`]s through a callback.
//!
//! Jobs supersede each other: submitting a translation cancels the one in flight, and queued
//! jobs that are already stale are skipped. That keeps auto-translate responsive while the
//! user types.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

use pfml::TranslationOptions;

use crate::TranslateStats;
use crate::document::translate_document;
use crate::llama::{Device, Model, ModelOptions, Runtime, RuntimeOptions, SessionOptions};

/// Identifies a translation job. Ids increase monotonically per worker.
pub type JobId = u64;

#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub path: PathBuf,
    pub model: ModelOptions,
    pub session: SessionOptions,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub path: PathBuf,
    pub n_params: u64,
    pub size: u64,
    pub n_ctx: usize,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// llama.cpp could not be initialized. The worker has exited.
    RuntimeFailed(String),
    /// The compute devices available, sent once after llama.cpp starts.
    Devices(Vec<Device>),
    LoadProgress(f32),
    Loaded(ModelInfo),
    LoadFailed(String),
    /// A long text was split; chunk `index` of `count` is starting.
    Chunk {
        job: JobId,
        index: usize,
        count: usize,
    },
    /// Generated text for a job, in order.
    Token {
        job: JobId,
        text: String,
    },
    Finished {
        job: JobId,
        stats: TranslateStats,
    },
    Failed {
        job: JobId,
        error: String,
    },
}

enum Command {
    Load(LoadRequest),
    Unload,
    Translate {
        job: JobId,
        options: TranslationOptions,
        text: String,
    },
}

type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

/// Handle to the inference worker. Dropping it shuts the worker down after the current job.
pub struct Worker {
    commands: mpsc::Sender<Command>,
    /// The newest job id. Any job with a different id is cancelled or skipped.
    latest: Arc<AtomicU64>,
}

impl Worker {
    pub fn spawn(options: RuntimeOptions, events: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let (commands, rx) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let events: EventSink = Arc::new(events);
        let thread_latest = latest.clone();
        thread::Builder::new()
            .name("inference".into())
            .spawn(move || run(options, rx, events, thread_latest))
            .expect("failed to spawn the inference thread");
        Self { commands, latest }
    }

    /// Loads a model, replacing the current one. Cancels any running job.
    pub fn load(&self, request: LoadRequest) {
        self.cancel();
        self.send(Command::Load(request));
    }

    pub fn unload(&self) {
        self.cancel();
        self.send(Command::Unload);
    }

    /// Starts a translation, cancelling any earlier one. Returns the new job id.
    pub fn translate(&self, options: TranslationOptions, text: String) -> JobId {
        let job = self.latest.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(Command::Translate { job, options, text });
        job
    }

    /// Cancels the running job, if any.
    pub fn cancel(&self) {
        self.latest.fetch_add(1, Ordering::SeqCst);
    }

    fn send(&self, command: Command) {
        // The worker only exits after the runtime failed, which was already reported.
        let _ = self.commands.send(command);
    }
}

fn run(
    options: RuntimeOptions,
    commands: mpsc::Receiver<Command>,
    events: EventSink,
    latest: Arc<AtomicU64>,
) {
    let runtime = match Runtime::init(options) {
        Ok(r) => r,
        Err(e) => {
            events(Event::RuntimeFailed(e.to_string()));
            return;
        }
    };
    events(Event::Devices(runtime.devices()));

    let mut next = None;
    loop {
        let Some(command) = next.take().or_else(|| commands.recv().ok()) else {
            return;
        };
        match command {
            Command::Load(request) => {
                if let Some(model) = load(&runtime, &request, &events) {
                    // Serve requests until the next load; the model drops at the end of this arm.
                    next = serve(&runtime, &model, &request, &commands, &events, &latest);
                }
            }
            Command::Unload => {}
            Command::Translate { job, .. } => events(Event::Failed {
                job,
                error: "no model is loaded".into(),
            }),
        }
    }
}

fn load(runtime: &Runtime, request: &LoadRequest, events: &EventSink) -> Option<Model> {
    let progress_events = events.clone();
    let result = runtime.load_model(&request.path, &request.model, move |p| {
        progress_events(Event::LoadProgress(p));
        true
    });
    match result {
        Ok(model) => Some(model),
        Err(e) => {
            events(Event::LoadFailed(e.to_string()));
            None
        }
    }
}

/// Runs translation jobs on `model`. Returns the command that ended the session
/// (a new load), or `None` when the session was unloaded or the channel closed.
fn serve(
    runtime: &Runtime,
    model: &Model,
    request: &LoadRequest,
    commands: &mpsc::Receiver<Command>,
    events: &EventSink,
    latest: &AtomicU64,
) -> Option<Command> {
    let mut session = match model.session(runtime, &request.session) {
        Ok(s) => s,
        Err(e) => {
            events(Event::LoadFailed(e.to_string()));
            return None;
        }
    };
    events(Event::Loaded(ModelInfo {
        path: request.path.clone(),
        n_params: model.n_params(),
        size: model.size(),
        n_ctx: session.n_ctx(),
    }));

    loop {
        let command = commands.recv().ok()?;
        let (job, options, text) = match command {
            Command::Load(_) => return Some(command),
            Command::Unload => return None,
            Command::Translate { job, options, text } => (job, options, text),
        };
        if latest.load(Ordering::SeqCst) != job {
            continue;
        }
        let n_ctx = session.n_ctx();
        let result = translate_document(
            &mut session,
            &|s| model.count_tokens(s),
            n_ctx,
            &options,
            &text,
            &mut |piece| {
                if !piece.is_empty() {
                    events(Event::Token {
                        job,
                        text: piece.to_owned(),
                    })
                }
            },
            &mut |index, count| events(Event::Chunk { job, index, count }),
            &|| latest.load(Ordering::Relaxed) != job,
        );
        events(match result {
            Ok(stats) => Event::Finished { job, stats },
            Err(e) => Event::Failed {
                job,
                error: e.to_string(),
            },
        });
    }
}
