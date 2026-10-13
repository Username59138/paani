//! Runs small Luau programs against a baseplate place and checks the DOM and output.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use kahua_io::rbx_dom_weak::types::{Ref, Variant};
use kahua_io::rbx_dom_weak::{InstanceBuilder, Ustr, WeakDom};
use kahua_io::template;

use super::{Options, OutputKind, OutputLine, ScriptHost, SharedDom};
use crate::rig;

const STEP: f64 = 1.0 / 60.0;

/// Adds an instance of `class` named `name` with `source` under `parent`.
fn add(dom: &mut WeakDom, parent: Ref, class: &str, name: &str, source: Option<&str>) -> Ref {
    let mut builder = InstanceBuilder::new(class).with_name(name);
    if let Some(source) = source {
        builder = builder.with_property("Source", source.to_string());
    }
    dom.insert(parent, builder)
}

fn shared(dom: WeakDom) -> SharedDom {
    Rc::new(RefCell::new(dom))
}

/// A started host over a baseplate with one Script in Workspace running `source`.
fn host_with_script(source: &str) -> (ScriptHost, SharedDom) {
    let mut dom = template::baseplate();
    let workspace = template::workspace(&dom);
    add(&mut dom, workspace, "Script", "Script", Some(source));
    let shared = shared(dom);
    let mut host = ScriptHost::new(Rc::clone(&shared), Options::default()).expect("host");
    host.start();
    (host, shared)
}

/// Steps `host` for `seconds` of script time.
fn run_for(host: &mut ScriptHost, seconds: f64) {
    let steps = (seconds / STEP).round() as usize;
    for _ in 0..steps {
        host.step(STEP);
    }
}

fn errors(output: &[OutputLine]) -> Vec<String> {
    output.iter().filter(|l| l.kind == OutputKind::Error).map(|l| l.text.clone()).collect()
}

fn texts(output: &[OutputLine]) -> Vec<String> {
    output.iter().filter(|l| l.kind == OutputKind::Print).map(|l| l.text.clone()).collect()
}

fn find(dom: &WeakDom, parent: Ref, name: &str) -> Option<Ref> {
    dom.get_by_ref(parent)?.children().iter().copied().find(|&c| dom.get_by_ref(c).is_some_and(|i| i.name == name))
}

fn child_named(dom: &WeakDom, parent_name: &str, name: &str) -> Ref {
    let root = dom.root_ref();
    let parent = find(dom, root, parent_name).unwrap_or_else(|| panic!("no {parent_name}"));
    find(dom, parent, name).unwrap_or_else(|| panic!("no {name} under {parent_name}"))
}

fn prop<'a>(dom: &'a WeakDom, r: Ref, key: &str) -> Option<&'a Variant> {
    dom.get_by_ref(r)?.properties.get(&Ustr::from(key))
}

fn float(dom: &WeakDom, r: Ref, key: &str) -> f32 {
    match prop(dom, r, key) {
        Some(Variant::Float32(v)) => *v,
        other => panic!("{key} is {other:?}"),
    }
}

#[test]
fn print_joins_arguments() {
    let (mut host, _) = host_with_script(r#"print("hello", 1, 2.5, true)"#);
    host.step(STEP);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["hello 1 2.5 true"]);
}

#[test]
fn timeout_stops_runaway_loop() {
    let mut dom = template::baseplate();
    let workspace = template::workspace(&dom);
    add(&mut dom, workspace, "Script", "Script", Some("while true do end"));
    let options = Options { timeout: Duration::from_millis(200) };
    let mut host = ScriptHost::new(shared(dom), options).expect("host");
    host.start();
    let output = host.take_output();
    assert!(errors(&output).iter().any(|e| e.contains("timeout")), "{output:?}");
}

#[test]
fn instance_new_sets_properties_and_marks_dirty() {
    let source = r#"
        local part = Instance.new("Part")
        part.Position = Vector3.new(0, 20, 0)
        part.Color = Color3.new(1, 0, 0)
        part.Anchored = true
        part.Parent = workspace
        print("done")
    "#;
    let (mut host, shared) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert!(host.take_dirty());
    assert!(!host.take_dirty());
    let dom = shared.borrow();
    let part = child_named(&dom, "Workspace", "Part");
    let inst = dom.get_by_ref(part).unwrap();
    let cf = kahua_render::scene::part_cframe(inst).expect("CFrame");
    assert_eq!(cf.position, kahua_io::rbx_dom_weak::types::Vector3::new(0.0, 20.0, 0.0));
    assert!(matches!(prop(&dom, part, "Anchored"), Some(Variant::Bool(true))));
    match prop(&dom, part, "Color") {
        Some(Variant::Color3(c)) => assert_eq!((c.r, c.g, c.b), (1.0, 0.0, 0.0)),
        other => panic!("Color is {other:?}"),
    }
}

#[test]
fn defaults_for_new_part() {
    let source = r#"
        local p = Instance.new("Part")
        -- Roblox's Part is 4 x 1.2 x 2 (the reflection database default), not 4 x 1 x 2.
        assert(p.Size == Vector3.new(4, 1.2, 2))
        assert(p.Anchored == false)
        assert(p.Transparency == 0)
        assert(p.CFrame == CFrame.new())
        print("ok")
    "#;
    let (mut host, _) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["ok"]);
}

#[test]
fn wait_resumes_after_its_delay() {
    let source = r#"
        local part = Instance.new("Part")
        part.Name = "Waiter"
        part.Parent = workspace
        part.Transparency = 0.5
        wait(1)
        part.Transparency = 1
    "#;
    let (mut host, shared) = host_with_script(source);
    run_for(&mut host, 0.5);
    {
        let dom = shared.borrow();
        let part = child_named(&dom, "Workspace", "Waiter");
        assert_eq!(float(&dom, part, "Transparency"), 0.5);
    }
    run_for(&mut host, 0.6);
    let dom = shared.borrow();
    let part = child_named(&dom, "Workspace", "Waiter");
    assert_eq!(float(&dom, part, "Transparency"), 1.0);
    drop(dom);
    assert!(errors(&host.take_output()).is_empty());
}

#[test]
fn task_library_runs_in_documented_order() {
    let source = r#"
        local order = {}
        task.spawn(function() table.insert(order, "spawn") end)
        table.insert(order, "main")
        task.defer(function() table.insert(order, "defer") end)
        task.delay(0.5, function() table.insert(order, "delay") end)
        task.delay(0.6, function() print(table.concat(order, " ")) end)
    "#;
    let (mut host, _) = host_with_script(source);
    run_for(&mut host, 1.0);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["spawn main defer delay"]);
}

#[test]
fn vector_cframe_and_color_math() {
    let source = r#"
        local rotated = CFrame.Angles(0, math.pi / 2, 0) * Vector3.new(0, 0, -1)
        assert((rotated - Vector3.new(-1, 0, 0)).Magnitude < 1e-5)
        local cf = CFrame.new(1, 2, 3) * CFrame.Angles(0.3, 0.2, 0.1)
        local identity = cf * cf:Inverse()
        assert(identity.Position.Magnitude < 1e-4)
        assert((identity.LookVector - Vector3.new(0, 0, -1)).Magnitude < 1e-4)
        assert(Color3.fromRGB(255, 0, 0) == Color3.new(1, 0, 0))
        assert(Vector3.new(1, 2, 3) + Vector3.new(1, 1, 1) == Vector3.new(2, 3, 4))
        assert(Vector3.new(3, 4, 0).Magnitude == 5)
        assert(Vector3.new(1, 2, 3):Dot(Vector3.new(4, 5, 6)) == 32)
        assert(tostring(Vector3.new(1, 2, 3)) == "1, 2, 3")
        assert(typeof(cf) == "CFrame" and typeof(workspace) == "Instance")
        local look = CFrame.lookAt(Vector3.new(0, 0, 0), Vector3.new(0, 0, -10))
        assert((look.LookVector - Vector3.new(0, 0, -1)).Magnitude < 1e-5)
        local yaw = CFrame.Angles(0, math.rad(90), 0)
        assert(math.abs(yaw.RightVector.Z + 1) < 1e-5)
        print("math ok")
    "#;
    let (mut host, _) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["math ok"]);
}

#[test]
fn instance_api_children_clone_destroy() {
    let source = r#"
        local folder = Instance.new("Folder")
        folder.Name = "F"
        folder.Parent = workspace
        local a = Instance.new("Part")
        a.Name = "A"
        a.Parent = folder
        local b = Instance.new("Model")
        b.Name = "Deep"
        b.Parent = a
        assert(#folder:GetChildren() == 1)
        assert(folder:FindFirstChild("A") == a)
        assert(folder:FindFirstChild("Deep") == nil)
        assert(folder:FindFirstChild("Deep", true) == b)
        assert(#folder:GetDescendants() == 2)
        assert(a:IsA("BasePart") and not a:IsA("Model"))
        assert(b:IsDescendantOf(folder))
        assert(folder:IsAncestorOf(b))
        assert(folder.A == a)
        assert(workspace.Baseplate:IsA("BasePart"))
        assert(workspace.Baseplate:GetFullName() == "Workspace.Baseplate")
        local clone = workspace.Baseplate:Clone()
        assert(clone.Parent == nil)
        assert(clone ~= workspace.Baseplate)
        clone:Destroy()
        folder:Destroy()
        print("api ok")
    "#;
    let (mut host, shared) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["api ok"]);
    let dom = shared.borrow();
    assert!(find(&dom, template::workspace(&dom), "F").is_none());
}

#[test]
fn wait_for_child_finds_late_instance() {
    let source = r#"
        task.delay(0.5, function()
            local p = Instance.new("Part")
            p.Name = "Late"
            p.Parent = workspace
        end)
        local late = workspace:WaitForChild("Late")
        print(late.Name)
    "#;
    let (mut host, _) = host_with_script(source);
    run_for(&mut host, 1.0);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["Late"]);
}

#[test]
fn script_error_stops_only_that_script() {
    let mut dom = template::baseplate();
    let workspace = template::workspace(&dom);
    add(&mut dom, workspace, "Script", "Script", Some("local x = nil; x.y = 1"));
    add(&mut dom, workspace, "Script", "Other", Some("wait(0.1)\nprint(\"alive\")"));
    let mut host = ScriptHost::new(shared(dom), Options::default()).expect("host");
    host.start();
    run_for(&mut host, 0.5);
    let output = host.take_output();
    let errs = errors(&output);
    assert_eq!(errs.len(), 1, "{output:?}");
    assert!(errs[0].contains("Workspace.Script:1:"), "{errs:?}");
    assert_eq!(texts(&output), ["alive"]);
}

#[test]
fn require_caches_module_result() {
    let mut dom = template::baseplate();
    let replicated = rig::service(&dom, "ReplicatedStorage").expect("baseplate has ReplicatedStorage");
    add(&mut dom, replicated, "ModuleScript", "M", Some("return { value = 42 }"));
    let workspace = template::workspace(&dom);
    add(
        &mut dom,
        workspace,
        "Script",
        "Script",
        Some(
            r#"
            local M = game.ReplicatedStorage.M
            local a = require(M)
            local b = require(M)
            print(a.value, a == b)
            "#,
        ),
    );
    let mut host = ScriptHost::new(shared(dom), Options::default()).expect("host");
    host.start();
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["42 true"]);
}

#[test]
fn unknown_member_errors_use_roblox_message() {
    let source = r#"
        local p = Instance.new("Part")
        p.Parent = workspace
        print(pcall(function() return p.Foo end))
        print(pcall(function() p.Foo = 1 end))
        print(pcall(function() p.ClassName = "x" end))
    "#;
    let (mut host, _) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    let lines = texts(&output);
    assert!(lines[0].contains("Foo is not a valid member of Part \"Workspace.Part\""), "{lines:?}");
    assert!(lines[1].contains("Foo is not a valid member of Part \"Workspace.Part\""), "{lines:?}");
    assert!(lines[2].contains("ClassName"), "{lines:?}");
}

#[test]
fn enum_round_trip() {
    let source = r#"
        local p = Instance.new("Part")
        p.Material = Enum.Material.Neon
        assert(p.Material == Enum.Material.Neon)
        assert(p.Material.Name == "Neon")
        assert(p.Material ~= Enum.Material.Plastic)
        print("enum ok")
    "#;
    let (mut host, _) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["enum ok"]);
}

#[test]
fn local_script_in_starter_player_scripts_runs_for_player() {
    let mut dom = template::baseplate();
    rig::spawn(&mut dom);
    let starter = child_named(&dom, "StarterPlayer", "StarterPlayerScripts");
    add(
        &mut dom,
        starter,
        "LocalScript",
        "Local",
        Some(
            r#"
            local humanoid = game:GetService("Players").LocalPlayer.Character.Humanoid
            print(humanoid.WalkSpeed)
            humanoid.WalkSpeed = 50
            "#,
        ),
    );
    let shared = shared(dom);
    let mut host = ScriptHost::new(Rc::clone(&shared), Options::default()).expect("host");
    host.start();
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["16"]);
    let dom = shared.borrow();
    let model = child_named(&dom, "Workspace", rig::PLAYER_NAME);
    let humanoid = find(&dom, model, "Humanoid").expect("humanoid");
    assert_eq!(float(&dom, humanoid, "WalkSpeed"), 50.0);
}

#[test]
fn brick_color_attributes_and_script_global() {
    let source = r#"
        local p = Instance.new("Part")
        p.BrickColor = BrickColor.new("Bright red")
        print(p.BrickColor.Name, p.Color == Color3.fromRGB(196, 40, 28))
        p:SetAttribute("Hp", 5)
        assert(p:GetAttribute("Hp") == 5)
        p:SetAttribute("Hp", nil)
        assert(p:GetAttribute("Hp") == nil)
        print(script.Name)
    "#;
    let (mut host, _) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["Bright red true", "Script"]);
}

#[test]
fn disabled_scripts_do_not_run_and_cancel_stops_delayed_thread() {
    let mut dom = template::baseplate();
    let workspace = template::workspace(&dom);
    let disabled = add(&mut dom, workspace, "Script", "Disabled", Some("print(\"disabled ran\")"));
    dom.get_by_ref_mut(disabled).unwrap().properties.insert(Ustr::from("Disabled"), Variant::Bool(true));
    add(
        &mut dom,
        workspace,
        "Script",
        "Canceller",
        Some(
            r#"
            local bad = task.delay(0.5, function() print("cancelled ran") end)
            task.cancel(bad)
            task.delay(0.6, function() print("kept") end)
            "#,
        ),
    );
    let mut host = ScriptHost::new(shared(dom), Options::default()).expect("host");
    host.start();
    run_for(&mut host, 1.0);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["kept"]);
}

#[test]
fn position_and_orientation_round_trip() {
    let source = r#"
        local p = Instance.new("Part")
        p.Parent = workspace
        p.Position = Vector3.new(1, 2, 3)
        p.Orientation = Vector3.new(0, 90, 0)
        assert(p.Position == Vector3.new(1, 2, 3))
        assert(math.abs(p.Orientation.Y - 90) < 1e-3)
        assert(math.abs(p.CFrame.RightVector.Z + 1) < 1e-5)
        print("pose ok")
    "#;
    let (mut host, _) = host_with_script(source);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{output:?}");
    assert_eq!(texts(&output), ["pose ok"]);
}

#[test]
fn wait_inside_coroutines_sleeps_the_coroutine() {
    // A common Roblox pattern: a loop with wait() inside coroutine.wrap keeps
    // running in the background while the script continues.
    let source = r#"
        local ticks = 0
        coroutine.wrap(function()
            while true do
                wait(0.25)
                ticks += 1
            end
        end)()
        local co = coroutine.create(function()
            wait(0.5)
            print("resumed")
        end)
        assert(coroutine.resume(co) == true)
        wait(1.1)
        print("ticks " .. ticks)
    "#;
    let (mut host, _shared) = host_with_script(source);
    run_for(&mut host, 1.2);
    let output = host.take_output();
    assert!(errors(&output).is_empty(), "{:?}", errors(&output));
    assert_eq!(texts(&output), ["resumed", "ticks 4"]);
}
