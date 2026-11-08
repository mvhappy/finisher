//! The GPU device (spec §5.2): the adapter `FIN_GPU` picks, the device and queue, and how
//! wgpu failures become error codes: no adapter → `GPU_UNAVAILABLE`, a lost device →
//! `GPU_DEVICE_LOST`, an allocation failure → `GPU_OUT_OF_MEMORY`, anything else (a
//! validation or internal error, which is a bug) → `INTERNAL`.

use std::sync::{Arc, Mutex, PoisonError};

use fin_core::CoreError;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Which adapter to use: `FIN_GPU=auto|software|<backend>` (`metal`, `dx12`, `vulkan`,
/// `webgpu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterChoice {
    /// The high-performance adapter of the primary backends, a software one if that is all.
    Auto,
    /// A CPU adapter only: lavapipe (Vulkan) on Linux, WARP (D3D12) on Windows. macOS has
    /// none.
    Software,
    /// Any adapter of these backends.
    Backend(wgpu::Backends),
}

impl AdapterChoice {
    /// Parses a `FIN_GPU` value; an unknown one is `GPU_UNAVAILABLE`.
    pub fn parse(value: &str) -> Result<AdapterChoice, CoreError> {
        Ok(match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => AdapterChoice::Auto,
            "software" => AdapterChoice::Software,
            "metal" => AdapterChoice::Backend(wgpu::Backends::METAL),
            "dx12" | "d3d12" => AdapterChoice::Backend(wgpu::Backends::DX12),
            "vulkan" => AdapterChoice::Backend(wgpu::Backends::VULKAN),
            "webgpu" => AdapterChoice::Backend(wgpu::Backends::BROWSER_WEBGPU),
            _ => {
                return Err(CoreError::new(
                    "GPU_UNAVAILABLE",
                    format!("FIN_GPU={value} names no adapter choice"),
                )
                .with_details(json!({ "FIN_GPU": value, "reason": "unknownChoice" }))
                .with_fix("set FIN_GPU to auto, software, metal, dx12, vulkan or webgpu"));
            }
        })
    }

    /// The choice in `FIN_GPU` (`Auto` when unset).
    pub fn from_env() -> Result<AdapterChoice, CoreError> {
        AdapterChoice::parse(&std::env::var("FIN_GPU").unwrap_or_default())
    }
}

/// What a render ran on (recorded as numeric ids, spec §21 decision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterSummary {
    pub backend: String,
    pub device_type: String,
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    /// A CPU adapter (lavapipe, WARP): slow but correct.
    pub software: bool,
}

/// A device and queue on one adapter. Uncaptured wgpu errors and device loss are recorded,
/// never a panic; the compositor reports them at the end of the frame they happen in.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    summary: AdapterSummary,
    lost: Arc<Mutex<Option<String>>>,
    uncaptured: Arc<Mutex<Option<CoreError>>>,
}

fn unavailable(choice: AdapterChoice, reason: &str, message: String) -> CoreError {
    CoreError::new("GPU_UNAVAILABLE", message)
        .with_details(json!({ "choice": format!("{choice:?}"), "reason": reason }))
        .with_fix("install a GPU driver, or Mesa's lavapipe (Linux) / WARP (Windows) for software rendering")
}

impl Gpu {
    /// Opens the adapter `choice` names (blocking).
    pub fn new(choice: AdapterChoice) -> Result<Gpu, CoreError> {
        pollster::block_on(Gpu::open(choice))
    }

    /// Opens the adapter `choice` names.
    pub async fn open(choice: AdapterChoice) -> Result<Gpu, CoreError> {
        let backends = match choice {
            AdapterChoice::Backend(b) => b,
            AdapterChoice::Auto | AdapterChoice::Software => wgpu::Backends::PRIMARY,
        };
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends;
        desc.flags = wgpu::InstanceFlags::empty();
        // FXC (part of Windows) compiles D3D12 shaders; DXC would be a shipped DLL (spec §5.2).
        desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: choice == AdapterChoice::Software,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| unavailable(choice, "noAdapter", format!("no GPU adapter: {e}")))?;
        let info = adapter.get_info();
        let software = info.device_type == wgpu::DeviceType::Cpu;
        if choice == AdapterChoice::Software && !software {
            return Err(unavailable(
                choice,
                "noSoftwareAdapter",
                format!(
                    "no software adapter (got {} {:?})",
                    info.name, info.device_type
                ),
            ));
        }
        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("fin-compositor"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|e| {
                unavailable(
                    choice,
                    "noDevice",
                    format!("the adapter gave no device: {e}"),
                )
            })?;
        let lost = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&lost);
        device.set_device_lost_callback(move |reason, message| {
            *sink.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(format!("{reason:?}: {message}"));
        });
        let uncaptured = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&uncaptured);
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert(map_error(&e));
        }));
        let summary = AdapterSummary {
            backend: format!("{:?}", info.backend).to_ascii_lowercase(),
            device_type: format!("{:?}", info.device_type),
            name: info.name.clone(),
            vendor: info.vendor,
            device: info.device,
            software,
        };
        Ok(Gpu {
            device,
            queue,
            summary,
            lost,
            uncaptured,
        })
    }

    pub fn info(&self) -> AdapterSummary {
        self.summary.clone()
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// `GPU_DEVICE_LOST` once the device is gone. Polls first (without blocking), which runs
    /// a pending device-lost callback.
    pub(crate) fn check_lost(&self) -> Result<(), CoreError> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        match &*self.lost.lock().unwrap_or_else(PoisonError::into_inner) {
            None => Ok(()),
            Some(reason) => Err(device_lost(reason)),
        }
    }

    /// The first error no scope caught since the last call (a lost device first).
    pub(crate) fn take_uncaptured(&self) -> Result<(), CoreError> {
        self.check_lost()?;
        match self
            .uncaptured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            None => Ok(()),
            Some(e) => Err(e),
        }
    }
}

pub(crate) fn device_lost(reason: &str) -> CoreError {
    CoreError::new(
        "GPU_DEVICE_LOST",
        format!("the GPU device was lost ({reason})"),
    )
    .with_details(json!({ "reason": reason }))
}

/// A wgpu error as an error code.
pub fn map_error(e: &wgpu::Error) -> CoreError {
    match e {
        wgpu::Error::OutOfMemory { .. } => CoreError::new(
            "GPU_OUT_OF_MEMORY",
            format!("the GPU ran out of memory: {e}"),
        )
        .with_details(json!({ "error": e.to_string() })),
        wgpu::Error::Validation { description, .. } => {
            CoreError::new("INTERNAL", format!("GPU validation error: {description}"))
                .with_details(json!({ "gpu": "validation", "error": description }))
        }
        wgpu::Error::Internal { description, .. } => {
            CoreError::new("INTERNAL", format!("GPU internal error: {description}"))
                .with_details(json!({ "gpu": "internal", "error": description }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fin_gpu_values() {
        assert_eq!(AdapterChoice::parse("").unwrap(), AdapterChoice::Auto);
        assert_eq!(
            AdapterChoice::parse(" Software ").unwrap(),
            AdapterChoice::Software
        );
        assert_eq!(
            AdapterChoice::parse("metal").unwrap(),
            AdapterChoice::Backend(wgpu::Backends::METAL)
        );
        assert_eq!(
            AdapterChoice::parse("d3d12").unwrap(),
            AdapterChoice::Backend(wgpu::Backends::DX12)
        );
        assert_eq!(
            AdapterChoice::parse("cuda").unwrap_err().code,
            "GPU_UNAVAILABLE"
        );
    }

    #[test]
    fn allocation_failures_and_bugs_map_to_their_codes() {
        let oom = wgpu::Error::OutOfMemory {
            source: Box::new(std::io::Error::other("no VRAM")),
        };
        assert_eq!(map_error(&oom).code, "GPU_OUT_OF_MEMORY");
        let invalid = wgpu::Error::Validation {
            source: Box::new(std::io::Error::other("bad")),
            description: "bad bind group".into(),
        };
        let e = map_error(&invalid);
        assert_eq!(
            (e.code.as_str(), &e.details["gpu"]),
            ("INTERNAL", &json!("validation"))
        );
        let internal = wgpu::Error::Internal {
            source: Box::new(std::io::Error::other("x")),
            description: "x".into(),
        };
        assert_eq!(map_error(&internal).code, "INTERNAL");
        assert_eq!(device_lost("Destroyed: gone").code, "GPU_DEVICE_LOST");
    }
}
