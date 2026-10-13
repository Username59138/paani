//! Cameras: Roblox's default orbit camera around the character, and a
//! Studio-style free camera.

use kahua_render::Camera;
use kahua_render::glam::Vec3;
use winit::keyboard::KeyCode;

use crate::input::Input;

/// Radians per pixel of mouse movement.
const LOOK_SPEED: f32 = 0.004;

/// Roblox's Classic camera: right-drag to turn, scroll to zoom, always looking at the character.
pub struct Orbit {
    pub yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
}

impl Orbit {
    /// `min_distance` and `max_distance` come from StarterPlayer's CameraMinZoomDistance/CameraMaxZoomDistance.
    pub fn new(yaw: f32, min_distance: f32, max_distance: f32) -> Self {
        let distance = 12.5f32.clamp(min_distance, max_distance);
        Self { yaw, pitch: -20f32.to_radians(), distance, min_distance, max_distance }
    }

    pub fn update(&mut self, look: (f32, f32), scroll: f32) {
        self.yaw -= look.0 * LOOK_SPEED;
        self.pitch = (self.pitch - look.1 * LOOK_SPEED).clamp(-80f32.to_radians(), 80f32.to_radians());
        self.distance = (self.distance * 0.85f32.powf(scroll)).clamp(self.min_distance, self.max_distance);
    }

    /// The camera looking at `focus` (the character's head).
    pub fn camera(&self, focus: Vec3) -> Camera {
        let mut camera = Camera { yaw: self.yaw, pitch: self.pitch, ..Camera::default() };
        camera.position = focus - camera.forward() * self.distance;
        camera
    }
}

/// Moves a free camera: WASD to move, Q/E to go down/up, Shift to go faster.
pub fn fly(camera: &mut Camera, input: &Input, look: (f32, f32), dt: f32) {
    camera.rotate(-look.0 * LOOK_SPEED, -look.1 * LOOK_SPEED);
    let dir = camera.forward() * input.axis(KeyCode::KeyW, KeyCode::KeyS)
        + camera.right() * input.axis(KeyCode::KeyD, KeyCode::KeyA)
        + Vec3::Y * input.axis(KeyCode::KeyE, KeyCode::KeyQ);
    let speed = if input.shift() { 128.0 } else { 32.0 };
    camera.position += dir.normalize_or_zero() * speed * dt;
}
