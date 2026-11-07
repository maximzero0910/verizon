use java2horizon::lifecycle::{Lifecycle, ModInitializationStatus};
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run(command: &mut Command, what: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(status.success(), "{what} exited with {status}");
}

fn write_mod(
    mods: &std::path::Path,
    classes: &std::path::Path,
    metadata: &std::path::Path,
    jar_name: &str,
    package: &str,
    json: &str,
) {
    let meta_dir = metadata.join(jar_name);
    fs::create_dir_all(&meta_dir).expect("metadata directory");
    fs::write(meta_dir.join("fabric.mod.json"), json).expect("metadata file");
    let jar = mods.join(format!("{jar_name}.jar"));
    run(
        Command::new("jar")
            .arg("--create")
            .arg("--file")
            .arg(jar)
            .arg("-C")
            .arg(classes)
            .arg(package)
            .arg("-C")
            .arg(&meta_dir)
            .arg("fabric.mod.json"),
        "package test mod jar",
    );
}

#[test]
fn failed_mod_rolls_back_and_only_required_dependents_are_skipped() {
    let scratch = tempdir().expect("scratch directory");
    let sources = scratch.path().join("src");
    let classes = scratch.path().join("classes");
    let mods = scratch.path().join("mods");
    let metadata = scratch.path().join("metadata");
    let content = scratch.path().join("content");
    fs::create_dir_all(&sources).expect("source directory");
    fs::create_dir_all(&classes).expect("class directory");
    fs::create_dir_all(&mods).expect("mods directory");

    let java = [
        (
            "com/example/testmod/NativeBridge.java",
            r#"package com.example.testmod;
public final class NativeBridge {
    public static native void log(String message);
    public static native void registerBlock(String id, String name, String template,
            float hardness, int light, boolean withItem);
    public static native void registerItem(String id, String name, String template, int maxStack);
    public static native void entrypointComplete(String modId);
}
"#,
        ),
        (
            "fixture/broken/Entrypoint.java",
            r#"package fixture.broken;
import com.example.testmod.NativeBridge;
public final class Entrypoint {
    public static void onInitialize() {
        NativeBridge.registerBlock("brokenmod:leaked", "Leaked", "minecraft:stone", 1, 0, true);
        throw new IllegalStateException("intentional initialization failure");
    }
}
"#,
        ),
        (
            "fixture/good/Entrypoint.java",
            r#"package fixture.good;
import com.example.testmod.NativeBridge;
public final class Entrypoint {
    public static void onInitialize() {
        NativeBridge.registerBlock("goodmod:survives", "Good", "minecraft:stone", 1, 0, true);
    }
}
"#,
        ),
        (
            "fixture/required/Entrypoint.java",
            r#"package fixture.required;
import com.example.testmod.NativeBridge;
public final class Entrypoint {
    public static void onInitialize() {
        NativeBridge.registerBlock("requiredmod:must_not_load", "Required", "minecraft:stone", 1, 0, true);
    }
}
"#,
        ),
        (
            "fixture/optional/Entrypoint.java",
            r#"package fixture.optional;
import com.example.testmod.NativeBridge;
public final class Entrypoint {
    public static void onInitialize() {
        NativeBridge.registerBlock("optionalmod:survives", "Optional", "minecraft:stone", 1, 0, true);
    }
}
"#,
        ),
    ];
    let mut java_files = Vec::new();
    for (relative, source) in java {
        let path = sources.join(relative);
        fs::create_dir_all(path.parent().expect("source parent")).expect("package directory");
        fs::write(&path, source).expect("Java source");
        java_files.push(path);
    }
    let mut javac = Command::new("javac");
    javac.arg("--release").arg("17").arg("-d").arg(&classes);
    javac.args(&java_files);
    run(&mut javac, "compile transaction fixtures");

    run(
        Command::new("jar")
            .arg("--create")
            .arg("--file")
            .arg(mods.join("bridge.jar"))
            .arg("-C")
            .arg(&classes)
            .arg("com/example/testmod"),
        "package test bridge jar",
    );
    write_mod(
        &mods,
        &classes,
        &metadata,
        "broken",
        "fixture/broken",
        r#"{"schemaVersion":1,"id":"brokenmod","version":"1.0.0","entrypoints":{"main":["fixture.broken.Entrypoint"]}}"#,
    );
    write_mod(
        &mods,
        &classes,
        &metadata,
        "good",
        "fixture/good",
        r#"{"schemaVersion":1,"id":"goodmod","version":"1.0.0","entrypoints":{"main":["fixture.good.Entrypoint"]}}"#,
    );
    write_mod(
        &mods,
        &classes,
        &metadata,
        "required",
        "fixture/required",
        r#"{"schemaVersion":1,"id":"requiredmod","version":"1.0.0","depends":{"brokenmod":"*"},"entrypoints":{"main":["fixture.required.Entrypoint"]}}"#,
    );
    write_mod(
        &mods,
        &classes,
        &metadata,
        "optional",
        "fixture/optional",
        r#"{"schemaVersion":1,"id":"optionalmod","version":"1.0.0","recommends":{"brokenmod":"*"},"entrypoints":{"main":["fixture.optional.Entrypoint"]}}"#,
    );

    let mut lifecycle = Lifecycle::new();
    assert_eq!(lifecycle.discover_dir(&mods, None).unwrap(), 4);
    lifecycle
        .run_all(&content)
        .expect("lifecycle should isolate mod failure");

    assert!(!lifecycle.engine.has_block("brokenmod:leaked"));
    assert!(!lifecycle.engine.has_block("requiredmod:must_not_load"));
    assert!(lifecycle.engine.has_block("goodmod:survives"));
    assert!(lifecycle.engine.has_block("optionalmod:survives"));
    assert!(!content.join("brokenmod").exists());
    assert!(!content.join("requiredmod").exists());
    assert!(content.join("goodmod").exists());
    assert!(content.join("optionalmod").exists());
    assert!(
        lifecycle
            .engine
            .registration_owners()
            .iter()
            .all(|(owner, _)| owner != "brokenmod" && owner != "requiredmod")
    );

    let result = |id: &str| {
        lifecycle
            .mod_initializations
            .iter()
            .find(|record| record.mod_id == id)
            .expect("mod initialization record")
    };
    assert_eq!(result("brokenmod").status, ModInitializationStatus::Failed);
    assert!(result("brokenmod").diagnostics[0].contains("intentional initialization failure"));
    assert_eq!(
        result("requiredmod").status,
        ModInitializationStatus::Skipped
    );
    assert_eq!(
        result("goodmod").status,
        ModInitializationStatus::Initialized
    );
    assert_eq!(
        result("optionalmod").status,
        ModInitializationStatus::Initialized
    );
}
