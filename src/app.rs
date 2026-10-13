//! The window, the GPU, and the event loop.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use kahua_io::rbx_dom_weak::WeakDom;
use kahua_render::{COLOR_FORMAT, Camera, Overlay, Renderer, Scene, default_camera, wgpu};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::blit::Blit;
use crate::fly::FlyCamera;
use crate::physics::Physics;

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    blit: Blit,
}

impl Gpu {
    fn new(event_loop: &ActiveEventLoop, title: &str, scene: &Scene) -> Result<Self> {
        let attributes =
            Window::default_attributes().with_title(title).with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(event_loop.create_window(attributes)?);
        let size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .context("no graphics adapter found. Install a Vulkan driver")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("paani"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .context("failed to open the graphics device")?;

        // Kahua hands out its render target already sRGB-encoded, so the
        // surface must not encode again.
        let formats = surface.get_capabilities(&adapter).formats;
        let format = formats.iter().copied().find(|f| !f.is_srgb()).context("the window has no usable format")?;
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("the window surface is not supported by the graphics adapter")?;
        config.format = format;
        surface.configure(&device, &config);

        let msaa = adapter.get_texture_format_features(COLOR_FORMAT).flags.sample_count_supported(4);
        let mut renderer = Renderer::new(&device, if msaa { 4 } else { 1 });
        renderer.set_scene(&device, &queue, scene);
        let blit = Blit::new(&device, format);
        Ok(Self { window, surface, config, device, queue, renderer, blit })
    }

    fn set_scene(&mut self, scene: &Scene) {
        self.renderer.set_scene(&self.device, &self.queue, scene);
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    fn draw(&mut self, camera: &Camera) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let size = (self.config.width, self.config.height);
        let scene = self.renderer.render(&self.device, &self.queue, size, camera, &Overlay::default());
        let target = frame.texture.create_view(&Default::default());
        self.blit.draw(&self.device, &self.queue, scene, &target);
        self.window.pre_present_notify();
        self.queue.present(frame);
    }

    fn set_cursor_captured(&self, captured: bool) {
        if captured {
            // Wayland supports Locked, X11 only Confined.
            let _ = self
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
        }
        self.window.set_cursor_visible(!captured);
    }
}

pub struct App {
    title: String,
    dom: WeakDom,
    /// The scene as loaded, for restarting the simulation.
    initial_scene: Scene,
    scene: Scene,
    physics: Physics,
    fly: FlyCamera,
    gpu: Option<Gpu>,
    error: Option<anyhow::Error>,
    last_frame: Instant,
    fps_window: (Instant, u32),
}

impl App {
    pub fn new(dom: WeakDom, title: String) -> Self {
        let scene = Scene::extract(&dom);
        for warning in &scene.warnings {
            eprintln!("warning: {warning}");
        }
        let physics = Physics::new(&dom, &scene);
        let fly = FlyCamera::new(default_camera(&scene));
        Self {
            title,
            dom,
            initial_scene: scene.clone(),
            scene,
            physics,
            fly,
            gpu: None,
            error: None,
            last_frame: Instant::now(),
            fps_window: (Instant::now(), 0),
        }
    }

    /// Puts every part back where it was loaded and starts the simulation over.
    fn restart(&mut self) {
        self.scene = self.initial_scene.clone();
        self.physics = Physics::new(&self.dom, &self.scene);
        if let Some(gpu) = &mut self.gpu {
            gpu.set_scene(&self.scene);
        }
    }

    /// Returns the error that stopped the event loop, if any.
    pub fn finish(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    fn set_looking(&mut self, looking: bool) {
        self.fly.looking = looking;
        if let Some(gpu) = &self.gpu {
            gpu.set_cursor_captured(looking);
        }
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.fly.update(dt);
        let moved = self.physics.update(dt, &mut self.scene);

        let Some(gpu) = &mut self.gpu else { return };
        if moved {
            gpu.set_scene(&self.scene);
        }
        gpu.draw(&self.fly.camera);

        let (start, frames) = &mut self.fps_window;
        *frames += 1;
        let elapsed = now.duration_since(*start).as_secs_f32();
        if elapsed >= 1.0 {
            gpu.window.set_title(&format!("{} ({:.0} FPS)", self.title, *frames as f32 / elapsed));
            self.fps_window = (now, 0);
        }
        gpu.window.request_redraw();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match Gpu::new(event_loop, &self.title, &self.scene) {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(err) => {
                self.error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(false) => {
                self.fly.release_all();
                self.set_looking(false);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape {
                        self.set_looking(false);
                    }
                    if code == KeyCode::KeyR && event.state.is_pressed() && !event.repeat {
                        self.restart();
                    }
                    self.fly.key(code, event.state.is_pressed());
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => self.set_looking(state.is_pressed()),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.fly.mouse_moved(dx, dy);
        }
    }
}
