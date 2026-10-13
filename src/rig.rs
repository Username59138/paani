//! Puts the player's character into the place like Roblox does when a player
//! joins: StarterPlayer.StarterCharacter if the place has one, otherwise the
//! default R6 rig, plus a Player in the Players service pointing at it.

use kahua_io::rbx_dom_weak::types::{Color3, Enum, Ref, Variant, Vector3};
use kahua_io::rbx_dom_weak::{InstanceBuilder, Ustr, WeakDom};
use kahua_io::template;
use kahua_render::glam::{Quat, Vec3};
use kahua_render::scene::{cframe_from_pos_rot, prop, world_root};

pub const PLAYER_NAME: &str = "Player1";

struct BodyPart {
    name: &'static str,
    size: [f32; 3],
    /// From the HumanoidRootPart's center.
    offset: [f32; 3],
    color: [u8; 3],
}

const fn body_part(name: &'static str, size: [f32; 3], offset: [f32; 3], color: [u8; 3]) -> BodyPart {
    BodyPart { name, size, offset, color }
}

/// R6 body parts with Roblox's default body colors.
const R6: [BodyPart; 6] = [
    body_part("Torso", [2.0, 2.0, 1.0], [0.0, 0.0, 0.0], [13, 105, 172]),
    body_part("Head", [2.0, 1.0, 1.0], [0.0, 1.5, 0.0], [245, 205, 48]),
    body_part("Left Arm", [1.0, 2.0, 1.0], [-1.5, 0.0, 0.0], [245, 205, 48]),
    body_part("Right Arm", [1.0, 2.0, 1.0], [1.5, 0.0, 0.0], [245, 205, 48]),
    body_part("Left Leg", [1.0, 2.0, 1.0], [-0.5, -2.0, 0.0], [164, 189, 71]),
    body_part("Right Leg", [1.0, 2.0, 1.0], [0.5, -2.0, 0.0], [164, 189, 71]),
];

/// R6 joints: name, Part1, C0 offset in the Torso, C1 offset in Part1.
/// Roblox's real C0/C1 are also rotated; translations are enough for now.
const R6_JOINTS: [(&str, &str, [f32; 3], [f32; 3]); 5] = [
    ("Neck", "Head", [0.0, 1.0, 0.0], [0.0, -0.5, 0.0]),
    ("Left Shoulder", "Left Arm", [-1.0, 0.5, 0.0], [0.5, 0.5, 0.0]),
    ("Right Shoulder", "Right Arm", [1.0, 0.5, 0.0], [-0.5, 0.5, 0.0]),
    ("Left Hip", "Left Leg", [-1.0, -1.0, 0.0], [-0.5, 1.0, 0.0]),
    ("Right Hip", "Right Leg", [1.0, -1.0, 0.0], [0.5, 1.0, 0.0]),
];

/// A top-level service such as Players or StarterPlayer.
pub fn service(dom: &WeakDom, class: &str) -> Option<Ref> {
    dom.root().children().iter().copied().find(|&r| dom.get_by_ref(r).is_some_and(|i| i.class == class))
}

/// A number property, or `default` when the instance or property is missing.
pub fn float(dom: &WeakDom, inst: Option<Ref>, key: &str, default: f32) -> f32 {
    match inst.and_then(|r| dom.get_by_ref(r)).and_then(|i| prop(i, key)) {
        Some(Variant::Float32(v)) => *v,
        Some(Variant::Float64(v)) => *v as f32,
        _ => default,
    }
}

pub fn boolean(dom: &WeakDom, inst: Option<Ref>, key: &str, default: bool) -> bool {
    match inst.and_then(|r| dom.get_by_ref(r)).and_then(|i| prop(i, key)) {
        Some(Variant::Bool(v)) => *v,
        _ => default,
    }
}

/// The first child of `parent` with this class.
pub fn child_of_class(dom: &WeakDom, parent: Ref, class: &str) -> Option<Ref> {
    let parent = dom.get_by_ref(parent)?;
    parent.children().iter().copied().find(|&r| dom.get_by_ref(r).is_some_and(|i| i.class == class))
}

/// Adds the character to Workspace and returns the Model.
pub fn spawn(dom: &mut WeakDom) -> Ref {
    let workspace = world_root(dom);
    let starter = service(dom, "StarterPlayer").and_then(|sp| {
        let sp = dom.get_by_ref(sp)?;
        sp.children().iter().copied().find(|&r| dom.get_by_ref(r).is_some_and(|i| i.name == "StarterCharacter"))
    });
    let model = match starter {
        Some(starter) => {
            let model = dom.clone_within(starter);
            dom.transfer_within(model, workspace);
            model
        }
        None => build_r6(dom, workspace),
    };
    dom.get_by_ref_mut(model).unwrap().name = PLAYER_NAME.into();

    // Roblox applies StarterPlayer's character settings to the Humanoid.
    let humanoid = match child_of_class(dom, model, "Humanoid") {
        Some(h) => h,
        None => dom.insert(model, InstanceBuilder::new("Humanoid").with_name("Humanoid")),
    };
    let sp = service(dom, "StarterPlayer");
    let settings: Vec<(&str, Variant)> = vec![
        ("WalkSpeed", float(dom, sp, "CharacterWalkSpeed", 16.0).into()),
        ("JumpPower", float(dom, sp, "CharacterJumpPower", 50.0).into()),
        ("JumpHeight", float(dom, sp, "CharacterJumpHeight", 7.2).into()),
        ("UseJumpPower", boolean(dom, sp, "CharacterUseJumpPower", false).into()),
        ("MaxSlopeAngle", float(dom, sp, "CharacterMaxSlopeAngle", 89.0).into()),
        ("Health", 100.0f32.into()),
        ("MaxHealth", 100.0f32.into()),
    ];
    let humanoid = dom.get_by_ref_mut(humanoid).unwrap();
    for (key, value) in settings {
        humanoid.properties.insert(Ustr::from(key), value);
    }

    let players = match service(dom, "Players") {
        Some(p) => p,
        None => dom.insert(dom.root_ref(), InstanceBuilder::new("Players").with_name("Players")),
    };
    let player = dom
        .get_by_ref(players)
        .and_then(|p| p.children().iter().copied().find(|&r| dom.get_by_ref(r).is_some_and(|i| i.class == "Player")));
    match player {
        Some(player) => {
            dom.get_by_ref_mut(player).unwrap().properties.insert(Ustr::from("Character"), Variant::Ref(model));
        }
        None => {
            dom.insert(
                players,
                InstanceBuilder::new("Player").with_name(PLAYER_NAME).with_property("Character", model),
            );
        }
    }
    model
}

fn build_r6(dom: &mut WeakDom, workspace: Ref) -> Ref {
    let model = dom.insert(workspace, InstanceBuilder::new("Model").with_name(PLAYER_NAME));
    let part = |name: &str, size: [f32; 3], offset: [f32; 3], color: [u8; 3]| {
        let c = |v: u8| v as f32 / 255.0;
        template::part(
            name,
            "Part",
            Vector3::new(offset[0], offset[1], offset[2]),
            Vector3::new(size[0], size[1], size[2]),
            Color3::new(c(color[0]), c(color[1]), c(color[2])),
        )
        .with_property("Anchored", false)
    };
    let root = dom.insert(
        model,
        part("HumanoidRootPart", [2.0, 2.0, 1.0], [0.0; 3], [163, 162, 165])
            .with_property("Transparency", 1.0f32)
            .with_property("CanCollide", false),
    );
    let mut parts = Vec::new();
    for p in R6 {
        parts.push((p.name, dom.insert(model, part(p.name, p.size, p.offset, p.color))));
    }
    let find = |name: &str| parts.iter().find(|(n, _)| *n == name).unwrap().1;
    let torso = find("Torso");
    let offset = |v: [f32; 3]| cframe_from_pos_rot(Vec3::from(v), Quat::IDENTITY);
    let motor = |name: &str, part0: Ref, part1: Ref, c0: [f32; 3], c1: [f32; 3]| {
        InstanceBuilder::new("Motor6D")
            .with_name(name)
            .with_property("Part0", part0)
            .with_property("Part1", part1)
            .with_property("C0", offset(c0))
            .with_property("C1", offset(c1))
    };
    dom.insert(root, motor("RootJoint", root, torso, [0.0; 3], [0.0; 3]));
    for (name, part1, c0, c1) in R6_JOINTS {
        dom.insert(torso, motor(name, torso, find(part1), c0, c1));
    }
    let model_inst = dom.get_by_ref_mut(model).unwrap();
    model_inst.properties.insert(Ustr::from("PrimaryPart"), Variant::Ref(root));
    dom.insert(
        model,
        InstanceBuilder::new("Humanoid").with_name("Humanoid").with_property("RigType", Enum::from_u32(0)),
    );
    model
}
