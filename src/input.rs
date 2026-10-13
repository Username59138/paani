//! Keyboard and mouse state collected from window events.

use std::collections::HashSet;

use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct Input {
    held: HashSet<KeyCode>,
    /// The right mouse button is down, so mouse movement turns the camera.
    pub looking: bool,
    look: (f32, f32),
    scroll: f32,
}

impl Input {
    pub fn key(&mut self, code: KeyCode, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    pub fn held(&self, code: KeyCode) -> bool {
        self.held.contains(&code)
    }

    /// 1 when `plus` is held, -1 when `minus` is, 0 for both or neither.
    pub fn axis(&self, plus: KeyCode, minus: KeyCode) -> f32 {
        self.held(plus) as i32 as f32 - self.held(minus) as i32 as f32
    }

    pub fn shift(&self) -> bool {
        self.held(KeyCode::ShiftLeft) || self.held(KeyCode::ShiftRight)
    }

    /// Forgets held keys, e.g. when the window loses focus and misses key releases.
    pub fn release_all(&mut self) {
        self.held.clear();
        self.looking = false;
    }

    pub fn mouse_moved(&mut self, dx: f64, dy: f64) {
        if self.looking {
            self.look.0 += dx as f32;
            self.look.1 += dy as f32;
        }
    }

    /// Scroll wheel movement in lines, positive away from the user.
    pub fn scrolled(&mut self, lines: f32) {
        self.scroll += lines;
    }

    /// Mouse movement in pixels since the last call.
    pub fn take_look(&mut self) -> (f32, f32) {
        std::mem::take(&mut self.look)
    }

    /// Scroll lines since the last call.
    pub fn take_scroll(&mut self) -> f32 {
        std::mem::take(&mut self.scroll)
    }
}
