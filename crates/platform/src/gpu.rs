//! Opening the GPU: an adapter and device for a window's surface, or with no window for tests and screenshots.
//! Opening is async because the browser's is; the desktop just waits for it.

/// The colour format everything is drawn in. Not sRGB, so a pixel in the art comes out as exactly that pixel.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    /// A GPU with no window. Any adapter will do, a software one included.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless() -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        pollster::block_on(Gpu::open(instance, None))
    }

    /// The browser's GPU: WebGPU where the browser has it, otherwise WebGL2.
    #[cfg(target_arch = "wasm32")]
    pub async fn browser_instance() -> wgpu::Instance {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        wgpu::util::new_instance_with_webgpu_detection(desc).await
    }

    /// The GPU that can draw to `surface`, made from `instance`.
    pub async fn open(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Gpu, String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: surface,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rts-platform"),
                // WebGL2's limits, so the same code runs in every browser.
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("no GPU device: {e}"))?;
        Ok(Gpu { instance, adapter, device, queue })
    }

    /// A one-line description of the adapter in use.
    pub fn describe(&self) -> String {
        let i = self.adapter.get_info();
        format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
    }
}
