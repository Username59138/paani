//! Moves the player's character like a Roblox Humanoid: walks at WalkSpeed
//! relative to the camera, turns to face where it walks, jumps with
//! JumpHeight or JumpPower, and respawns after falling out of the world.
//!
//! The Humanoid's properties are read every frame, so changing them in the
//! DOM (later: from scripts) takes effect right away.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use kahua_io::rbx_dom_weak::WeakDom;
use kahua_io::rbx_dom_weak::types::{Ref, Variant};
use kahua_render::Scene;
use kahua_render::glam::{Mat4, Quat, Vec3};
use kahua_render::scene::{bounds_of, cframe_to_mat4, mat4_to_cframe, part_cframe, part_size, prop, world_root};
use winit::keyboard::KeyCode;

use crate::input::Input;
use crate::physics::{Capsule, Physics};
use crate::rig::{boolean, child_of_class, float};

/// Workspace.FallenPartsDestroyHeight's default.
const FALLEN_HEIGHT: f32 = -500.0;
/// How quickly the character turns toward where it walks.
const TURN_RATE: f32 = 15.0;
/// Like a Humanoid, the character floats on floor raycasts instead of
/// standing on its legs: the collision capsule starts this far above the
/// feet, and anything lower is stepped onto without jumping.
const STEP_HEIGHT: f32 = 1.5;
/// While on the ground, how far down the character follows the floor (stairs, slopes).
const FLOOR_SNAP: f32 = 0.6;

pub struct Character {
    pub model: Ref,
    root: Ref,
    pub humanoid: Ref,
    /// Every part of the character with its transform relative to the root frame.
    members: Vec<(Ref, Mat4)>,
    /// The visible ones: (index in `Scene::parts`, transform relative to the root frame).
    parts: Vec<(usize, Mat4)>,
    capsule: Capsule,
    /// From the root frame to the capsule's center.
    capsule_offset: Vec3,
    half_height: f32,
    radius: f32,
    /// What the character touches with: (reported part, half size, offset from the capsule center).
    touch: (Ref, Vec3, Vec3),
    /// From the root down to the feet (negative).
    feet: f32,
    step: f32,
    pub position: Vec3,
    /// Radians; 0 faces -Z, like `Camera::yaw`.
    pub yaw: f32,
    vertical_speed: f32,
    grounded: bool,
}

impl Character {
    pub fn new(dom: &WeakDom, model: Ref, scene: &Scene, physics: &mut Physics) -> Self {
        let model_inst = dom.get_by_ref(model).unwrap();
        let root = match prop(model_inst, "PrimaryPart") {
            Some(Variant::Ref(r)) if dom.get_by_ref(*r).is_some() => *r,
            _ => dom
                .descendants_of(model)
                .find(|i| i.name == "HumanoidRootPart" && part_cframe(i).is_some())
                .or_else(|| dom.descendants_of(model).find(|i| part_cframe(i).is_some()))
                .map(|i| i.referent())
                .unwrap_or(model),
        };
        let humanoid = child_of_class(dom, model, "Humanoid").unwrap_or(model);
        let (position, yaw) = root_frame(dom, root).unwrap_or_default();
        let inverse = frame(position, yaw).inverse();
        let members: Vec<(Ref, Mat4)> = dom
            .descendants_of(model)
            .filter_map(|i| Some((i.referent(), inverse * cframe_to_mat4(&part_cframe(i)?))))
            .collect();
        let parts = visible(&members, scene);

        // A capsule as tall as the visible body and as wide as the root part.
        let corners = parts.iter().flat_map(|&(i, _)| scene.parts[i].corners()).map(|c| inverse.transform_point3(c));
        let (min, max) = bounds_of(corners).unwrap_or((Vec3::splat(-2.5), Vec3::splat(2.5)));
        let step = STEP_HEIGHT.min((max.y - min.y) * 0.4);
        let bottom = min.y + step;
        let height = max.y - bottom;
        let root_width = dom.get_by_ref(root).map_or(2.0, |r| part_size(r).x);
        let radius = (root_width * 0.5).clamp(0.2, height * 0.5);
        let half_height = (height * 0.5 - radius).max(0.01);
        let capsule_offset = Vec3::new(0.0, (bottom + max.y) * 0.5, 0.0);
        // Touches are reported as the lowest part (a leg), so `hit.Parent` is the
        // character. The box reaches a little below the feet to touch the floor.
        let lowest = members.iter().min_by(|a, b| a.1.w_axis.y.total_cmp(&b.1.w_axis.y)).map_or(root, |m| m.0);
        let (low, high) = (min - Vec3::new(0.0, 0.1, 0.0), max);
        let touch = (lowest, (high - low) * 0.5, (high + low) * 0.5 - capsule_offset);

        let spawn = spawn_point(dom, physics) - Vec3::new(0.0, min.y, 0.0);
        let capsule = physics.add_capsule(spawn + capsule_offset, half_height, radius, touch);
        Self {
            model,
            root,
            humanoid,
            members,
            parts,
            capsule,
            capsule_offset,
            half_height,
            radius,
            touch,
            feet: min.y,
            step,
            position: spawn,
            yaw,
            vertical_speed: 0.0,
            grounded: false,
        }
    }

    /// Hooks the character up to a rebuilt scene and physics world. If a script
    /// moved the root part (a teleport), the character continues from there.
    pub fn reattach(&mut self, dom: &WeakDom, scene: &Scene, physics: &mut Physics) {
        if let Some((position, yaw)) = root_frame(dom, self.root)
            && (position.distance(self.position) > 1e-3 || (yaw - self.yaw).abs() > 1e-3)
        {
            self.position = position;
            self.yaw = yaw;
            self.vertical_speed = 0.0;
        }
        self.parts = visible(&self.members, scene);
        self.capsule =
            physics.add_capsule(self.position + self.capsule_offset, self.half_height, self.radius, self.touch);
    }

    /// Writes the character's parts' CFrames, so scripts see where it is.
    pub fn write_back(&self, dom: &mut WeakDom) {
        let root = frame(self.position, self.yaw);
        for &(part, relative) in &self.members {
            if let Some(part) = dom.get_by_ref_mut(part) {
                part.properties.insert("CFrame".into(), Variant::CFrame(mat4_to_cframe(&(root * relative))));
            }
        }
    }

    /// Where the camera looks: around the head.
    pub fn focus(&self) -> Vec3 {
        self.position + Vec3::new(0.0, 1.5, 0.0)
    }

    pub fn update(&mut self, dom: &WeakDom, input: &Input, camera_yaw: f32, dt: f32, physics: &mut Physics) {
        let humanoid = Some(self.humanoid);
        let walk_speed = float(dom, humanoid, "WalkSpeed", 16.0);
        let gravity = physics.gravity();
        let jump_speed = if boolean(dom, humanoid, "UseJumpPower", false) {
            float(dom, humanoid, "JumpPower", 50.0)
        } else {
            (2.0 * gravity * float(dom, humanoid, "JumpHeight", 7.2)).sqrt()
        };
        let max_slope = float(dom, humanoid, "MaxSlopeAngle", 89.0).to_radians();

        let forward = Vec3::new(-camera_yaw.sin(), 0.0, -camera_yaw.cos());
        let right = Vec3::new(camera_yaw.cos(), 0.0, -camera_yaw.sin());
        let walk = (forward * input.axis(KeyCode::KeyW, KeyCode::KeyS)
            + right * input.axis(KeyCode::KeyD, KeyCode::KeyA))
        .normalize_or_zero();
        if walk != Vec3::ZERO {
            let target = (-walk.x).atan2(-walk.z);
            let diff = (target - self.yaw + PI).rem_euclid(TAU) - PI;
            self.yaw += diff * (1.0 - (-TURN_RATE * dt).exp());
        }

        // Holding Space keeps jumping on every landing, like in Roblox.
        if self.grounded && input.held(KeyCode::Space) {
            self.vertical_speed = jump_speed;
        }
        // Average of the speeds before and after this frame's gravity, which
        // is exact for constant gravity, so jumps reach JumpHeight.
        let rise = (self.vertical_speed - 0.5 * gravity * dt) * dt;
        self.vertical_speed -= gravity * dt;
        let desired = walk * walk_speed * dt + Vec3::Y * rise;
        let moved = physics.move_capsule(&self.capsule, self.position + self.capsule_offset, desired, dt, max_slope);
        self.position += moved;
        // Stop rising after bumping into a ceiling.
        if desired.y > 0.0 && moved.y < desired.y * 0.5 {
            self.vertical_speed = 0.0;
        }

        // Find the floor with rays from the bottom of the capsule (or from where
        // it was, if it fell past a floor this frame) down past the feet.
        let was_grounded = std::mem::take(&mut self.grounded);
        if self.vertical_speed <= 0.0 {
            let feet = self.position.y + self.feet;
            let top = feet + self.step - moved.y.min(0.0);
            let snap = if was_grounded { FLOOR_SNAP } else { 0.0 };
            let r = self.radius * 0.7;
            let origins = [
                Vec3::ZERO,
                Vec3::new(r, 0.0, 0.0),
                Vec3::new(-r, 0.0, 0.0),
                Vec3::new(0.0, 0.0, r),
                Vec3::new(0.0, 0.0, -r),
            ]
            .map(|o| Vec3::new(self.position.x, top, self.position.z) + o);
            if let Some(floor) = physics.floor_below(&self.capsule, &origins, top - feet + snap) {
                self.position.y = floor - self.feet;
                self.vertical_speed = 0.0;
                self.grounded = true;
            }
        }
        physics.place_capsule(&self.capsule, self.position + self.capsule_offset);
    }

    /// Whether the character fell below Workspace.FallenPartsDestroyHeight, which kills it.
    pub fn fell(&self, dom: &WeakDom) -> bool {
        self.position.y < float(dom, Some(world_root(dom)), "FallenPartsDestroyHeight", FALLEN_HEIGHT)
    }

    /// Moves the character's parts in the scene to where the character is.
    pub fn place(&self, scene: &mut Scene) {
        let root = frame(self.position, self.yaw);
        for &(i, relative) in &self.parts {
            scene.parts[i].transform = root * relative;
        }
    }
}

/// The position and facing of the root part, if it still exists.
fn root_frame(dom: &WeakDom, root: Ref) -> Option<(Vec3, f32)> {
    let transform = cframe_to_mat4(&part_cframe(dom.get_by_ref(root)?)?);
    let forward = transform.transform_vector3(Vec3::NEG_Z);
    Some((transform.w_axis.truncate(), (-forward.x).atan2(-forward.z)))
}

/// The members that are drawn, with their index in the scene.
fn visible(members: &[(Ref, Mat4)], scene: &Scene) -> Vec<(usize, Mat4)> {
    let relative: HashMap<Ref, Mat4> = members.iter().copied().collect();
    scene.parts.iter().enumerate().filter_map(|(i, p)| Some((i, *relative.get(&p.owner)?))).collect()
}

/// An upright frame at `position` facing `yaw`.
fn frame(position: Vec3, yaw: f32) -> Mat4 {
    Mat4::from_rotation_translation(Quat::from_rotation_y(yaw), position)
}

/// The ground point the character stands on when it spawns: the top of the
/// first enabled SpawnLocation, or whatever is below the world origin.
fn spawn_point(dom: &WeakDom, physics: &Physics) -> Vec3 {
    let spawn = dom
        .descendants_of(world_root(dom))
        .find(|i| i.class == "SpawnLocation" && !matches!(prop(i, "Enabled"), Some(Variant::Bool(false))));
    if let Some(spawn) = spawn
        && let Some(cf) = part_cframe(spawn)
    {
        let top = cframe_to_mat4(&cf).transform_point3(Vec3::new(0.0, part_size(spawn).y * 0.5, 0.0));
        return top;
    }
    let ground = physics.ground_below(0.0, 0.0).unwrap_or(0.0);
    Vec3::new(0.0, ground, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig;

    struct World {
        dom: WeakDom,
        scene: Scene,
        physics: Physics,
        character: Character,
        input: Input,
    }

    impl World {
        fn load(path: &str) -> Self {
            let mut dom = kahua_io::read_file(std::path::Path::new(path)).unwrap();
            let model = rig::spawn(&mut dom);
            let scene = Scene::extract(&dom);
            let mut physics = Physics::new(&dom, &scene, Some(model), None);
            let character = Character::new(&dom, model, &scene, &mut physics);
            Self { dom, scene, physics, character, input: Input::default() }
        }

        /// Runs `seconds` at 60 FPS and returns the highest point the character reached.
        fn run(&mut self, seconds: f32) -> f32 {
            let mut highest = f32::MIN;
            for _ in 0..(seconds * 60.0) as usize {
                self.character.update(&self.dom, &self.input, 0.0, 1.0 / 60.0, &mut self.physics);
                self.physics.update(1.0 / 60.0, &mut self.scene);
                highest = highest.max(self.character.position.y);
            }
            highest
        }
    }

    #[test]
    fn walks_and_jumps_like_a_humanoid() {
        let mut world = World::load("examples/physics.rbxl");
        world.run(1.0);
        let standing = world.character.position;
        assert!(world.character.grounded, "stands on the ground");
        world.run(1.0);
        assert!(
            (world.character.position - standing).length() < 0.05,
            "doesn't drift: {standing} -> {}",
            world.character.position
        );

        // It spawned on the SpawnLocation: its feet (3 studs below the root) on the spawn's top.
        assert!((standing - Vec3::new(0.0, 4.0, -30.0)).length() < 0.01, "spawned at {standing}");

        // Walk forward (away from everything) for one second at the default WalkSpeed of 16.
        world.input.key(KeyCode::KeyW, true);
        world.run(1.0);
        world.input.key(KeyCode::KeyW, false);
        let walked = world.character.position - standing;
        eprintln!("walked {walked}");
        assert!((walked.z + 16.0).abs() < 1.0 && walked.x.abs() < 0.5);
        // Turn around and walk back a bit: the character turns to face where it walks.
        world.input.key(KeyCode::KeyS, true);
        world.run(0.5);
        world.input.key(KeyCode::KeyS, false);
        assert!((world.character.yaw.rem_euclid(TAU) - PI).abs() < 0.05, "faces where it walks");

        // A jump rises JumpHeight (7.2 studs).
        let ground = world.character.position.y;
        world.input.key(KeyCode::Space, true);
        world.run(0.1);
        world.input.key(KeyCode::Space, false);
        let peak = world.run(1.5) - ground;
        eprintln!("jumped {peak}");
        assert!((peak - 7.2).abs() < 0.5);
        assert!((world.character.position.y - ground).abs() < 0.05, "lands again");
    }

    #[test]
    fn walk_speed_is_read_from_the_humanoid() {
        let mut world = World::load("examples/physics.rbxl");
        world.run(1.0);
        let humanoid = world.character.humanoid;
        world.dom.get_by_ref_mut(humanoid).unwrap().properties.insert("WalkSpeed".into(), Variant::Float32(32.0));
        let start = world.character.position;
        world.input.key(KeyCode::KeyW, true);
        world.run(1.0);
        assert!(((world.character.position - start).z + 32.0).abs() < 1.0);
    }
}
