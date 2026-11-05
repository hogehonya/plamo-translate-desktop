//! Choosing the compute device for a model.

use engine::llama::{Device, DeviceKind, GpuOffload, ModelOptions};

use crate::settings::DeviceSetting;

/// The device to run on for `setting`, or `None` for the CPU.
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

/// Model options that put every layer on `device` (or run on the CPU for `None`).
pub fn model_options(device: Option<&Device>) -> ModelOptions {
    ModelOptions {
        gpu: device.map(|d| GpuOffload {
            device: d.index,
            layers: None,
        }),
    }
}

/// A short name for the device, such as `Vulkan0` or `CPU`.
pub fn device_label(device: Option<&Device>) -> String {
    device.map_or_else(|| "CPU".to_owned(), |d| d.name.clone())
}
