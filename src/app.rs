//! The window, the GPU, and the event loop.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use kahua_io::rbx_dom_weak::types::{Ref, Variant};
use kahua_render::{COLOR_FORMAT, Camera, Overlay, Renderer, Scene, wgpu};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::blit::Blit;
use crate::camera::{self, Orbit};
use crate::character::Character;
use crate::input::Input;
use crate::luau::{Options, OutputLine, ScriptHost, SharedDom};
use crate::physics::Physics;
use crate::rig;

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
    path: PathBuf,
    dom: SharedDom,
    scripts: ScriptHost,
    /// Recent script output, newest last.
    console: Vec<OutputLine>,
    scene: Scene,
    physics: Physics,
    character: Character,
    /// Game time when the character died; it respawns after Players.RespawnTime.
    dead_since: Option<f32>,
    /// Seconds since the place was loaded.
    time: f32,
    orbit: Orbit,
    /// Set while the free camera (F) is on; the character then stands still.
    free_camera: Option<Camera>,
    input: Input,
    gpu: Option<Gpu>,
    error: Option<anyhow::Error>,
    last_frame: Instant,
    fps_window: (Instant, u32),
}

impl App {
    /// Opens a place: spawns the character and starts the scripts, like joining a game.
    pub fn load(path: &Path, title: String) -> Result<Self> {
        let mut dom = kahua_io::read_file(path).with_context(|| format!("failed to open {}", path.display()))?;
        let model = rig::spawn(&mut dom);
        let scene = Scene::extract(&dom);
        for warning in &scene.warnings {
            eprintln!("warning: {warning}");
        }
        let mut physics = Physics::new(&dom, &scene, Some(model), None);
        let character = Character::new(&dom, model, &scene, &mut physics);
        let starter_player = rig::service(&dom, "StarterPlayer");
        let orbit = Orbit::new(
            character.yaw,
            rig::float(&dom, starter_player, "CameraMinZoomDistance", 0.5),
            rig::float(&dom, starter_player, "CameraMaxZoomDistance", 400.0),
        );
        character.write_back(&mut dom);
        let dom: SharedDom = Rc::new(RefCell::new(dom));
        let mut scripts = ScriptHost::new(dom.clone(), Options::default())
            .map_err(|err| anyhow::anyhow!("failed to start Luau: {err}"))?;
        scripts.start();
        let mut app = Self {
            title,
            path: path.to_path_buf(),
            dom,
            scripts,
            console: Vec::new(),
            scene,
            physics,
            character,
            dead_since: None,
            time: 0.0,
            orbit,
            free_camera: None,
            input: Input::default(),
            gpu: None,
            error: None,
            last_frame: Instant::now(),
            fps_window: (Instant::now(), 0),
        };
        app.sync_scripts();
        Ok(app)
    }

    /// Loads the place again from its file, like rejoining.
    fn restart(&mut self) {
        match App::load(&self.path, self.title.clone()) {
            Ok(fresh) => {
                let gpu = self.gpu.take();
                *self = fresh;
                self.gpu = gpu;
                if let Some(gpu) = &mut self.gpu {
                    gpu.set_scene(&self.scene);
                }
            }
            Err(err) => eprintln!("error: {err:#}"),
        }
    }

    /// Rebuilds the scene and physics if scripts changed the place. Moving
    /// parts keep their velocity and the character stays where it is, unless
    /// a script moved it.
    fn sync_scripts(&mut self) {
        // The host also prints the output; this copy is for the in-game console.
        const CONSOLE_LINES: usize = 500;
        self.console.extend(self.scripts.take_output());
        let excess = self.console.len().saturating_sub(CONSOLE_LINES);
        self.console.drain(..excess);
        if self.scripts.take_dirty() {
            self.rebuild();
        }
    }

    fn alive(&self) -> bool {
        self.dead_since.is_none()
    }

    /// Rebuilds the scene and physics from the DOM. A living character moves
    /// on its own; a dead one's parts are ordinary loose parts.
    fn rebuild(&mut self) {
        let dom = self.dom.borrow();
        self.scene = Scene::extract(&dom);
        let skip = self.alive().then_some(self.character.model);
        self.physics = Physics::new(&dom, &self.scene, skip, Some(&self.physics));
        if self.alive() {
            self.character.reattach(&dom, &self.scene, &mut self.physics);
        }
    }

    /// Fires Touched and TouchEnded on both parts of every pair that started or stopped touching.
    fn fire_touches(&mut self) {
        let (started, ended) = self.physics.touches();
        for (pairs, event) in [(started, "Touched"), (ended, "TouchEnded")] {
            for (a, b) in pairs {
                self.scripts.fire(a, event, &[b]);
                self.scripts.fire(b, event, &[a]);
            }
        }
    }

    /// Like Roblox: a Humanoid at 0 health, or one that fell out of the world,
    /// dies. Its joints break so the body falls apart.
    fn check_death(&mut self) {
        if !self.alive() {
            return;
        }
        let humanoid = self.character.humanoid;
        let (health, fell) = {
            let dom = self.dom.borrow();
            (rig::float(&dom, Some(humanoid), "Health", 100.0), self.character.fell(&dom))
        };
        if health > 0.0 && !fell {
            return;
        }
        {
            let mut dom = self.dom.borrow_mut();
            if let Some(h) = dom.get_by_ref_mut(humanoid) {
                h.properties.insert("Health".into(), Variant::Float32(0.0));
            }
            let joints: Vec<Ref> = dom
                .descendants_of(self.character.model)
                .filter(|i| matches!(i.class.as_str(), "Motor6D" | "Weld" | "ManualWeld" | "WeldConstraint" | "Snap"))
                .map(|i| i.referent())
                .collect();
            for joint in joints {
                dom.destroy(joint);
            }
        }
        // A script that set Health to 0 already fired Died.
        if health > 0.0 {
            self.scripts.fire(humanoid, "Died", &[]);
        }
        self.dead_since = Some(self.time);
        self.rebuild();
    }

    /// Replaces a dead character with a new one after Players.RespawnTime.
    fn check_respawn(&mut self) {
        let Some(died) = self.dead_since else { return };
        let respawn_time = {
            let dom = self.dom.borrow();
            rig::float(&dom, rig::service(&dom, "Players"), "RespawnTime", 5.0)
        };
        if self.time - died < respawn_time {
            return;
        }
        let (model, gone) = {
            let mut dom = self.dom.borrow_mut();
            let old = self.character.model;
            let gone: Vec<Ref> = dom.descendants_of(old).map(|i| i.referent()).collect();
            dom.destroy(old);
            (rig::spawn(&mut dom), gone)
        };
        self.scripts.forget(&gone);
        self.dead_since = None;
        {
            let dom = self.dom.borrow();
            self.scene = Scene::extract(&dom);
            self.physics = Physics::new(&dom, &self.scene, Some(model), Some(&self.physics));
            self.character = Character::new(&dom, model, &self.scene, &mut self.physics);
        }
        self.character.write_back(&mut self.dom.borrow_mut());
        self.scripts.character_added(model);
        self.sync_scripts();
    }

    /// Returns the error that stopped the event loop, if any.
    pub fn finish(self) -> Result<()> {
        self.error.map_or(Ok(()), Err)
    }

    fn set_looking(&mut self, looking: bool) {
        self.input.looking = looking;
        if let Some(gpu) = &self.gpu {
            gpu.set_cursor_captured(looking);
        }
    }

    fn toggle_free_camera(&mut self) {
        self.free_camera = match self.free_camera {
            Some(_) => None,
            None => Some(self.orbit.camera(self.character.focus())),
        };
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let camera = self.tick(dt);

        let Some(gpu) = &mut self.gpu else { return };
        gpu.set_scene(&self.scene);
        gpu.draw(&camera);

        let (start, frames) = &mut self.fps_window;
        *frames += 1;
        let elapsed = now.duration_since(*start).as_secs_f32();
        if elapsed >= 1.0 {
            gpu.window.set_title(&format!("{} ({:.0} FPS)", self.title, *frames as f32 / elapsed));
            self.fps_window = (now, 0);
        }
        gpu.window.request_redraw();
    }

    /// Advances the game by `dt` seconds: scripts, character, physics. Returns
    /// the camera to draw with.
    fn tick(&mut self, dt: f32) -> Camera {
        self.time += dt;
        // Scripts see where things are now, then may change them.
        {
            let mut dom = self.dom.borrow_mut();
            self.physics.write_back(&mut dom);
            if self.alive() {
                self.character.write_back(&mut dom);
            }
        }
        self.scripts.step(dt as f64);
        self.sync_scripts();

        let look = self.input.take_look();
        let scroll = self.input.take_scroll();
        let camera = match &mut self.free_camera {
            Some(camera) => {
                camera::fly(camera, &self.input, look, dt);
                *camera
            }
            None => {
                self.orbit.update(look, scroll);
                if self.alive() {
                    let dom = self.dom.borrow();
                    self.character.update(&dom, &self.input, self.orbit.yaw, dt, &mut self.physics);
                }
                self.orbit.camera(self.character.focus())
            }
        };
        self.physics.update(dt, &mut self.scene);
        if self.alive() {
            self.character.place(&mut self.scene);
        }
        self.fire_touches();
        self.check_death();
        self.check_respawn();
        camera
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
                self.input.release_all();
                self.set_looking(false);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let pressed = event.state.is_pressed();
                self.input.key(code, pressed);
                if !pressed || event.repeat {
                    return;
                }
                match code {
                    KeyCode::Escape => self.set_looking(false),
                    KeyCode::KeyR => self.restart(),
                    KeyCode::KeyF => self.toggle_free_camera(),
                    _ => {}
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => self.set_looking(state.is_pressed()),
            WindowEvent::MouseWheel { delta, .. } => self.input.scrolled(match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
            }),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.input.mouse_moved(dx, dy);
        }
    }
}

#[cfg(test)]
mod tests {
    use kahua_io::rbx_dom_weak::InstanceBuilder;
    use kahua_io::rbx_dom_weak::types::Variant;
    use kahua_io::template;
    use kahua_render::glam::Vec3;
    use kahua_render::scene::{cframe_to_mat4, part_cframe};

    use super::*;
    use crate::luau::OutputKind;

    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            app.tick(1.0 / 60.0);
        }
    }

    fn errors(app: &App) -> Vec<&str> {
        app.console.iter().filter(|l| l.kind == OutputKind::Error).map(|l| l.text.as_str()).collect()
    }

    /// Where the named part in Workspace is, and its forward direction.
    fn part(app: &App, name: &str) -> Option<(Vec3, Vec3)> {
        let dom = app.dom.borrow();
        let part = dom.descendants().find(|i| i.name == name)?;
        let transform = cframe_to_mat4(&part_cframe(part)?);
        Some((transform.w_axis.truncate(), transform.transform_vector3(Vec3::NEG_Z)))
    }

    #[test]
    fn scripts_change_the_world() {
        let mut app = App::load(Path::new("examples/scripts.rbxl"), String::new()).unwrap();
        let spinner_before = part(&app, "Spinner").unwrap().1;
        run(&mut app, 0.5);
        // The spinner turns 2 degrees per frame: 60 degrees in half a second.
        let turned = part(&app, "Spinner").unwrap().1.angle_between(spinner_before).to_degrees();
        assert!((turned - 60.0).abs() < 4.0, "turned {turned}");
        run(&mut app, 2.5);
        assert!(errors(&app).is_empty(), "{:?}", errors(&app));

        // Balls appear every half second and fall onto the baseplate.
        assert!(part(&app, "Ball6").is_some() && part(&app, "Ball8").is_none());
        let (ball, _) = part(&app, "Ball1").unwrap();
        assert!(ball.y < 10.0, "the first ball fell: {ball}");
        assert!(app.scene.parts.len() > 6, "new parts are drawn");

        // The LocalScript in StarterCharacterScripts made the character faster.
        assert!(app.console.iter().any(|l| l.text == "Hello, Player1! WalkSpeed is now 24"));
        // Walk away from the balls.
        let start = app.character.position;
        app.input.key(KeyCode::KeyS, true);
        run(&mut app, 1.0);
        let walked = app.character.position - start;
        assert!((walked.length() - 24.0).abs() < 1.5, "walked {walked}");
    }

    #[test]
    fn scripts_can_teleport_the_character() {
        let mut dom = template::baseplate();
        let source = "wait(1) workspace.Player1.HumanoidRootPart.CFrame = CFrame.new(50, 20, 50)";
        let workspace = template::workspace(&dom);
        dom.insert(workspace, InstanceBuilder::new("Script").with_property("Source", source));
        let path = std::env::temp_dir().join(format!("paani-teleport-{}.rbxl", std::process::id()));
        kahua_io::write_file(&dom, &path).unwrap();
        let mut app = App::load(&path, String::new()).unwrap();
        std::fs::remove_file(&path).unwrap();

        run(&mut app, 0.5);
        assert!(app.character.position.x.abs() < 1.0);
        run(&mut app, 2.0);
        assert!(errors(&app).is_empty(), "{:?}", errors(&app));
        let at = app.character.position;
        // Teleported, then fell onto the baseplate (top at y = 0, root 3 studs above the feet).
        assert!((at - Vec3::new(50.0, 3.0, 50.0)).length() < 0.1, "at {at}");
        // Scripts read the character where it is.
        let dom = app.dom.borrow();
        let root = dom.descendants().find(|i| i.name == "HumanoidRootPart").unwrap();
        let Some(Variant::CFrame(cf)) = root.properties.get(&"CFrame".into()) else { panic!() };
        assert!((cf.position.x - 50.0).abs() < 0.1);
    }
}

#[cfg(test)]
mod event_tests {
    use std::f32::consts::FRAC_PI_2;

    use kahua_render::glam::Vec3;

    use super::*;

    fn load() -> App {
        App::load(Path::new("examples/scripts.rbxl"), String::new()).unwrap()
    }

    fn run(app: &mut App, seconds: f32) {
        for _ in 0..(seconds * 60.0) as usize {
            app.tick(1.0 / 60.0);
        }
    }

    fn printed(app: &App, text: &str) -> usize {
        app.console.iter().filter(|l| l.text == text).count()
    }

    /// Walks toward `yaw` (0 = -Z) for `seconds`.
    fn walk(app: &mut App, yaw: f32, seconds: f32) {
        app.orbit.yaw = yaw;
        app.input.key(KeyCode::KeyW, true);
        run(app, seconds);
        app.input.key(KeyCode::KeyW, false);
    }

    #[test]
    fn kill_brick_kills_and_the_character_respawns() {
        let mut app = load();
        run(&mut app, 0.5);
        assert_eq!(printed(&app, "Player1 joined"), 1);
        assert_eq!(printed(&app, "Character spawned"), 1);
        let first = app.character.model;

        // Walk +X onto the kill brick.
        walk(&mut app, -FRAC_PI_2, 1.0);
        assert!(!app.alive(), "died at {}", app.character.position);
        assert_eq!(printed(&app, "Player1 died"), 1);
        {
            let dom = app.dom.borrow();
            assert!(dom.descendants_of(first).all(|i| i.class != "Motor6D"), "the body fell apart");
        }

        // A new character spawns after Players.RespawnTime (5 s).
        run(&mut app, 4.0);
        assert!(!app.alive());
        run(&mut app, 1.5);
        assert!(app.alive());
        assert_ne!(app.character.model, first);
        assert!(app.dom.borrow().get_by_ref(first).is_none(), "the old body is gone");
        assert_eq!(printed(&app, "Character spawned"), 2);
        assert!((app.character.position - Vec3::new(0.0, 4.0, 0.0)).length() < 0.1);
        // The new character's scripts ran again (WalkSpeed 24 from StarterCharacterScripts).
        let dom = app.dom.borrow();
        assert_eq!(rig::float(&dom, Some(app.character.humanoid), "WalkSpeed", 0.0), 24.0);
        assert_eq!(rig::float(&dom, Some(app.character.humanoid), "Health", 0.0), 100.0);
        drop(dom);
        let errors: Vec<_> = app.console.iter().filter(|l| l.kind == crate::luau::OutputKind::Error).collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn touching_a_coin_collects_it_once() {
        let mut app = load();
        run(&mut app, 0.5);
        walk(&mut app, FRAC_PI_2, 1.0);
        assert_eq!(printed(&app, "Coin collected!"), 1);
        assert!(app.dom.borrow().descendants().all(|i| i.name != "Coin"));
        assert!(app.alive());
    }

    #[test]
    fn falling_out_of_the_world_kills() {
        let mut app = load();
        run(&mut app, 0.5);
        let dom = app.dom.borrow();
        let root = dom.descendants().find(|i| i.name == "HumanoidRootPart").unwrap().referent();
        drop(dom);
        // Teleport far off the baseplate and let it fall.
        app.dom.borrow_mut().get_by_ref_mut(root).unwrap().properties.insert(
            "CFrame".into(),
            Variant::CFrame(kahua_render::scene::cframe_from_pos_rot(
                Vec3::new(5000.0, 0.0, 0.0),
                kahua_render::glam::Quat::IDENTITY,
            )),
        );
        app.rebuild();
        run(&mut app, 3.0);
        assert!(!app.alive());
        assert_eq!(printed(&app, "Player1 died"), 1);
    }
}
