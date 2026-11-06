//! Choosing the compute device for a model.
//!
//! Automatic selection uses the fastest device measured on this machine, as long as the devices
//! are the ones measured. Without a measurement it takes the discrete GPU with the most free
//! memory, and otherwise the CPU. Integrated GPUs range from much slower than the CPU to much
//! faster, so a machine with one and no discrete GPU is measured on its first model load
//! ([`first_measurement`]); the settings can measure again at any time ([`candidates`]). NPUs
//! are never chosen automatically: they save power rather than time.

use engine::llama::{Device, DeviceKind, GpuOffload, ModelOptions, SessionOptions};
use engine::worker::Measurement;

use crate::settings::{self, BackendSettings, DeviceSetting, MeasuredChoice, MeasuredDevice};

/// The device to run on for `setting`, or `None` for the CPU. `measured` is the saved
/// measurement, which automatic selection follows while it holds for `devices`.
pub fn resolve<'a>(
    setting: &DeviceSetting,
    measured: Option<&MeasuredChoice>,
    devices: &'a [Device],
) -> Option<&'a Device> {
    let auto = || match measured.filter(|m| m.hardware == hardware(devices)) {
        Some(choice) => choice
            .device
            .as_ref()
            .and_then(|name| devices.iter().find(|d| &d.name == name)),
        None => devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Gpu)
            .max_by_key(|d| d.memory_free),
    };
    match setting {
        DeviceSetting::Auto => auto(),
        DeviceSetting::Cpu => None,
        // A device that disappeared (driver change, unplugged eGPU) falls back to automatic.
        DeviceSetting::Device(name) => devices.iter().find(|d| &d.name == name).or_else(auto),
    }
}

/// The devices automatic selection chooses from: the CPU (`None`) and the GPUs, discrete and
/// integrated, as ggml device indices.
pub fn candidates(devices: &[Device]) -> Vec<Option<usize>> {
    std::iter::once(None)
        .chain(
            devices
                .iter()
                .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
                .map(|d| Some(d.index)),
        )
        .collect()
}

/// Identifies the devices that [`candidates`] measures, so that a measurement is used only
/// while they stay the same.
pub fn hardware(devices: &[Device]) -> String {
    devices
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                DeviceKind::Cpu | DeviceKind::Gpu | DeviceKind::IntegratedGpu
            )
        })
        .map(|d| format!("{}: {}", d.name, d.description))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The devices to measure before loading a model with `backend`, if any: on automatic, with
/// an integrated GPU, no discrete one, and no measurement of these devices.
pub fn first_measurement(
    backend: &BackendSettings,
    devices: &[Device],
) -> Option<Vec<Option<usize>>> {
    let unmeasured = backend
        .measured
        .as_ref()
        .is_none_or(|m| m.hardware != hardware(devices));
    let integrated_only = devices.iter().any(|d| d.kind == DeviceKind::IntegratedGpu)
        && !devices.iter().any(|d| d.kind == DeviceKind::Gpu);
    (backend.device == DeviceSetting::Auto && unmeasured && integrated_only)
        .then(|| candidates(devices))
}

/// The choice to save from `results`: the device that translates a typical piece of text the
/// fastest, or the CPU if none ran.
pub fn choose(results: &[Measurement], devices: &[Device]) -> MeasuredChoice {
    let name = |device: Option<usize>| {
        device_label(device.and_then(|index| devices.iter().find(|d| d.index == index)))
    };
    let fastest = results
        .iter()
        .filter_map(|m| {
            m.result
                .as_ref()
                .ok()
                .map(|speed| (m.device, speed.typical_seconds()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(device, _)| device);
    MeasuredChoice {
        hardware: hardware(devices),
        device: fastest.map(|index| name(Some(index))),
        results: results
            .iter()
            .map(|m| MeasuredDevice {
                name: name(m.device),
                tokens_per_second: m
                    .result
                    .as_ref()
                    .ok()
                    .map(|s| s.generation_tokens_per_second),
            })
            .collect(),
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

#[cfg(test)]
mod tests {
    use engine::llama::{Device, DeviceKind};
    use engine::worker::{Measurement, Speed};

    use super::*;

    fn device(index: usize, name: &str, kind: DeviceKind, free_gb: u64) -> Device {
        Device {
            index,
            name: name.into(),
            description: format!("{name} device"),
            backend: String::new(),
            memory_total: free_gb << 30,
            memory_free: free_gb << 30,
            kind,
        }
    }

    fn speed(prompt: f64, generation: f64) -> Result<Speed, String> {
        Ok(Speed {
            prompt_tokens_per_second: prompt,
            generation_tokens_per_second: generation,
        })
    }

    fn backend(device: DeviceSetting, measured: Option<MeasuredChoice>) -> BackendSettings {
        BackendSettings { device, measured }
    }

    #[test]
    fn measures_an_integrated_gpu_without_a_discrete_one() {
        let integrated = [
            device(0, "Vulkan0", DeviceKind::IntegratedGpu, 64),
            device(1, "NPU", DeviceKind::Npu, 64),
            device(2, "CPU", DeviceKind::Cpu, 64),
        ];
        let auto = backend(DeviceSetting::Auto, None);
        // The NPU is never a candidate.
        assert_eq!(
            first_measurement(&auto, &integrated),
            Some(vec![None, Some(0)])
        );
        // Not when a device is chosen, with a discrete GPU, or without any GPU.
        assert_eq!(
            first_measurement(&backend(DeviceSetting::Cpu, None), &integrated),
            None
        );
        let discrete = [
            device(0, "Vulkan0", DeviceKind::IntegratedGpu, 64),
            device(1, "Vulkan1", DeviceKind::Gpu, 16),
            device(2, "CPU", DeviceKind::Cpu, 64),
        ];
        assert_eq!(first_measurement(&auto, &discrete), None);
        assert_eq!(first_measurement(&auto, &integrated[2..]), None);

        // The CPU wins against a weak integrated GPU, and nothing more is measured.
        let results = [
            Measurement {
                device: None,
                result: speed(170.0, 17.4),
            },
            Measurement {
                device: Some(0),
                result: speed(17.0, 6.6),
            },
        ];
        let choice = choose(&results, &integrated);
        assert_eq!(choice.device, None);
        assert_eq!(choice.results[1].name, "Vulkan0");
        assert_eq!(choice.results[1].tokens_per_second, Some(6.6));
        let measured = backend(DeviceSetting::Auto, Some(choice));
        assert_eq!(first_measurement(&measured, &integrated), None);
        assert!(
            resolve(
                &DeviceSetting::Auto,
                measured.measured.as_ref(),
                &integrated
            )
            .is_none()
        );
    }

    #[test]
    fn automatic_follows_the_measurement_while_the_devices_stay() {
        let devices = [
            device(0, "Vulkan0", DeviceKind::IntegratedGpu, 32),
            device(1, "Vulkan1", DeviceKind::Gpu, 16),
            device(2, "CPU", DeviceKind::Cpu, 64),
        ];
        let results = [
            Measurement {
                device: None,
                result: speed(170.0, 17.0),
            },
            Measurement {
                device: Some(0),
                result: speed(900.0, 80.0),
            },
            Measurement {
                device: Some(1),
                result: Err("out of memory".into()),
            },
        ];
        let choice = choose(&results, &devices);
        assert_eq!(choice.device.as_deref(), Some("Vulkan0"));
        assert_eq!(choice.results[2].tokens_per_second, None);
        let auto =
            |devices| resolve(&DeviceSetting::Auto, Some(&choice), devices).map(|d| d.name.clone());
        assert_eq!(auto(&devices[..]).as_deref(), Some("Vulkan0"));
        // Other devices: the discrete GPU with the most free memory, as without a measurement.
        let mut changed = [
            device(0, "Vulkan0", DeviceKind::Gpu, 8),
            device(1, "Vulkan1", DeviceKind::Gpu, 16),
            device(2, "CPU", DeviceKind::Cpu, 64),
        ];
        changed[0].description = "another GPU".into();
        assert_eq!(auto(&changed[..]).as_deref(), Some("Vulkan1"));
        // A chosen device that is gone falls back to automatic.
        let gone = DeviceSetting::Device("Vulkan9".into());
        assert_eq!(
            resolve(&gone, Some(&choice), &devices).unwrap().name,
            "Vulkan0"
        );
        // Nothing ran: the CPU.
        let failed = [Measurement {
            device: Some(0),
            result: Err("failed".into()),
        }];
        assert_eq!(choose(&failed, &devices).device, None);
    }
}
