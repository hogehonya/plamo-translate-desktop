//! Inference device and context settings.

use engine::llama::{Device, DeviceKind, GpuOffload, ModelOptions};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rust_i18n::t;

use crate::lang::{self, Choice};
use crate::models::format_bytes;
use crate::settings::{BackendSettings, DeviceSetting};

/// Context lengths offered in the UI. The model was trained with up to 32,768 tokens.
pub const CONTEXT_LENGTHS: [u32; 4] = [4096, 8192, 16384, 32768];

/// The device a setting resolves to on this machine. `None` means the CPU.
pub fn resolve<'a>(setting: &DeviceSetting, devices: &'a [Device]) -> Option<&'a Device> {
    let best_gpu = || {
        devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Gpu)
            .max_by_key(|d| d.memory_free)
    };
    match setting {
        DeviceSetting::Auto => best_gpu(),
        DeviceSetting::Cpu => None,
        // A device that disappeared (driver change, unplugged eGPU) falls back to automatic.
        DeviceSetting::Device(name) => devices.iter().find(|d| &d.name == name).or_else(best_gpu),
    }
}

pub fn model_options(device: Option<&Device>) -> ModelOptions {
    ModelOptions {
        gpu: device.map(|d| GpuOffload {
            device: d.index,
            layers: None,
        }),
    }
}

/// Memory available to the model on `device`, or system memory for the CPU.
pub fn available_memory(device: Option<&Device>, devices: &[Device]) -> u64 {
    match device {
        Some(d) => d.memory_free,
        None => devices
            .iter()
            .find(|d| d.kind == DeviceKind::Cpu)
            .map_or(0, |d| d.memory_free),
    }
}

/// Rough memory needed to run a model file: weights, compute buffers and the KV cache.
/// PLaMo 3 uses sliding-window attention on most layers, so the cache stays small.
pub fn estimate_required(model_bytes: u64, n_ctx: u32) -> u64 {
    model_bytes + model_bytes / 5 + u64::from(n_ctx) * 32 * 1024 + (256 << 20)
}

pub fn device_label(device: Option<&Device>) -> String {
    device.map_or_else(|| "CPU".to_owned(), |d| d.name.clone())
}

pub enum BackendEvent {
    Changed(BackendSettings),
}

type DeviceSelect = SelectState<Vec<Choice<DeviceSetting>>>;
type ContextSelect = SelectState<Vec<Choice<u32>>>;

pub struct BackendPanel {
    settings: BackendSettings,
    devices: Vec<Device>,
    /// Size of the active model file, for the memory check.
    model_bytes: Option<u64>,
    device_select: Entity<DeviceSelect>,
    context_select: Entity<ContextSelect>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BackendEvent> for BackendPanel {}

impl BackendPanel {
    pub fn new(settings: BackendSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let device_choices = device_choices(&[]);
        let selected = lang::position(&device_choices, &settings.device).map(IndexPath::new);
        let device_select = cx.new(|cx| {
            SelectState::new(
                device_choices,
                selected.or(Some(IndexPath::new(0))),
                window,
                cx,
            )
        });

        let context_choices: Vec<_> = CONTEXT_LENGTHS
            .iter()
            .map(|&n| Choice::new(n, t!("backend.tokens", count = n).into_owned(), &[]))
            .collect();
        let selected = lang::position(&context_choices, &settings.n_ctx).map(IndexPath::new);
        let context_select = cx.new(|cx| SelectState::new(context_choices, selected, window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&device_select, window, |this, _, event, _, cx| {
                if let SelectEvent::Confirm(Some(device)) = event
                    && *device != this.settings.device
                {
                    this.settings.device = device.clone();
                    cx.emit(BackendEvent::Changed(this.settings.clone()));
                    cx.notify();
                }
            }),
            cx.subscribe_in(&context_select, window, |this, _, event, _, cx| {
                if let SelectEvent::Confirm(Some(n_ctx)) = event
                    && *n_ctx != this.settings.n_ctx
                {
                    this.settings.n_ctx = *n_ctx;
                    cx.emit(BackendEvent::Changed(this.settings.clone()));
                    cx.notify();
                }
            }),
        ];

        Self {
            settings,
            devices: Vec::new(),
            model_bytes: None,
            device_select,
            context_select,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_devices(
        &mut self,
        devices: Vec<Device>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let choices = device_choices(&devices);
        let current = self.settings.device.clone();
        let known = lang::position(&choices, &current).is_some();
        self.devices = devices;
        self.device_select.update(cx, |state, cx| {
            state.set_items(choices, window, cx);
            state.set_selected_value(
                if known {
                    &current
                } else {
                    &DeviceSetting::Auto
                },
                window,
                cx,
            );
        });
        cx.notify();
    }

    pub fn set_model_bytes(&mut self, bytes: Option<u64>, cx: &mut Context<Self>) {
        self.model_bytes = bytes;
        cx.notify();
    }
}

fn device_choices(devices: &[Device]) -> Vec<Choice<DeviceSetting>> {
    let auto_target = resolve(&DeviceSetting::Auto, devices);
    let mut choices = vec![
        Choice::new(
            DeviceSetting::Auto,
            t!("backend.auto", device = device_label(auto_target)).into_owned(),
            &[],
        ),
        Choice::new(DeviceSetting::Cpu, t!("backend.cpu").into_owned(), &[]),
    ];
    choices.extend(
        devices
            .iter()
            .filter(|d| d.kind != DeviceKind::Cpu)
            .map(|d| {
                Choice::new(
                    DeviceSetting::Device(d.name.clone()),
                    format!(
                        "{} — {} ({} free)",
                        d.name,
                        d.description,
                        format_bytes(d.memory_free)
                    ),
                    &[],
                )
            }),
    );
    choices
}

impl Render for BackendPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (muted, warning) = {
            let theme = cx.theme();
            (theme.muted_foreground, theme.warning)
        };
        let device = resolve(&self.settings.device, &self.devices);
        let available = available_memory(device, &self.devices);
        let memory_warning = self.model_bytes.and_then(|bytes| {
            let required = estimate_required(bytes, self.settings.n_ctx);
            (!self.devices.is_empty() && required > available).then(|| {
                t!(
                    "backend.memory_warning",
                    device = device_label(device),
                    required = format_bytes(required),
                    available = format_bytes(available)
                )
                .into_owned()
            })
        });

        v_flex()
            .gap_2()
            .text_sm()
            .child(
                h_flex()
                    .gap_3()
                    .child(div().w(px(110.)).child(t!("backend.device").into_owned()))
                    .child(
                        Select::new(&self.device_select)
                            .w(px(420.))
                            .menu_width(px(480.)),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(div().w(px(110.)).child(t!("backend.context").into_owned()))
                    .child(Select::new(&self.context_select).w(px(180.)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("backend.context_hint").into_owned()),
                    ),
            )
            .when_some(memory_warning, |this, text| {
                this.child(div().text_xs().text_color(warning).child(text))
            })
    }
}

#[cfg(test)]
mod tests {
    use engine::llama::{Device, DeviceKind};

    use super::{DeviceSetting, estimate_required, resolve};

    fn device(index: usize, name: &str, kind: DeviceKind, free_gb: u64) -> Device {
        Device {
            index,
            name: name.into(),
            description: String::new(),
            backend: String::new(),
            memory_total: free_gb << 30,
            memory_free: free_gb << 30,
            kind,
        }
    }

    #[test]
    fn auto_prefers_the_largest_discrete_gpu() {
        let devices = [
            device(0, "CPU", DeviceKind::Cpu, 64),
            device(1, "Vulkan0", DeviceKind::Gpu, 16),
            device(2, "Vulkan1", DeviceKind::IntegratedGpu, 32),
        ];
        assert_eq!(
            resolve(&DeviceSetting::Auto, &devices).unwrap().name,
            "Vulkan0"
        );
        assert!(resolve(&DeviceSetting::Cpu, &devices).is_none());
        let igpu = DeviceSetting::Device("Vulkan1".into());
        assert_eq!(resolve(&igpu, &devices).unwrap().name, "Vulkan1");
        let gone = DeviceSetting::Device("CUDA0".into());
        assert_eq!(resolve(&gone, &devices).unwrap().name, "Vulkan0");
        // Without a discrete GPU, automatic means the CPU.
        assert!(resolve(&DeviceSetting::Auto, &devices[..1]).is_none());
    }

    #[test]
    fn estimate_grows_with_model_and_context() {
        let small = estimate_required(1 << 30, 4096);
        assert!(small > 1 << 30);
        assert!(estimate_required(1 << 30, 32768) > small);
    }
}
