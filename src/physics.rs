//! Rigid-body physics with rapier. Parts joined by welds move as one body,
//! like Roblox assemblies, and an assembly with any anchored part stays still.

use std::collections::HashMap;

use kahua_io::rbx_dom_weak::types::{Ref, Variant};
use kahua_io::rbx_dom_weak::{Instance, WeakDom};
use kahua_render::glam::{Mat4, Quat, Vec3};
use kahua_render::scene::{cframe_to_mat4, is_renderable, part_cframe, part_size, prop, world_root};
use kahua_render::{Scene, Shape, mesh};
use rapier3d::prelude as rp;

/// Workspace.Gravity's default, in studs/s².
const DEFAULT_GRAVITY: f32 = 196.2;
/// Roblox's Plastic. Other materials come later.
const FRICTION: f32 = 0.3;
const RESTITUTION: f32 = 0.0;
/// Joints that hold two parts rigidly together.
const WELDS: [&str; 5] = ["WeldConstraint", "Weld", "ManualWeld", "Snap", "Motor6D"];

pub struct Physics {
    world: rp::PhysicsWorld,
    /// Visible parts that can move: (index in `Scene::parts`, body, part transform relative to the body).
    moving: Vec<(usize, rp::RigidBodyHandle, Mat4)>,
    /// Simulation time not yet stepped.
    pending: f32,
}

impl Physics {
    pub fn new(dom: &WeakDom, scene: &Scene) -> Self {
        let mut world = rp::PhysicsWorld::new();
        let gravity = match dom.get_by_ref(world_root(dom)).and_then(|ws| prop(ws, "Gravity")) {
            Some(Variant::Float32(g)) => *g,
            _ => DEFAULT_GRAVITY,
        };
        world.gravity = rp::Vector::new(0.0, -gravity, 0.0);

        // Invisible parts are not in the scene but still collide, so read parts from the DOM.
        let parts: Vec<&Instance> =
            dom.descendants_of(world_root(dom)).filter(|i| is_renderable(i) && part_cframe(i).is_some()).collect();
        let index: HashMap<Ref, usize> = parts.iter().enumerate().map(|(i, p)| (p.referent(), i)).collect();
        let mut assemblies = UnionFind::new(parts.len());
        for joint in dom.descendants_of(world_root(dom)).filter(|i| WELDS.contains(&i.class.as_str())) {
            if matches!(prop(joint, "Enabled"), Some(Variant::Bool(false))) {
                continue;
            }
            let part = |key| match prop(joint, key) {
                Some(Variant::Ref(r)) => index.get(r).copied(),
                _ => None,
            };
            if let (Some(a), Some(b)) = (part("Part0"), part("Part1")) {
                assemblies.union(a, b);
            }
        }

        let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..parts.len() {
            members.entry(assemblies.find(i)).or_default().push(i);
        }
        let scene_index: HashMap<Ref, usize> = scene.parts.iter().enumerate().map(|(i, p)| (p.owner, i)).collect();
        let mut moving = Vec::new();
        for list in members.values() {
            let anchored = list.iter().any(|&i| matches!(prop(parts[i], "Anchored"), Some(Variant::Bool(true))));
            // The first part's transform is the body's; the others hang off it.
            let root = transform(parts[list[0]]);
            let builder = if anchored { rp::RigidBodyBuilder::fixed() } else { rp::RigidBodyBuilder::dynamic() };
            let body = world.insert_body(builder.pose(to_pose(root)));
            for &i in list {
                let relative = root.inverse() * transform(parts[i]);
                world.insert_collider(collider(parts[i], relative), Some(body));
                if !anchored && let Some(&s) = scene_index.get(&parts[i].referent()) {
                    moving.push((s, body, relative));
                }
            }
        }
        Self { world, moving, pending: 0.0 }
    }

    /// Advances the simulation by `dt` seconds in fixed steps and moves the
    /// scene's parts. Returns whether any part moved.
    pub fn update(&mut self, dt: f32, scene: &mut Scene) -> bool {
        let step = self.world.integration_parameters.dt;
        // Don't try to catch up after a long stall, it would stall again.
        self.pending = (self.pending + dt).min(step * 8.0);
        let mut stepped = false;
        while self.pending >= step {
            self.world.step();
            self.pending -= step;
            stepped = true;
        }
        if !stepped {
            return false;
        }
        let mut moved = false;
        for &(i, body, relative) in &self.moving {
            let body = &self.world.bodies[body];
            if !body.is_sleeping() {
                scene.parts[i].transform = from_pose(body.position()) * relative;
                moved = true;
            }
        }
        moved
    }
}

/// The part's position and rotation.
fn transform(part: &Instance) -> Mat4 {
    part_cframe(part).map(|cf| cframe_to_mat4(&cf)).unwrap_or_default()
}

fn collider(part: &Instance, relative: Mat4) -> rp::ColliderBuilder {
    let size = part_size(part).abs().max(Vec3::splat(0.05));
    let half = size * 0.5;
    let mut pose = relative;
    let builder = match shape_of(part) {
        Shape::Block => rp::ColliderBuilder::cuboid(half.x, half.y, half.z),
        Shape::Ball => rp::ColliderBuilder::ball(half.min_element()),
        Shape::Cylinder => {
            // Roblox cylinders lie along X, rapier's stand along Y.
            pose *= Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
            rp::ColliderBuilder::cylinder(half.x, half.y.min(half.z))
        }
        shape => {
            // Kahua's unit wedge meshes, scaled, so collisions match what is drawn.
            let points: Vec<rp::Vector> =
                mesh::build(shape).vertices.iter().map(|v| (Vec3::from(v.position) * size).to_array().into()).collect();
            rp::ColliderBuilder::convex_hull(&points)
                .unwrap_or_else(|| rp::ColliderBuilder::cuboid(half.x, half.y, half.z))
        }
    };
    let can_collide = !matches!(prop(part, "CanCollide"), Some(Variant::Bool(false)));
    builder.position(to_pose(pose)).friction(FRICTION).restitution(RESTITUTION).sensor(!can_collide)
}

fn shape_of(part: &Instance) -> Shape {
    match part.class.as_str() {
        "WedgePart" => Shape::Wedge,
        "CornerWedgePart" => Shape::CornerWedge,
        "Part" => match prop(part, "Shape") {
            Some(Variant::Enum(e)) => match e.to_u32() {
                0 => Shape::Ball,
                2 => Shape::Cylinder,
                3 => Shape::Wedge,
                4 => Shape::CornerWedge,
                _ => Shape::Block,
            },
            _ => Shape::Block,
        },
        // MeshParts and unions collide as their box for now.
        _ => Shape::Block,
    }
}

// rapier uses a different glam version than Kahua, so convert through arrays.

fn to_pose(m: Mat4) -> rp::Pose {
    let (_, rotation, translation) = m.to_scale_rotation_translation();
    rp::Pose::from_parts(translation.to_array().into(), rp::Rotation::from_array(rotation.to_array()))
}

fn from_pose(pose: &rp::Pose) -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_array(pose.rotation.to_array()),
        Vec3::from_array(pose.translation.to_array()),
    )
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(len: usize) -> Self {
        Self((0..len).collect())
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.0[a] = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates `seconds` of the physics demo place and returns where each named part ended up.
    fn settle(seconds: f32) -> HashMap<String, Vec3> {
        let dom = kahua_io::read_file(std::path::Path::new("examples/physics.rbxl")).unwrap();
        let mut scene = Scene::extract(&dom);
        let mut physics = Physics::new(&dom, &scene);
        for _ in 0..(seconds * 60.0) as usize {
            physics.update(1.0 / 60.0, &mut scene);
        }
        scene.parts.iter().map(|p| (dom.get_by_ref(p.owner).unwrap().name.clone(), p.position())).collect()
    }

    #[test]
    fn demo_place_settles() {
        let at = settle(4.0);
        let loose = ["Block0", "Block9", "Ball0", "Barrel", "Handle", "Head", "Floater"];
        for name in loose {
            eprintln!("{name}: {:.2}", at[name]);
        }
        // The tower stands: the top block rests on nine 2-stud blocks.
        assert!((at["Block9"].y - 19.0).abs() < 0.5);
        // The block rests on the invisible wall's top (y = 10) instead of the floor.
        assert!((at["Floater"].y - 11.5).abs() < 1.0);
        // The welded head stays at the same distance from the handle.
        assert!((at["Head"].distance(at["Handle"]) - Vec3::new(-2.4, 4.4, 0.0).length()).abs() < 0.05);
        // Loose parts end up on the floor, not under it.
        assert!(loose.iter().all(|name| at[*name].y > 0.0));
    }
}
