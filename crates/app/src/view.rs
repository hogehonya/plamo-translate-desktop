//! The main translator window.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

use engine::StopReason;
use engine::llama::{Device, RuntimeOptions, SessionOptions};
use engine::models::{self as catalog, CatalogEntry, LocalState, ModelStore};
use engine::pfml::{LangCode, TranslationOptions};
use engine::worker::{Event, JobId, LoadRequest, Worker};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _, StyledExt as _,
    TitleBar, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use crate::backend::{self, BackendEvent, BackendPanel};
use crate::lang::{self, Choice, SourceChoice, StyleChoice, TargetChoice};
use crate::models::{ModelsEvent, ModelsPanel};
use crate::settings::{self, Settings};

actions!(translator, [CopyTranslation]);

const KEY_CONTEXT: &str = "Translator";
/// Delay between the last keystroke and an automatic translation.
const DEBOUNCE: Duration = Duration::from_millis(500);
/// How long the copy button shows its confirmation.
const COPIED_FEEDBACK: Duration = Duration::from_millis(1500);
/// Number of finished translations kept for instant reuse.
const CACHE_SIZE: usize = 64;

#[cfg(target_os = "macos")]
const COPY_TRANSLATION_KEY: &str = "cmd-shift-c";
#[cfg(not(target_os = "macos"))]
const COPY_TRANSLATION_KEY: &str = "ctrl-shift-c";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        COPY_TRANSLATION_KEY,
        CopyTranslation,
        Some(KEY_CONTEXT),
    )]);
}

type SourceSelect = SelectState<SearchableVec<Choice<SourceChoice>>>;
type TargetSelect = SelectState<SearchableVec<Choice<TargetChoice>>>;
type StyleSelect = SelectState<Vec<Choice<StyleChoice>>>;

/// One translation request: the text and every option that affects its output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    options: TranslationOptions,
    text: String,
}

/// The model to run.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelChoice {
    Catalog(&'static CatalogEntry),
    /// A GGUF file given on the command line.
    Custom(PathBuf),
}

impl ModelChoice {
    fn label(&self) -> String {
        match self {
            Self::Catalog(entry) => entry.to_string(),
            Self::Custom(path) => model_label(path),
        }
    }
}

enum Status {
    NoModel,
    Loading(f32),
    Ready,
    Translating { target: &'static str },
    Done { tokens_per_second: f64 },
    Cached,
    Truncated,
    Error(String),
}

pub struct TranslatorView {
    source: Entity<TextareaState>,
    output: Entity<TextareaState>,
    source_select: Entity<SourceSelect>,
    target_select: Entity<TargetSelect>,
    style_select: Entity<StyleSelect>,
    source_choice: SourceChoice,
    target_choice: TargetChoice,
    style_choice: StyleChoice,
    auto_translate: bool,

    worker: Worker,
    models: Entity<ModelsPanel>,
    backend: Entity<BackendPanel>,
    /// Compute devices, once llama.cpp has started.
    devices: Option<Vec<Device>>,
    /// Device label of the loaded (or loading) model.
    loaded_on: Option<String>,
    settings: Settings,
    active_model: Option<ModelChoice>,
    model_ready: bool,
    status: Status,
    /// The job whose output is streaming into the output pane.
    current_job: Option<JobId>,
    /// The request whose result is shown (or being produced) in the output pane.
    shown: Option<Request>,
    translation: String,
    cache: VecDeque<(Request, String)>,

    /// Set by the paste hook so the following change translates without debouncing.
    paste_pending: bool,
    copied: bool,
    debounce: Task<()>,
    copied_reset: Task<()>,
    _subscriptions: Vec<Subscription>,
    _events: Task<()>,
}

impl TranslatorView {
    /// `custom_model` (from the command line) takes precedence over the saved selection.
    pub fn new(
        custom_model: Option<PathBuf>,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let source = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder(t!("source.placeholder").into_owned())
        });
        let output = cx.new(|cx| TextareaState::new(window, cx));

        let source_choice = SourceChoice::Detect;
        let target_choice = TargetChoice::JapaneseEnglish;
        let style_choice = StyleChoice::Default;
        let source_select = {
            let choices = lang::source_choices();
            let selected = lang::position(&choices, &source_choice).map(IndexPath::new);
            cx.new(|cx| {
                SelectState::new(SearchableVec::new(choices), selected, window, cx).searchable(true)
            })
        };
        let target_select = {
            let choices = lang::target_choices();
            let selected = lang::position(&choices, &target_choice).map(IndexPath::new);
            cx.new(|cx| {
                SelectState::new(SearchableVec::new(choices), selected, window, cx).searchable(true)
            })
        };
        let style_select = {
            let choices = lang::style_choices();
            let selected = lang::position(&choices, &style_choice).map(IndexPath::new);
            cx.new(|cx| SelectState::new(choices, selected, window, cx))
        };

        let backend = cx.new(|cx| BackendPanel::new(settings.backend.clone(), window, cx));
        let store = ModelStore::new(settings::models_dir());
        let models = cx.new(|cx| {
            ModelsPanel::new(
                store.clone(),
                catalog::default_entry(),
                settings.license_accepted,
                cx,
            )
        });

        let subscriptions = vec![
            cx.subscribe_in(&source, window, Self::on_source_event),
            cx.subscribe_in(&models, window, Self::on_models_event),
            cx.subscribe_in(&backend, window, Self::on_backend_event),
            cx.subscribe_in(&source_select, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    this.source_choice = *choice;
                    this.options_changed(window, cx);
                }
            }),
            cx.subscribe_in(&target_select, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    this.target_choice = *choice;
                    this.options_changed(window, cx);
                }
            }),
            cx.subscribe_in(&style_select, window, |this, _, event, window, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    this.style_choice = *choice;
                    this.options_changed(window, cx);
                }
            }),
        ];

        let (tx, rx) = async_channel::unbounded();
        let worker = Worker::spawn(RuntimeOptions::default(), move |event| {
            let _ = tx.send_blocking(event);
        });
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.on_worker_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        source.update(cx, |state, cx| state.focus(window, cx));

        let initial = match custom_model {
            Some(path) => Some(ModelChoice::Custom(path)),
            None => {
                let downloaded = |e: &&CatalogEntry| store.state(e) == LocalState::Downloaded;
                settings
                    .model
                    .as_deref()
                    .and_then(catalog::find_entry)
                    .filter(downloaded)
                    .or_else(|| Some(catalog::default_entry()).filter(downloaded))
                    .or_else(|| catalog::CATALOG.iter().find(downloaded))
                    .map(ModelChoice::Catalog)
            }
        };

        let mut this = Self {
            source,
            output,
            source_select,
            target_select,
            style_select,
            source_choice,
            target_choice,
            style_choice,
            auto_translate: true,
            worker,
            models,
            backend,
            devices: None,
            loaded_on: None,
            settings,
            active_model: None,
            model_ready: false,
            status: Status::NoModel,
            current_job: None,
            shown: None,
            translation: String::new(),
            cache: VecDeque::new(),
            paste_pending: false,
            copied: false,
            debounce: Task::ready(()),
            copied_reset: Task::ready(()),
            _subscriptions: subscriptions,
            _events: events,
        };
        match initial {
            Some(choice) => this.load_model(choice, cx),
            None => cx.defer_in(window, |this, window, cx| this.open_models(window, cx)),
        }
        this
    }

    /// Makes `choice` the active model and loads it.
    fn load_model(&mut self, choice: ModelChoice, cx: &mut Context<Self>) {
        let entry = match &choice {
            ModelChoice::Catalog(entry) => Some(*entry),
            ModelChoice::Custom(_) => None,
        };
        self.models
            .update(cx, |models, cx| models.set_active(entry, cx));
        if let Some(entry) = entry {
            self.settings.model = Some(entry.id().to_owned());
            self.settings.save();
        }
        self.active_model = Some(choice);
        self.start_load(cx);
    }

    /// (Re)loads the active model with the current backend settings. Before llama.cpp has
    /// reported its devices this only marks the model as loading; the devices event retries.
    fn start_load(&mut self, cx: &mut Context<Self>) {
        let Some(choice) = self.active_model.clone() else {
            return;
        };
        self.current_job = None;
        self.shown = None;
        self.model_ready = false;
        // Cached results belong to the previous model or settings.
        self.cache.clear();
        self.status = Status::Loading(0.0);
        cx.notify();

        let path = match &choice {
            ModelChoice::Catalog(entry) => self.models.read(cx).store().path(entry),
            ModelChoice::Custom(path) => path.clone(),
        };
        if !path.is_file() {
            self.worker.unload();
            self.loaded_on = None;
            self.status = Status::Error(
                t!("status.model_missing", path = path.display().to_string()).into_owned(),
            );
            return;
        }
        let bytes = std::fs::metadata(&path).ok().map(|m| m.len());
        self.backend
            .update(cx, |backend, cx| backend.set_model_bytes(bytes, cx));
        let Some(devices) = &self.devices else {
            return;
        };
        let device = backend::resolve(&self.settings.backend.device, devices);
        self.loaded_on = Some(backend::device_label(device));
        self.worker.load(LoadRequest {
            path,
            model: backend::model_options(device),
            session: SessionOptions {
                n_ctx: self.settings.backend.n_ctx,
                ..SessionOptions::default()
            }
            .with_auto_threads(),
        });
    }

    /// Updates the recommended model for the memory of the selected device.
    fn update_recommendation(&mut self, cx: &mut Context<Self>) {
        let Some(devices) = &self.devices else {
            return;
        };
        let device = backend::resolve(&self.settings.backend.device, devices);
        let recommended = catalog::recommend(backend::available_memory(device, devices));
        self.models
            .update(cx, |models, cx| models.set_recommended(recommended, cx));
    }

    fn on_backend_event(
        &mut self,
        _: &Entity<BackendPanel>,
        event: &BackendEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let BackendEvent::Changed(backend) = event;
        self.settings.backend = backend.clone();
        self.settings.save();
        self.update_recommendation(cx);
        self.start_load(cx);
    }

    fn on_models_event(
        &mut self,
        _: &Entity<ModelsPanel>,
        event: &ModelsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ModelsEvent::Use(entry) => {
                self.load_model(ModelChoice::Catalog(entry), cx);
                if window.has_active_dialog(cx) {
                    window.close_dialog(cx);
                }
            }
            ModelsEvent::LicenseAccepted => {
                self.settings.license_accepted = true;
                self.settings.save();
            }
        }
    }

    fn open_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.models.clone();
        let backend = self.backend.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(t!("models.title").into_owned())
                .w(px(800.))
                .child(v_flex().gap_4().child(backend.clone()).child(panel.clone()))
        });
    }

    fn source_text(&self, cx: &App) -> String {
        self.source.read(cx).value().to_string()
    }

    fn on_source_event(
        &mut self,
        _: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                if std::mem::take(&mut self.paste_pending) {
                    self.translate_now(window, cx);
                } else if self.auto_translate {
                    self.schedule_translation(window, cx);
                }
                // The character count and the style selector depend on the text.
                cx.notify();
            }
            InputEvent::PressEnter {
                secondary: true, ..
            } => self.translate_now(window, cx),
            _ => {}
        }
    }

    fn options_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.auto_translate {
            self.translate_now(window, cx);
        }
        cx.notify();
    }

    fn schedule_translation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update_in(cx, |this, window, cx| this.translate_now(window, cx));
        });
    }

    fn build_request(&self, text: String) -> Request {
        let target = self.target_choice.resolve(&text);
        let mut options = TranslationOptions::new(LangCode::new(target).expect("built-in code"));
        options.source = self
            .source_choice
            .code()
            .map(|c| LangCode::new(c).expect("built-in code"));
        if options.target.is_japanese() {
            options.form = self.style_choice.form();
        }
        Request { options, text }
    }

    fn translate_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = Task::ready(());
        if !self.model_ready {
            return;
        }
        let text = self.source_text(cx);
        if text.trim().is_empty() {
            self.worker.cancel();
            self.current_job = None;
            self.shown = None;
            self.set_translation(String::new(), window, cx);
            self.status = Status::Ready;
            cx.notify();
            return;
        }

        let request = self.build_request(text);
        if self.shown.as_ref() == Some(&request) {
            return;
        }
        if let Some(hit) = self.cache.iter().find(|(r, _)| r == &request) {
            let hit = hit.1.clone();
            self.worker.cancel();
            self.current_job = None;
            self.shown = Some(request);
            self.set_translation(hit, window, cx);
            self.status = Status::Cached;
            cx.notify();
            return;
        }

        let target = self.target_choice.resolve(&request.text);
        self.set_translation(String::new(), window, cx);
        self.current_job = Some(
            self.worker
                .translate(request.options.clone(), request.text.clone()),
        );
        self.shown = Some(request);
        self.status = Status::Translating { target };
        cx.notify();
    }

    fn set_translation(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.translation = text;
        self.refresh_output(window, cx);
    }

    fn refresh_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = SharedString::from(self.translation.clone());
        self.output
            .update(cx, |state, cx| state.set_value(text, window, cx));
    }

    fn on_worker_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let is_current = |job| self.current_job == Some(job);
        match event {
            Event::Devices(devices) => {
                self.backend.update(cx, |backend, cx| {
                    backend.set_devices(devices.clone(), window, cx)
                });
                self.devices = Some(devices);
                self.update_recommendation(cx);
                if !self.model_ready && self.active_model.is_some() {
                    self.start_load(cx);
                }
            }
            Event::LoadProgress(p) => self.status = Status::Loading(p),
            Event::Loaded(_) => {
                self.model_ready = true;
                self.status = Status::Ready;
                self.translate_now(window, cx);
            }
            Event::LoadFailed(e) | Event::RuntimeFailed(e) => self.status = Status::Error(e),
            Event::Token { job, text } if is_current(job) => {
                self.translation.push_str(&text);
                self.refresh_output(window, cx);
            }
            Event::Finished { job, stats } if is_current(job) => {
                self.current_job = None;
                self.status = match stats.stop_reason {
                    StopReason::Finished => {
                        if let Some(request) = self.shown.clone() {
                            self.remember(request, self.translation.clone());
                        }
                        Status::Done {
                            tokens_per_second: stats.generation_tokens_per_second(),
                        }
                    }
                    StopReason::ContextFull => Status::Truncated,
                    // Only superseded jobs are cancelled, and those are no longer current.
                    StopReason::Cancelled => Status::Ready,
                };
            }
            Event::Failed { job, error } if is_current(job) => {
                self.current_job = None;
                self.shown = None;
                self.status = Status::Error(error);
            }
            // Output of superseded jobs.
            Event::Token { .. } | Event::Finished { .. } | Event::Failed { .. } => return,
        }
        cx.notify();
    }

    fn remember(&mut self, request: Request, translation: String) {
        if self.cache.len() == CACHE_SIZE {
            self.cache.pop_front();
        }
        self.cache.push_back((request, translation));
    }

    fn can_swap(&self) -> bool {
        matches!(
            (self.source_choice, self.target_choice),
            (SourceChoice::Lang(_), TargetChoice::Lang(_)) | (_, TargetChoice::JapaneseEnglish)
        )
    }

    /// Swaps the languages (when both are explicit) and moves the translation into the source.
    fn swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (SourceChoice::Lang(s), TargetChoice::Lang(t)) =
            (self.source_choice, self.target_choice)
        {
            self.source_choice = SourceChoice::Lang(t);
            self.target_choice = TargetChoice::Lang(s);
            let (source_choice, target_choice) = (self.source_choice, self.target_choice);
            self.source_select.update(cx, |state, cx| {
                state.set_selected_value(&source_choice, window, cx)
            });
            self.target_select.update(cx, |state, cx| {
                state.set_selected_value(&target_choice, window, cx)
            });
        }
        if !self.translation.is_empty() {
            let text = SharedString::from(self.translation.clone());
            self.source
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
        self.translate_now(window, cx);
        cx.notify();
    }

    fn copy_translation(
        &mut self,
        _: &CopyTranslation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.translation.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(self.translation.clone()));
        self.copied = true;
        self.copied_reset = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(COPIED_FEEDBACK).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        });
        cx.notify();
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.source.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        self.translate_now(window, cx);
    }

    fn status_text(&self) -> String {
        match &self.status {
            Status::NoModel => t!("status.no_model").into_owned(),
            Status::Loading(p) => {
                t!("status.loading", percent = format!("{:.0}", p * 100.0)).into_owned()
            }
            Status::Ready => t!("status.ready").into_owned(),
            Status::Translating { target } => {
                t!("status.translating", lang = lang::language_name(target)).into_owned()
            }
            Status::Done { tokens_per_second } => {
                t!("status.done", speed = format!("{tokens_per_second:.1}")).into_owned()
            }
            Status::Cached => t!("status.cached").into_owned(),
            Status::Truncated => t!("status.truncated").into_owned(),
            Status::Error(message) => t!("status.error", message = message).into_owned(),
        }
    }
}

impl Render for TranslatorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source_text = self.source_text(cx);
        let char_count = source_text.chars().count();
        let shows_style = self.target_choice.resolve(&source_text) == "ja";
        let view = cx.entity().downgrade();
        let theme = cx.theme();

        let model_button_label = self.active_model.as_ref().map_or_else(
            || t!("models.none").into_owned(),
            |model| match &self.loaded_on {
                Some(device) => format!("{} · {device}", model.label()),
                None => model.label(),
            },
        );
        let title_bar = TitleBar::new().child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(t!("app.title").into_owned()),
                )
                .child(
                    Button::new("models")
                        .ghost()
                        .xsmall()
                        .icon(IconName::HardDrive)
                        .label(model_button_label)
                        .tooltip(t!("models.title").into_owned())
                        .on_click(cx.listener(|this, _, window, cx| this.open_models(window, cx))),
                ),
        );

        let toolbar = h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .child(
                Select::new(&self.source_select)
                    .w(px(210.))
                    .menu_width(px(260.))
                    .search_placeholder(t!("search.language").into_owned()),
            )
            .child(
                Button::new("swap")
                    .ghost()
                    .icon(Lucide::ArrowLeftRight)
                    .tooltip(t!("action.swap").into_owned())
                    .disabled(!self.can_swap())
                    .on_click(cx.listener(|this, _, window, cx| this.swap(window, cx))),
            )
            .child(
                Select::new(&self.target_select)
                    .w(px(230.))
                    .menu_width(px(260.))
                    .search_placeholder(t!("search.language").into_owned()),
            )
            .when(shows_style, |this| {
                this.child(Select::new(&self.style_select).w(px(190.)))
            })
            .child(div().flex_1())
            .child(
                Switch::new("auto-translate")
                    .checked(self.auto_translate)
                    .label(t!("action.auto_translate").into_owned())
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.auto_translate = *checked;
                        if this.auto_translate {
                            this.translate_now(window, cx);
                        }
                        cx.notify();
                    })),
            )
            .child(
                Button::new("translate")
                    .primary()
                    .label(t!("action.translate").into_owned())
                    .disabled(!self.model_ready)
                    .on_click(cx.listener(|this, _, window, cx| this.translate_now(window, cx))),
            );

        let source_pane = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_1()
            .child(
                Textarea::new(&self.source)
                    .flex_1()
                    .h_full()
                    .on_paste(move |_, _, cx| {
                        let _ = view.update(cx, |this, _| this.paste_pending = true);
                        // Let the textarea insert the text as usual.
                        false
                    }),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(t!("source.chars", count = char_count).into_owned())
                    .child(
                        Button::new("clear")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .label(t!("action.clear").into_owned())
                            .disabled(source_text.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
                    ),
            );

        let output_pane = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .gap_1()
            .child(Textarea::new(&self.output).readonly(true).flex_1().h_full())
            .child(
                h_flex().justify_end().child(
                    Button::new("copy")
                        .ghost()
                        .xsmall()
                        .icon(if self.copied {
                            IconName::Check
                        } else {
                            IconName::Copy
                        })
                        .label(if self.copied {
                            t!("action.copied").into_owned()
                        } else {
                            t!("action.copy").into_owned()
                        })
                        .disabled(self.translation.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.copy_translation(&CopyTranslation, window, cx)
                        })),
                ),
            );

        let status_bar = h_flex()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(self.status_text());

        v_flex()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::copy_translation))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(title_bar)
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .pb_2()
                    .gap_3()
                    .child(source_pane)
                    .child(output_pane),
            )
            .child(status_bar)
    }
}

/// A short label such as `2B · Q4_K_M` for the standard file names, else the file stem.
fn model_label(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem
        .strip_prefix("plamo-3-translate-")
        .and_then(|rest| rest.split_once('-'))
    {
        Some((size, quant)) => format!("{} · {quant}", size.to_uppercase()),
        None => stem,
    }
}

#[cfg(test)]
mod tests {
    // Import explicitly: `gpui_kit::*` exports a `test` macro that would shadow `#[test]`.
    use std::path::Path;

    use super::model_label;

    #[test]
    fn labels_standard_model_files() {
        assert_eq!(
            model_label(Path::new("/m/plamo-3-translate-2b-Q4_K_M.gguf")),
            "2B · Q4_K_M"
        );
        assert_eq!(
            model_label(Path::new("plamo-3-translate-0.8b-Q2_K.gguf")),
            "0.8B · Q2_K"
        );
        assert_eq!(model_label(Path::new("custom.gguf")), "custom");
    }
}
