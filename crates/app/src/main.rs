//! PLaMo Translate Desktop.
//!
//! M0 prototype: two panes, automatic translation as you type, and a fixed model path.
//! The model path comes from the first argument, then `PLAMO_MODEL`, then the default models
//! directory.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::Duration;

use engine::llama::{ModelOptions, RuntimeOptions, SessionOptions};
use engine::pfml::{LangCode, TranslationOptions};
use engine::worker::{Event, JobId, LoadRequest, Worker};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;

const DEFAULT_MODEL: &str = "plamo-3-translate-2b-Q4_K_M.gguf";
const DEBOUNCE: Duration = Duration::from_millis(400);

fn model_path() -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .or_else(|| std::env::var_os("PLAMO_MODEL"))
        .map(PathBuf::from)
        .or_else(|| {
            directories::ProjectDirs::from("", "", "plamo-translate-desktop")
                .map(|dirs| dirs.data_local_dir().join("models").join(DEFAULT_MODEL))
        })
}

enum Status {
    Loading(f32),
    Ready,
    Translating,
    Done { tokens_per_second: f64 },
    Error(String),
}

struct TranslatorView {
    source: Entity<TextareaState>,
    output: Entity<TextareaState>,
    worker: Worker,
    status: Status,
    model_loaded: bool,
    job: Option<JobId>,
    translation: String,
    target: &'static str,
    debounce: Task<()>,
    _subscription: Subscription,
    _events: Task<()>,
}

impl TranslatorView {
    fn new(model: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("Type or paste text to translate")
        });
        let output = cx.new(|cx| TextareaState::new(window, cx));
        let subscription = cx.subscribe_in(&source, window, |this, _, event, window, cx| {
            if let InputEvent::Change = event {
                this.schedule_translation(window, cx);
            }
        });

        let (tx, rx) = async_channel::unbounded();
        let worker = Worker::spawn(RuntimeOptions::default(), move |event| {
            let _ = tx.send_blocking(event);
        });
        worker.load(LoadRequest {
            path: model,
            model: ModelOptions::default(),
            session: SessionOptions::default(),
        });
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let updated =
                    this.update_in(cx, |this, window, cx| this.on_event(event, window, cx));
                if updated.is_err() {
                    break;
                }
            }
        });

        source.update(cx, |state, cx| state.focus(window, cx));

        Self {
            source,
            output,
            worker,
            status: Status::Loading(0.0),
            model_loaded: false,
            job: None,
            translation: String::new(),
            target: "ja",
            debounce: Task::ready(()),
            _subscription: subscription,
            _events: events,
        }
    }

    fn schedule_translation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update_in(cx, |this, window, cx| this.translate(window, cx));
        });
    }

    fn translate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model_loaded {
            return;
        }
        let text = self.source.read(cx).value().to_string();
        self.translation.clear();
        self.show_translation(window, cx);
        if text.trim().is_empty() {
            self.worker.cancel();
            self.job = None;
            self.status = Status::Ready;
            cx.notify();
            return;
        }
        // Prototype heuristic until the language selector exists.
        self.target = if contains_japanese(&text) { "en" } else { "ja" };
        let options = TranslationOptions::new(LangCode::new(self.target).expect("valid code"));
        self.job = Some(self.worker.translate(options, text));
        self.status = Status::Translating;
        cx.notify();
    }

    fn show_translation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = SharedString::from(self.translation.clone());
        self.output
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    fn on_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::LoadProgress(p) => self.status = Status::Loading(p),
            Event::Loaded(_) => {
                self.model_loaded = true;
                self.status = Status::Ready;
                self.translate(window, cx);
            }
            Event::LoadFailed(e) | Event::RuntimeFailed(e) => self.status = Status::Error(e),
            Event::Token { job, text } if Some(job) == self.job => {
                self.translation.push_str(&text);
                self.show_translation(window, cx);
            }
            Event::Finished { job, stats } if Some(job) == self.job => {
                self.status = Status::Done {
                    tokens_per_second: stats.generation_tokens_per_second(),
                };
            }
            Event::Failed { job, error } if Some(job) == self.job => {
                self.status = Status::Error(error);
            }
            // Output from superseded jobs.
            Event::Token { .. } | Event::Finished { .. } | Event::Failed { .. } => return,
        }
        cx.notify();
    }

    fn status_text(&self) -> String {
        match &self.status {
            Status::Loading(p) => format!("Loading model… {:.0}%", p * 100.0),
            Status::Ready => "Ready".into(),
            Status::Translating => format!("Translating into {}…", self.target),
            Status::Done { tokens_per_second } => {
                format!("Done · {tokens_per_second:.1} tok/s")
            }
            Status::Error(e) => format!("Error: {e}"),
        }
    }
}

impl Render for TranslatorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .child(Textarea::new(&self.source).flex_1().h_full())
                    .child(Textarea::new(&self.output).readonly(true).flex_1().h_full()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.status_text()),
            )
    }
}

fn contains_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{3040}'..='\u{309f}' // Hiragana
            | '\u{30a0}'..='\u{30ff}' // Katakana
            | '\u{4e00}'..='\u{9fff}') // CJK unified ideographs
    })
}

fn main() {
    let Some(model) = model_path() else {
        eprintln!("cannot determine the model path; pass it as the first argument");
        std::process::exit(2);
    };
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("PLaMo Translate".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.), px(700.)),
                cx,
            ))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| TranslatorView::new(model, window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
