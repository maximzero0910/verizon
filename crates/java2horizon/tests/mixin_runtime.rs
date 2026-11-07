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
fn fabric_mixin_config_transforms_target_before_class_load() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = root.join("../..");
    let minecraft = repo.join(".local/minecraft/26.3");
    let client = minecraft.join("jars/client.jar");
    let fabric_libraries = repo.join(".local/j2h-compat/fabric-libraries");
    assert!(
        client.is_file(),
        "prepared Minecraft 26.3 client is required: {}",
        client.display()
    );
    assert!(
        fabric_libraries.join("fabric-loader-0.20.1.jar").is_file(),
        "pinned Fabric libraries are required: {}",
        fabric_libraries.display()
    );

    let temp = tempdir().unwrap();
    let source = temp.path().join("src");
    let classes = temp.path().join("classes");
    let mixin_source = source.join("fixture/mixin");
    let target_source = source.join("fixture");
    let aw_dir = temp.path().join("access-wideners");
    fs::create_dir_all(&mixin_source).unwrap();
    fs::create_dir_all(&target_source).unwrap();
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&aw_dir).unwrap();

    fs::write(
        target_source.join("Target.java"),
        "package fixture; public final class Target { public int value() { return 1; } }\n",
    )
    .unwrap();
    fs::write(target_source.join("Outer.java"),
             "package fixture; public final class Outer { public int value() { return 1; } public static final class Inner { public int value() { return 1; } } }\n")
        .unwrap();
    fs::write(
        mixin_source.join("TargetMixin.java"),
        r#"package fixture.mixin;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;
@Mixin(targets = "fixture.Target")
public abstract class TargetMixin {
    @Inject(method = "value", at = @At("HEAD"), cancellable = true)
    private void j2h$replaceValue(CallbackInfoReturnable<Integer> callback) {
        callback.setReturnValue(new java.util.function.IntSupplier() {
            @Override
            public int getAsInt() { return 42; }
        }.getAsInt());
    }
}
"#,
    )
    .unwrap();
    fs::write(
        mixin_source.join("NestedTargetMixin.java"),
        r#"package fixture.mixin;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;
@Mixin(targets = "fixture.Outer$Inner")
public abstract class NestedTargetMixin {
    @Inject(method = "value", at = @At("HEAD"), cancellable = true)
    private void j2h$replaceValue(CallbackInfoReturnable<Integer> callback) {
        callback.setReturnValue(new java.util.function.IntSupplier() {
            @Override
            public int getAsInt() { return 43; }
        }.getAsInt());
    }
}
"#,
    )
    .unwrap();
    fs::write(
        mixin_source.join("OuterMixin.java"),
        r#"package fixture.mixin;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable;
@Mixin(targets = "fixture.Outer")
public abstract class OuterMixin {
    @Inject(method = "value", at = @At("HEAD"), cancellable = true)
    private void j2h$replaceValue(CallbackInfoReturnable<Integer> callback) {
        callback.setReturnValue(new java.util.function.IntSupplier() {
            @Override
            public int getAsInt() { return 44; }
        }.getAsInt());
    }
}
"#,
    )
    .unwrap();
    fs::write(
        source.join("MixinHarness.java"),
        "public final class MixinHarness { public static int value() { return new fixture.Target().value(); } public static int outerValue() { return new fixture.Outer().value(); } public static int nestedValue() { return new fixture.Outer.Inner().value(); } }\n",
    )
    .unwrap();
    fs::create_dir_all(source.join("dev/horizon/j2h")).unwrap();
    fs::write(
        source.join("dev/horizon/j2h/MixinTestHooks.java"),
        r#"package dev.horizon.j2h;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
public final class MixinTestHooks {
    public static boolean generatedClassesShareHostDomain(String targetName) throws Exception {
        Class<?> host = Class.forName(targetName.replace('/', '.'));
        String names = AccessWidenerAgent.generatedMixinClassesForTarget(targetName);
        if (names.isEmpty()) return false;
        for (String name : names.split(",")) {
            Class<?> generated = Class.forName(name, false, host.getClassLoader());
            if (generated.getProtectionDomain() != host.getProtectionDomain()) return false;
        }
        return true;
    }

    public static boolean registryDiscoveryFailsClosed() throws Exception {
        Method method = AccessWidenerAgent.class.getDeclaredMethod(
                "registryCandidates", Object.class, String.class, Method.class);
        method.setAccessible(true);
        try {
            method.invoke(null, new Object(), "fixture/Outer", null);
            return false;
        } catch (InvocationTargetException failure) {
            Throwable cause = failure.getCause();
            return cause instanceof IllegalStateException
                    && cause.getMessage().contains("refusing NestMembers-only fallback");
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
            .arg(target_source.join("Outer.java"))
            .arg(mixin_source.join("TargetMixin.java"))
            .arg(mixin_source.join("NestedTargetMixin.java"))
            .arg(mixin_source.join("OuterMixin.java"))
            .arg(source.join("dev/horizon/j2h/MixinTestHooks.java"))
            .arg(source.join("MixinHarness.java")),
        "compile Mixin target and mixin",
    );

    let mod_root = temp.path().join("mod");
    fs::create_dir_all(mod_root.join("fixture/mixin")).unwrap();
    for entry in fs::read_dir(classes.join("fixture")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "class")
        {
            fs::copy(
                &path,
                mod_root.join("fixture").join(path.file_name().unwrap()),
            )
            .unwrap();
        }
    }
    for entry in fs::read_dir(classes.join("fixture/mixin")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "class")
        {
            fs::copy(
                &path,
                mod_root
                    .join("fixture/mixin")
                    .join(path.file_name().unwrap()),
            )
            .unwrap();
        }
    }
    fs::write(
        mod_root.join("fabric.mod.json"),
        r#"{"schemaVersion":1,"id":"mixintest","version":"1.0.0","name":"Mixin Test","depends":{"fabricloader":">=0.16.0","minecraft":"*"},"mixins":["j2h-test.mixins.json"]}"#,
    )
    .unwrap();
    fs::write(
        mod_root.join("j2h-test.mixins.json"),
        r#"{"required":true,"minVersion":"0.8","package":"fixture.mixin","compatibilityLevel":"JAVA_17","mixins":["TargetMixin","OuterMixin","NestedTargetMixin"],"injectors":{"defaultRequire":1}}"#,
    )
    .unwrap();
    let mod_jar = temp.path().join("mixintest.jar");
    run(
        Command::new("jar")
            .arg("--create")
            .arg("--file")
            .arg(&mod_jar)
            .arg("-C")
            .arg(&mod_root)
            .arg("."),
        "package Mixin test mod",
    );

    let agent = temp.path().join("j2h-access-widener-agent.jar");
    fs::write(&agent, J2H_ACCESS_WIDENER_AGENT_JAR).unwrap();
    let mut classpath = vec![
        root.join("test_resources/j2h-runtime.jar"),
        agent.clone(),
        mod_jar.clone(),
        classes
            .join("MixinHarness.class")
            .parent()
            .unwrap()
            .to_owned(),
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

    let value = jvm
        .call_static_i32("MixinHarness", "value", "()I")
        .expect("Mixin-transformed class loads");
    assert_eq!(
        value, 42,
        "the configured injection must change the target result"
    );
    let outer_value = jvm
        .call_static_i32("MixinHarness", "outerValue", "()I")
        .expect("outer target Mixin transforms before nested target");
    assert_eq!(outer_value, 44);
    let outer_generated = jvm
        .call_static_string_with_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "generatedMixinClassesForTarget",
            "(Ljava/lang/String;)Ljava/lang/String;",
            "fixture/Outer",
        )
        .expect("outer-generated class ownership is observable");
    assert!(
        !outer_generated.is_empty(),
        "Outer should own its generated class"
    );
    assert!(
        jvm.call_static_bool_with_string(
            "dev/horizon/j2h/MixinTestHooks",
            "generatedClassesShareHostDomain",
            "(Ljava/lang/String;)Z",
            "fixture/Outer",
        )
        .expect("Outer generated classes share its ProtectionDomain")
    );
    let nested_generated_before = jvm
        .call_static_string_with_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "generatedMixinClassesForTarget",
            "(Ljava/lang/String;)Ljava/lang/String;",
            "fixture/Outer$Inner",
        )
        .expect("nested-generated class ownership is observable");
    assert!(
        nested_generated_before.is_empty(),
        "transforming Outer must not claim Outer$Inner generated classes"
    );
    let nested_value = jvm
        .call_static_i32("MixinHarness", "nestedValue", "()I")
        .expect("nested Mixin target and generated anonymous class load");
    assert_eq!(
        nested_value, 43,
        "registry-discovered generated class must load for a nested Mixin target"
    );
    let repeated_nested_value = jvm
        .call_static_i32("MixinHarness", "nestedValue", "()I")
        .expect("the already-defined generated class remains loadable");
    assert_eq!(repeated_nested_value, 43);
    let nested_generated_after = jvm
        .call_static_string_with_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "generatedMixinClassesForTarget",
            "(Ljava/lang/String;)Ljava/lang/String;",
            "fixture/Outer$Inner",
        )
        .expect("nested target owns its generated class after transformation");
    assert!(!nested_generated_after.is_empty());
    assert!(
        jvm.call_static_bool_with_string(
            "dev/horizon/j2h/MixinTestHooks",
            "generatedClassesShareHostDomain",
            "(Ljava/lang/String;)Z",
            "fixture/Outer$Inner",
        )
        .expect("nested generated classes share their exact host ProtectionDomain")
    );
    assert!(
        jvm.call_static_bool(
            "dev/horizon/j2h/MixinTestHooks",
            "registryDiscoveryFailsClosed",
            "()Z"
        )
        .expect("registry fallback failure is exercised")
    );
}
