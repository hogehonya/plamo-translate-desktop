//! The model manager: catalog, downloads and the license gate.

use std::collections::HashMap;
use std::time::Duration;

use engine::download::{DownloadEvent, Downloads, Progress};
use engine::models::{CATALOG, CatalogEntry, LocalState, ModelStore};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::progress::Progress as ProgressBar;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

pub const LICENSE_URL_EN: &str = "https://plamo.preferredai.jp/info/plamo-community-license-en";
pub const LICENSE_URL_JA: &str = "https://plamo.preferredai.jp/info/plamo-community-license-ja";

pub enum ModelsEvent {
    /// The user picked a downloaded model, or the download they started first finished.
    Use(&'static CatalogEntry),
    /// Release the active model.
    Unload,
    /// Delete the active model. The owner unloads it first, then calls [`ModelsPanel::delete`].
    DeleteActive(&'static CatalogEntry),
    LicenseAccepted,
}

pub struct ModelsPanel {
    downloads: Downloads,
    progress: HashMap<&'static str, Progress>,
    errors: HashMap<&'static str, String>,
    active: Option<&'static str>,
    recommended: &'static CatalogEntry,
    license_accepted: bool,
    /// A download to switch to as soon as it finishes (set when no model is active).
    use_when_done: Option<&'static str>,
    _events: Task<()>,
}

impl EventEmitter<ModelsEvent> for ModelsPanel {}

impl ModelsPanel {
    pub fn new(
        store: ModelStore,
        recommended: &'static CatalogEntry,
        license_accepted: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = async_channel::unbounded();
        let downloads = Downloads::new(store, move |event| {
            let _ = tx.send_blocking(event);
        });
        let events = cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| this.on_download_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            downloads,
            progress: HashMap::new(),
            errors: HashMap::new(),
            active: None,
            recommended,
            license_accepted,
            use_when_done: None,
            _events: events,
        }
    }

    pub fn store(&self) -> &ModelStore {
        self.downloads.store()
    }

    pub fn set_recommended(&mut self, entry: &'static CatalogEntry, cx: &mut Context<Self>) {
        self.recommended = entry;
        cx.notify();
    }

    pub fn set_active(&mut self, entry: Option<&'static CatalogEntry>, cx: &mut Context<Self>) {
        self.active = entry.map(CatalogEntry::id);
        cx.notify();
    }

    fn on_download_event(&mut self, event: DownloadEvent, cx: &mut Context<Self>) {
        match event {
            DownloadEvent::Progress { entry, progress } => {
                self.progress.insert(entry.id(), progress);
            }
            DownloadEvent::Finished { entry, .. } => {
                self.progress.remove(entry.id());
                if self.use_when_done == Some(entry.id()) {
                    self.use_when_done = None;
                    cx.emit(ModelsEvent::Use(entry));
                }
            }
            DownloadEvent::Cancelled { entry } => {
                self.progress.remove(entry.id());
            }
            DownloadEvent::Failed { entry, error } => {
                self.progress.remove(entry.id());
                self.errors.insert(entry.id(), error);
            }
        }
        cx.notify();
    }

    fn start(&mut self, entry: &'static CatalogEntry, cx: &mut Context<Self>) {
        if !self.license_accepted {
            return;
        }
        self.errors.remove(entry.id());
        let downloaded = match self.store().state(entry) {
            LocalState::Partial(n) => n,
            _ => 0,
        };
        self.progress.insert(
            entry.id(),
            Progress {
                downloaded,
                total: entry.bytes,
                bytes_per_second: 0.0,
            },
        );
        if self.active.is_none() && self.use_when_done.is_none() {
            self.use_when_done = Some(entry.id());
        }
        self.downloads.start(entry);
        cx.notify();
    }

    fn cancel(&mut self, entry: &'static CatalogEntry, cx: &mut Context<Self>) {
        self.downloads.cancel(entry);
        if self.use_when_done == Some(entry.id()) {
            self.use_when_done = None;
        }
        cx.notify();
    }

    fn confirm_delete(
        &mut self,
        entry: &'static CatalogEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            alert
                .title(t!("models.delete_title").into_owned())
                .description(
                    t!(
                        "models.delete_body",
                        model = entry.to_string(),
                        size = format_bytes(entry.bytes)
                    )
                    .into_owned(),
                )
                .show_cancel(true)
                .ok_text(t!("models.delete").into_owned())
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(t!("models.cancel").into_owned())
                .on_ok(move |_, _, cx| {
                    let _ = panel.update(cx, |this, cx| {
                        if this.active == Some(entry.id()) {
                            cx.emit(ModelsEvent::DeleteActive(entry));
                        } else {
                            this.delete(entry, cx);
                        }
                    });
                    true
                })
        });
    }

    /// Deletes the files of a model that is not loaded.
    pub fn delete(&mut self, entry: &'static CatalogEntry, cx: &mut Context<Self>) {
        if let Err(e) = self.store().delete(entry) {
            self.errors.insert(entry.id(), e.to_string());
        } else {
            self.errors.remove(entry.id());
        }
        cx.notify();
    }

    fn render_row(&self, entry: &'static CatalogEntry, cx: &mut Context<Self>) -> AnyElement {
        let (danger, secondary, muted) = {
            let theme = cx.theme();
            (theme.danger, theme.secondary, theme.muted_foreground)
        };
        let id = entry.id();
        let state = self.store().state(entry);
        let progress = self.progress.get(id).copied();
        let error = self.errors.get(id).cloned();
        let is_active = self.active == Some(id);
        let can_download = self.license_accepted;

        let status: AnyElement = if let Some(p) = progress {
            h_flex()
                .gap_2()
                .child(
                    ProgressBar::new(SharedString::from(format!("progress-{id}")))
                        .value(p.fraction() * 100.0)
                        .w(px(120.)),
                )
                .child(progress_text(&p))
                .into_any_element()
        } else if let Some(error) = &error {
            div()
                .text_color(danger)
                .child(t!("models.failed", message = error).into_owned())
                .into_any_element()
        } else {
            match state {
                LocalState::Downloaded if is_active => Tag::primary()
                    .small()
                    .child(t!("models.in_use").into_owned())
                    .into_any_element(),
                LocalState::Downloaded => div()
                    .child(t!("models.downloaded").into_owned())
                    .into_any_element(),
                LocalState::Partial(n) => div()
                    .child(
                        t!(
                            "models.paused",
                            percent = format!("{:.0}", n as f64 / entry.bytes as f64 * 100.0)
                        )
                        .into_owned(),
                    )
                    .into_any_element(),
                LocalState::Missing => div().into_any_element(),
            }
        };

        let button = |name: &str| Button::new(SharedString::from(format!("{name}-{id}"))).xsmall();
        let actions = h_flex().gap_1().justify_end().map(|this| {
            if progress.is_some() {
                this.child(
                    button("cancel")
                        .label(t!("models.cancel").into_owned())
                        .on_click(cx.listener(move |this, _, _, cx| this.cancel(entry, cx))),
                )
            } else {
                match state {
                    LocalState::Downloaded => this
                        .map(|this| {
                            if is_active {
                                this.child(
                                    button("unload")
                                        .label(t!("models.unload").into_owned())
                                        .on_click(
                                            cx.listener(|_, _, _, cx| cx.emit(ModelsEvent::Unload)),
                                        ),
                                )
                            } else {
                                this.child(
                                    button("use")
                                        .primary()
                                        .label(t!("models.use").into_owned())
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(ModelsEvent::Use(entry))
                                        })),
                                )
                            }
                        })
                        .child(
                            button("delete")
                                .ghost()
                                .label(t!("models.delete").into_owned())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_delete(entry, window, cx)
                                })),
                        ),
                    LocalState::Partial(_) => this
                        .child(
                            button("resume")
                                .primary()
                                .label(t!("models.resume").into_owned())
                                .disabled(!can_download)
                                .on_click(cx.listener(move |this, _, _, cx| this.start(entry, cx))),
                        )
                        .child(
                            button("delete")
                                .ghost()
                                .label(t!("models.delete").into_owned())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_delete(entry, window, cx)
                                })),
                        ),
                    LocalState::Missing => this.child(
                        button("download")
                            .label(
                                if error.is_some() {
                                    t!("models.retry")
                                } else {
                                    t!("models.download")
                                }
                                .into_owned(),
                            )
                            .disabled(!can_download)
                            .on_click(cx.listener(move |this, _, _, cx| this.start(entry, cx))),
                    ),
                }
            }
        });

        h_flex()
            .id(SharedString::from(format!("row-{id}")))
            .gap_3()
            .px_2()
            .py_1p5()
            .rounded_md()
            .when(is_active, |this| this.bg(secondary))
            .child(
                h_flex()
                    .w(px(190.))
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(entry.to_string()),
                    )
                    .when(std::ptr::eq(entry, self.recommended), |this| {
                        this.child(
                            Tag::secondary()
                                .small()
                                .child(t!("models.recommended").into_owned()),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(70.))
                    .text_color(muted)
                    .child(format_bytes(entry.bytes)),
            )
            .child(div().flex_1().min_w_0().text_sm().child(status))
            .child(actions)
            .into_any_element()
    }
}

impl Render for ModelsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = CATALOG.iter().map(|e| self.render_row(e, cx)).collect();
        let (border, muted) = {
            let theme = cx.theme();
            (theme.border, theme.muted_foreground)
        };
        let license_url = if rust_i18n::locale().starts_with("ja") {
            LICENSE_URL_JA
        } else {
            LICENSE_URL_EN
        };
        let models_dir = self.store().dir().to_path_buf();

        v_flex()
            .gap_3()
            .text_sm()
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Built with PLaMo"),
                    )
                    .child(t!("models.license_notice").into_owned())
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Checkbox::new("accept-license")
                                    .checked(self.license_accepted)
                                    .disabled(self.license_accepted)
                                    .label(t!("models.license_agree").into_owned())
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        if *checked && !this.license_accepted {
                                            this.license_accepted = true;
                                            cx.emit(ModelsEvent::LicenseAccepted);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                Button::new("license-link")
                                    .link()
                                    .xsmall()
                                    .label(t!("models.license_link").into_owned())
                                    .on_click(move |_, _, cx| cx.open_url(license_url)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("model-list")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .child(v_flex().gap_0p5().children(rows)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(
                        t!("models.folder", path = models_dir.display().to_string()).into_owned(),
                    )
                    .child(
                        Button::new("open-folder")
                            .ghost()
                            .xsmall()
                            .label(t!("models.open_folder").into_owned())
                            .on_click(move |_, _, cx| {
                                let _ = std::fs::create_dir_all(&models_dir);
                                cx.reveal_path(&models_dir);
                            }),
                    ),
            )
    }
}

fn progress_text(p: &Progress) -> String {
    let mut text = format!(
        "{:.0}% · {} / {}",
        p.fraction() * 100.0,
        format_bytes(p.downloaded),
        format_bytes(p.total)
    );
    if p.bytes_per_second > 0.0 {
        text.push_str(&format!(" · {}/s", format_bytes(p.bytes_per_second as u64)));
    }
    if let Some(eta) = p.eta() {
        text.push_str(&format!(" · {}", format_duration(eta)));
    }
    text
}

pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1e6;
    const GB: f64 = 1e9;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{format_bytes, format_duration};

    #[test]
    fn formatting() {
        assert_eq!(format_bytes(1_807_192_480), "1.81 GB");
        assert_eq!(format_bytes(573_116_288), "573 MB");
        assert_eq!(format_duration(Duration::from_secs(75)), "1:15");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1:02:05");
    }
}
