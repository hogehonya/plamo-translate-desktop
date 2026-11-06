//! Choosing the compute device for a model.

use engine::llama::{Device, DeviceKind, GpuOffload, ModelOptions, SessionOptions};

use crate::settings::{self, DeviceSetting};

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

/// Session options for a model on `device` (or the CPU for `None`).
pub fn session_options(device: Option<&Device>) -> SessionOptions {
    let options = SessionOptions::default().with_auto_threads();
    if device.is_some_and(|d| d.kind == DeviceKind::Npu) {
        options.for_npu()
    } else {
        options
    }
}

/// A short name for the device, such as `Vulkan0` or `CPU`.
pub fn device_label(device: Option<&Device>) -> String {
    device.map_or_else(|| "CPU".to_owned(), |d| d.name.clone())
}

/// The OpenVINO backend module, which only packages for Intel NPUs ship.
const OPENVINO_MODULE: &str = if cfg!(windows) {
    "ggml-openvino.dll"
} else {
    "libggml-openvino.so"
};

/// Gives the OpenVINO backend, when it ships next to the executable, a folder to cache its
/// compiled graphs in (`openvino-cache` in the data directory), so an NPU compiles a model's
/// graphs on the first translation only, not at every start. A value set beforehand is kept;
/// an empty one turns the cache off.
///
/// # Safety
///
/// Sets an environment variable, which the backend reads when it loads: call this first thing
/// in `main`, while no other thread exists.
pub unsafe fn configure_openvino() {
    const CACHE: &str = "GGML_OPENVINO_CACHE_DIR";
    let shipped = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(OPENVINO_MODULE)))
        .is_some_and(|module| module.is_file());
    if shipped && std::env::var_os(CACHE).is_none() {
        let dir = settings::data_file("openvino-cache");
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: the caller guarantees that no other thread exists.
        unsafe { std::env::set_var(CACHE, dir) };
    }
}
