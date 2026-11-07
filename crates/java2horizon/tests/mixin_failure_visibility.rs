use java2horizon::{J2H_ACCESS_WIDENER_AGENT_JAR, JvmConfig, JvmRuntime};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

fn run(command: &mut Command, label: &str) {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label}: {error}"));
    assert!(
        output.status.success(),
        "{label} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn jars_in(directory: &Path) -> Vec<PathBuf> {
    if !directory.is_dir() {
        return Vec::new();
    }
    let mut pending = vec![directory.to_owned()];
    let mut jars = Vec::new();
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(current).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "jar") {
                jars.push(path);
            }
        }
    }
    jars.sort();
    jars
}

#[test]
#[ignore = "requires pinned Minecraft 26.3 and Fabric libraries; run with --ignored"]
fn required_mixin_failure_is_visible_and_original_target_is_rejected() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = root.join("../..");
    let minecraft = repo.join(".local/minecraft/26.3");
    let client = minecraft.join("jars/client.jar");
    let fabric_libraries = repo.join(".local/j2h-compat/fabric-libraries");
    assert!(
        client.is_file(),
        "prepared Minecraft 26.3 client is required"
    );
    assert!(
        fabric_libraries.join("fabric-loader-0.20.1.jar").is_file(),
        "pinned Fabric libraries are required"
    );

    let temp = tempdir().unwrap();
    let source = temp.path().join("src");
    let classes = temp.path().join("classes");
    let mixin_source = source.join("fixture/mixin");
    let target_source = source.join("fixture");
    let aw_dir = temp.path().join("access-wideners");
    for directory in [&mixin_source, &target_source, &classes, &aw_dir] {
        fs::create_dir_all(directory).unwrap();
    }

    fs::write(
        target_source.join("Target.java"),
        "package fixture; public final class Target { public int value() { return 1; } }\n",
    )
    .unwrap();
    fs::write(
        target_source.join("OptionalTarget.java"),
        "package fixture; public final class OptionalTarget { public int value() { return 2; } }\n",
    )
    .unwrap();
    fs::write(
        mixin_source.join("BrokenMixin.java"),
        r#"package fixture.mixin;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
@Mixin(targets = "fixture.Target")
public abstract class BrokenMixin {
    @Inject(method = "definitelyMissing", at = @At("HEAD"))
    private void requiredInjection(CallbackInfo callback) {}
}
"#,
    )
    .unwrap();
    fs::write(
        mixin_source.join("OptionalMixin.java"),
        r#"package fixture.mixin;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;
@Mixin(targets = "fixture.OptionalTarget")
public abstract class OptionalMixin {
    @Inject(method = "definitelyMissing", at = @At("HEAD"))
    private void optionalInjection(CallbackInfo callback) {}
}
"#,
    )
    .unwrap();
    fs::write(
        source.join("MixinFailureProbe.java"),
        r#"public final class MixinFailureProbe {
    public static String observeOptional() {
        try {
            int value = new fixture.OptionalTarget().value();
            return "ACCEPTED:" + value;
        } catch (Throwable failure) {
            return failure.getClass().getName() + ":" + failure.getMessage();
        }
    }
    public static void trigger() {
        new fixture.Target().value();
    }
    public static String observe() {
        try {
            int value = new fixture.Target().value();
            return "ACCEPTED:" + value;
        } catch (Throwable failure) {
            java.io.StringWriter trace = new java.io.StringWriter();
            failure.printStackTrace(new java.io.PrintWriter(trace));
            return failure.getClass().getName() + ":" + failure.getMessage() + "\n" + trace;
        }
    }
}
"#,
    )
    .unwrap();

    let mut compile_classpath = jars_in(&minecraft.join("libraries"));
    compile_classpath.extend(jars_in(&fabric_libraries));
    compile_classpath.push(client.clone());
    compile_classpath.push(root.join("test_resources/j2h-access-widener-agent.jar"));
    let compile_classpath = compile_classpath
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    run(
        Command::new("javac")
            .arg("--release")
            .arg("17")
            .arg("-classpath")
            .arg(compile_classpath)
            .arg("-d")
            .arg(&classes)
            .arg(target_source.join("Target.java"))
            .arg(target_source.join("OptionalTarget.java"))
            .arg(mixin_source.join("BrokenMixin.java"))
            .arg(mixin_source.join("OptionalMixin.java"))
            .arg(source.join("MixinFailureProbe.java")),
        "compile required-failure Mixin fixture",
    );

    let mod_root = temp.path().join("mod");
    fs::create_dir_all(mod_root.join("fixture/mixin")).unwrap();
    for (from, to) in [
        (
            classes.join("fixture/Target.class"),
            mod_root.join("fixture/Target.class"),
        ),
        (
            classes.join("fixture/OptionalTarget.class"),
            mod_root.join("fixture/OptionalTarget.class"),
        ),
        (
            classes.join("fixture/mixin/BrokenMixin.class"),
            mod_root.join("fixture/mixin/BrokenMixin.class"),
        ),
        (
            classes.join("fixture/mixin/OptionalMixin.class"),
            mod_root.join("fixture/mixin/OptionalMixin.class"),
        ),
    ] {
        fs::copy(from, to).unwrap();
    }
    fs::write(
        mod_root.join("fabric.mod.json"),
        r#"{"schemaVersion":1,"id":"mixinfailuretest","version":"1.0.0","name":"Mixin Failure Test","depends":{"fabricloader":">=0.16.0","minecraft":"*"},"mixins":["j2h-optional.mixins.json","j2h-failure.mixins.json"]}"#,
    )
    .unwrap();
    fs::write(
        mod_root.join("j2h-optional.mixins.json"),
        r#"{"required":false,"minVersion":"0.8","package":"fixture.mixin","compatibilityLevel":"JAVA_17","mixins":["OptionalMixin"],"injectors":{"defaultRequire":1}}"#,
    )
    .unwrap();
    fs::write(
        mod_root.join("j2h-failure.mixins.json"),
        r#"{"required":true,"minVersion":"0.8","package":"fixture.mixin","compatibilityLevel":"JAVA_17","mixins":["BrokenMixin"],"injectors":{"defaultRequire":1}}"#,
    )
    .unwrap();
    let mod_jar = temp.path().join("mixin-failure-test.jar");
    run(
        Command::new("jar")
            .arg("--create")
            .arg("--file")
            .arg(&mod_jar)
            .arg("-C")
            .arg(&mod_root)
            .arg("."),
        "package required-failure Mixin fixture",
    );

    let agent = temp.path().join("j2h-access-widener-agent.jar");
    fs::write(&agent, J2H_ACCESS_WIDENER_AGENT_JAR).unwrap();
    let mut classpath = vec![
        root.join("test_resources/j2h-runtime.jar"),
        agent.clone(),
        mod_jar.clone(),
        classes.clone(),
        client.clone(),
    ];
    classpath.extend(jars_in(&minecraft.join("libraries")));
    classpath.extend(jars_in(&fabric_libraries));
    let agent_option = format!(
        "-javaagent:{}={}|{}",
        agent.display(),
        aw_dir.display(),
        client.display()
    );
    let mut config = JvmConfig::with_classpath(classpath);
    config.options.push(agent_option);
    let jvm = JvmRuntime::start(config).expect("embedded JVM starts with Mixin instrumentation");

    jvm.call_static_void(
        "dev/horizon/j2h/FabricLoaderBootstrap",
        "installLauncher",
        "()V",
    )
    .unwrap();
    jvm.call_static_void_with_string(
        "dev/horizon/j2h/FabricLoaderBootstrap",
        "setGameDir",
        "(Ljava/lang/String;)V",
        temp.path().to_str().unwrap(),
    )
    .unwrap();
    jvm.call_static_void_with_two_strings(
        "dev/horizon/j2h/FabricLoaderBootstrap",
        "initializeGameProvider",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        "26.3",
        client.to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        jvm.call_static_string(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "minecraftMetadataVersion",
            "()Ljava/lang/String;",
        )
        .unwrap(),
        "26.3"
    );
    assert_eq!(
        jvm.call_static_string(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "rawGameVersion",
            "()Ljava/lang/String;",
        )
        .unwrap(),
        "26.3"
    );
    for (java_version, expected) in [("24", false), ("25", true), ("26", true)] {
        assert_eq!(
            jvm.call_static_bool_with_string(
                "dev/horizon/j2h/FabricLoaderBootstrap",
                "minecraftJavaDependencyAccepts",
                "(Ljava/lang/String;)Z",
                java_version,
            )
            .unwrap(),
            expected,
            "Minecraft built-in Java dependency should evaluate Java {java_version} correctly"
        );
    }
    assert!(
        jvm.call_static_void_with_two_strings(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "validateClientJarVersion",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            client.to_str().unwrap(),
            "not-26.3",
        )
        .is_err(),
        "provider must reject a client JAR whose embedded version differs"
    );
    jvm.call_static_void_with_string(
        "dev/horizon/j2h/FabricLoaderBootstrap",
        "registerMod",
        "(Ljava/lang/String;)V",
        mod_jar.to_str().unwrap(),
    )
    .unwrap();
    jvm.call_static_void(
        "dev/horizon/j2h/FabricLoaderBootstrap",
        "finalizeLoader",
        "()V",
    )
    .unwrap();
    assert!(
        jvm.call_static_bool_with_string(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "modLoaded",
            "(Ljava/lang/String;)Z",
            "minecraft",
        )
        .unwrap(),
        "FabricLoader.isModLoaded must include the built-in Minecraft provider"
    );
    assert_eq!(
        jvm.call_static_i32("dev/horizon/j2h/FabricLoaderBootstrap", "modCount", "()I",)
            .unwrap(),
        1,
        "bootstrap mod count excludes the built-in Minecraft container"
    );
    jvm.call_static_void_with_string(
        "dev/horizon/j2h/FabricLoaderBootstrap",
        "initializeMixins",
        "(Ljava/lang/String;)V",
        "client",
    )
    .unwrap();

    let optional_observed = jvm
        .call_static_string(
            "MixinFailureProbe",
            "observeOptional",
            "()Ljava/lang/String;",
        )
        .expect("optional Mixin failure must not poison class definition");
    let optional_diagnostic = jvm
        .call_static_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "mixinTransformFailure",
            "()Ljava/lang/String;",
        )
        .expect("agent remains callable after an optional Mixin warning");
    assert_eq!(optional_observed, "ACCEPTED:2");
    assert!(
        optional_diagnostic.is_empty(),
        "optional Mixin warning must not be promoted to a fatal transformation error: {optional_diagnostic}"
    );

    let boundary_error = jvm
        .call_static_void("MixinFailureProbe", "trigger", "()V")
        .expect_err("J2H JNI boundary must reject a required Mixin failure")
        .to_string();
    let observed = jvm
        .call_static_string("MixinFailureProbe", "observe", "()Ljava/lang/String;")
        .expect("Java probe captures the target load result or throwable stack");
    let diagnostic = jvm
        .call_static_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "mixinTransformFailure",
            "()Ljava/lang/String;",
        )
        .expect("agent failure diagnostic remains readable after the class load");

    assert!(
        !diagnostic.is_empty(),
        "required Mixin transform failure must be recorded; probe result: {observed}"
    );
    assert!(
        boundary_error.contains("ClassFormatError")
            && boundary_error.contains("fixture.Target")
            && (boundary_error.contains("InvalidInjectionException")
                || boundary_error.contains("InvalidMixinException")
                || boundary_error.contains("MixinApplyError")),
        "JNI failure must include the class-definition error and retained Mixin cause chain, got: {boundary_error}"
    );
    assert!(
        !observed.starts_with("ACCEPTED:"),
        "failed required transformation must not accept original target behavior; observed: {observed}; diagnostic: {diagnostic}"
    );
    assert!(
        diagnostic.contains("fixture.Target")
            && (diagnostic.contains("InvalidInjectionException")
                || diagnostic.contains("InvalidMixinException")
                || diagnostic.contains("MixinApplyError")
                || diagnostic.contains("InjectionError")),
        "diagnostic must retain the required Mixin failure cause chain, got: {diagnostic}"
    );
    assert!(
        observed.contains("java.lang.ClassFormatError")
            || observed.contains("MixinApplyError")
            || observed.contains("InvalidMixinException"),
        "caller should receive a Java transformation failure with its stack, got: {observed}"
    );
}
