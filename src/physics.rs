//! Rigid-body physics with rapier. Parts joined by welds move as one body,
//! like Roblox assemblies, and an assembly with any anchored part stays still.

use std::collections::{HashMap, HashSet};

use kahua_io::rbx_dom_weak::types::{Ref, Variant};
use kahua_io::rbx_dom_weak::{Instance, WeakDom};
use kahua_render::glam::{Mat4, Quat, Vec3};
use kahua_render::scene::{cframe_to_mat4, is_renderable, part_cframe, part_size, prop, world_root};
use kahua_render::{Scene, Shape, mesh};
use rapier3d::control::{CharacterLength, KinematicCharacterController};
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
    /// Builds bodies for the parts in Workspace, except those inside `skip`
    /// (the character, which moves on its own).
    pub fn new(dom: &WeakDom, scene: &Scene, skip: Option<Ref>) -> Self {
        let mut world = rp::PhysicsWorld::new();
        let gravity = match dom.get_by_ref(world_root(dom)).and_then(|ws| prop(ws, "Gravity")) {
            Some(Variant::Float32(g)) => *g,
            _ => DEFAULT_GRAVITY,
        };
        world.gravity = rp::Vector::new(0.0, -gravity, 0.0);
        // Roblox simulates at 240 Hz. Its gravity is strong in studs, so contacts
        // also need to be stiffer than rapier's default or stacks sink into each other.
        world.integration_parameters.dt = 1.0 / 240.0;
        world.integration_parameters.contact_softness.natural_frequency = 100.0;

        // Invisible parts are not in the scene but still collide, so read parts from the DOM.
        let skipped: HashSet<Ref> =
            skip.map(|r| dom.descendants_of(r).map(|i| i.referent()).collect()).unwrap_or_default();
        let parts: Vec<&Instance> = dom
            .descendants_of(world_root(dom))
            .filter(|i| is_renderable(i) && part_cframe(i).is_some() && !skipped.contains(&i.referent()))
            .collect();
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
        self.pending = (self.pending + dt).min(0.1);
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

impl Physics {
    pub fn gravity(&self) -> f32 {
        -self.world.gravity.y
    }

    /// The height of the highest solid surface below (x, z), if any.
    pub fn ground_below(&self, x: f32, z: f32) -> Option<f32> {
        const TOP: f32 = 1.0e5;
        let ray = rp::Ray::new(rp::Vector::new(x, TOP, z), rp::Vector::NEG_Y);
        let queries = self.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_sensors());
        queries.cast_ray(&ray, 2.0 * TOP, true).map(|(_, toi)| TOP - toi)
    }

    /// Adds a kinematic upright capsule for a character. It pushes loose parts
    /// but moves only through `place_capsule` and `teleport_capsule`.
    pub fn add_capsule(&mut self, center: Vec3, half_height: f32, radius: f32) -> Capsule {
        let body = self.world.insert_body(rp::RigidBodyBuilder::kinematic_position_based().translation(vector(center)));
        self.world.insert_collider(rp::ColliderBuilder::capsule_y(half_height, radius), Some(body));
        Capsule { body, shape: rp::SharedShape::capsule_y(half_height, radius) }
    }

    /// Moves the capsule centered at `center` by up to `desired`, sliding
    /// along walls and slopes steeper than `max_slope` radians. Returns how
    /// far it can move. Standing on floors is up to the caller (see `floor_below`).
    pub fn move_capsule(&self, capsule: &Capsule, center: Vec3, desired: Vec3, dt: f32, max_slope: f32) -> Vec3 {
        let controller = KinematicCharacterController {
            offset: CharacterLength::Absolute(0.02),
            max_slope_climb_angle: max_slope,
            min_slope_slide_angle: max_slope,
            snap_to_ground: None,
            ..Default::default()
        };
        let movement = controller.move_shape(
            dt,
            &self.world.query_pipeline_with_filter(capsule.filter()),
            &*capsule.shape,
            &rp::Pose::from_translation(vector(center)),
            vector(desired),
            |_| {},
        );
        Vec3::from_array(movement.translation.to_array())
    }

    /// The highest solid surface hit by rays going down `length` studs from
    /// each of `origins`, ignoring the capsule itself.
    pub fn floor_below(&self, capsule: &Capsule, origins: &[Vec3], length: f32) -> Option<f32> {
        let queries = self.world.query_pipeline_with_filter(capsule.filter());
        origins
            .iter()
            .filter_map(|o| {
                let ray = rp::Ray::new(vector(*o), rp::Vector::NEG_Y);
                queries.cast_ray(&ray, length, true).map(|(_, toi)| o.y - toi)
            })
            .reduce(f32::max)
    }

    /// Moves the capsule's body to `center` in the next step, pushing loose parts out of the way.
    pub fn place_capsule(&mut self, capsule: &Capsule, center: Vec3) {
        self.world.bodies[capsule.body].set_next_kinematic_translation(vector(center));
    }

    pub fn teleport_capsule(&mut self, capsule: &Capsule, center: Vec3) {
        self.world.bodies[capsule.body].set_translation(vector(center), true);
    }
}

/// A character's body in the physics world.
pub struct Capsule {
    body: rp::RigidBodyHandle,
    shape: rp::SharedShape,
}

impl Capsule {
    /// Scene queries for the character: everything solid except itself.
    fn filter(&self) -> rp::QueryFilter<'static> {
        rp::QueryFilter::default().exclude_rigid_body(self.body).exclude_sensors()
    }
}

fn vector(v: Vec3) -> rp::Vector {
    rp::Vector::from_array(v.to_array())
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
        let mut physics = Physics::new(&dom, &scene, None);
        for _ in 0..(seconds * 60.0) as usize {
            physics.update(1.0 / 60.0, &mut scene);
        }
        scene.parts.iter().map(|p| (dom.get_by_ref(p.owner).unwrap().name.clone(), p.position())).collect()
    }

    #[test]
    fn demo_place_settles() {
        let at = settle(4.0);
        let loose = [
            "Block0", "Block1", "Block2", "Block3", "Block4", "Block5", "Block6", "Block7", "Block8", "Block9",
            "Ball0", "Barrel", "Handle", "Head", "Floater",
        ];
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
