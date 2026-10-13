//! A Studio-style free camera: hold the right mouse button to look around,
//! WASD to move, Q/E to go down/up, Shift to go faster.

use std::collections::HashSet;

use kahua_render::Camera;
use kahua_render::glam::Vec3;
use winit::keyboard::KeyCode;

/// Radians per pixel of mouse movement.
const LOOK_SPEED: f32 = 0.004;
/// Studs per second.
const MOVE_SPEED: f32 = 32.0;
const FAST_MOVE_SPEED: f32 = 128.0;

pub struct FlyCamera {
    pub camera: Camera,
    pub looking: bool,
    held: HashSet<KeyCode>,
}

impl FlyCamera {
    pub fn new(camera: Camera) -> Self {
        Self { camera, looking: false, held: HashSet::new() }
    }

    pub fn key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    /// Forgets held keys, e.g. when the window loses focus and misses key releases.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.looking = false;
    }

    pub fn mouse_moved(&mut self, dx: f64, dy: f64) {
        if self.looking {
            self.camera.rotate(-dx as f32 * LOOK_SPEED, -dy as f32 * LOOK_SPEED);
        }
    }

    pub fn update(&mut self, dt: f32) {
        let axis = |plus, minus| self.held.contains(&plus) as i32 as f32 - self.held.contains(&minus) as i32 as f32;
        let dir = self.camera.forward() * axis(KeyCode::KeyW, KeyCode::KeyS)
            + self.camera.right() * axis(KeyCode::KeyD, KeyCode::KeyA)
            + Vec3::Y * axis(KeyCode::KeyE, KeyCode::KeyQ);
        let fast = self.held.contains(&KeyCode::ShiftLeft) || self.held.contains(&KeyCode::ShiftRight);
        let speed = if fast { FAST_MOVE_SPEED } else { MOVE_SPEED };
        self.camera.position += dir.normalize_or_zero() * speed * dt;
    }
}
