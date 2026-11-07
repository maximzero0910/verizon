use java2horizon::{J2H_ACCESS_WIDENER_AGENT_JAR, JvmConfig, JvmRuntime};
use std::fs;
use std::path::PathBuf;
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

#[test]
fn official_access_widener_changes_visibility_and_finality_before_class_load() {
    let temp = tempdir().unwrap();
    let src = temp.path().join("src/fixture");
    let classes = temp.path().join("classes");
    let agent = temp.path().join("j2h-access-widener-agent.jar");
    let client = temp.path().join("client.jar");
    let aw_dir = temp.path().join("access-wideners");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&aw_dir).unwrap();
    fs::write(
        src.join("Target.java"),
        r#"package fixture;
final class Target {
    private static final int VALUE = 7;
    private static final int MUTABLE = 11;
    private int read() { return VALUE; }
    private static int staticRead() { return VALUE; }
    private final void extendMe() {}
}
"#,
    )
    .unwrap();
    fs::write(
        src.join("AwHarness.java"),
        r#"package fixture;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
public final class AwHarness {
    public static String inspect() throws Exception {
        Class<?> target = Class.forName("fixture.Target");
        Field value = target.getDeclaredField("VALUE");
        Field mutable = target.getDeclaredField("MUTABLE");
        Method read = target.getDeclaredMethod("read");
        Method staticRead = target.getDeclaredMethod("staticRead");
        Method extend = target.getDeclaredMethod("extendMe");
        Object instance = target.getDeclaredConstructor().newInstance();
        return Modifier.isPublic(target.getModifiers()) + ":"
            + Modifier.isPublic(value.getModifiers()) + ":"
            + Modifier.isFinal(mutable.getModifiers()) + ":"
            + Modifier.isPublic(read.getModifiers()) + ":"
            + Modifier.isFinal(read.getModifiers()) + ":"
            + Modifier.isPublic(staticRead.getModifiers()) + ":"
            + Modifier.isFinal(staticRead.getModifiers()) + ":"
            + Modifier.isProtected(extend.getModifiers()) + ":"
            + Modifier.isFinal(extend.getModifiers()) + ":"
            + (instance instanceof Injected) + ":"
            + ((Injected) instance).injected();
    }
}
"#,
    )
    .unwrap();
    fs::write(
        src.join("Injected.java"),
        "package fixture; public interface Injected { default int injected() { return 19; } }\n",
    )
    .unwrap();
    run(
        Command::new("javac")
            .arg("--release")
            .arg("17")
            .arg("-d")
            .arg(&classes)
            .arg(src.join("Target.java"))
            .arg(src.join("AwHarness.java"))
            .arg(src.join("Injected.java")),
        "compile access widener fixtures",
    );
    run(
        Command::new("jar")
            .arg("--create")
            .arg("--file")
            .arg(&client)
            .arg("-C")
            .arg(&classes)
            .arg("fixture/Target.class"),
        "package pinned client fixture",
    );
    fs::write(&agent, J2H_ACCESS_WIDENER_AGENT_JAR).unwrap();
    fs::write(
        aw_dir.join("samplemod.aw"),
        "classTweaker v1 official\n\
         accessible class fixture/Target\n\
         accessible field fixture/Target VALUE I\n\
         mutable field fixture/Target MUTABLE I\n\
         accessible method fixture/Target read ()I\n\
         accessible method fixture/Target staticRead ()I\n\
         extendable method fixture/Target extendMe ()V\n\
         transitive-inject-interface fixture/Target fixture/Injected\n",
    )
    .unwrap();
    fs::write(
        aw_dir.join("badmod.aw"),
        "accessWidener v1 named\naccessible class fixture/Target\n",
    )
    .unwrap();

    let agent_option = format!(
        "-javaagent:{}={}|{}",
        agent.display(),
        aw_dir.display(),
        client.display()
    );
    // The agent's Mixin nest-member resolver uses the same pinned ASM runtime
    // library as Fabric. Keep it visible to the JVM that loads the javaagent.
    let asm = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.local/j2h-compat/fabric-libraries/asm-9.10.1.jar");
    assert!(
        asm.is_file(),
        "pinned ASM dependency is missing: {}",
        asm.display()
    );
    let mut config = JvmConfig::with_classpath([classes, client, agent, asm]);
    config.options.push(agent_option);
    let jvm = JvmRuntime::start(config).expect("embedded JVM should accept a Java agent option");
    let observed = jvm
        .call_static_string("fixture/AwHarness", "inspect", "()Ljava/lang/String;")
        .unwrap();
    assert_eq!(
        observed,
        "true:true:false:true:true:true:false:true:false:true:19"
    );
    assert!(
        jvm.call_static_bool_with_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "modRejected",
            "(Ljava/lang/String;)Z",
            "badmod",
        )
        .unwrap()
    );
    assert!(
        jvm.call_static_string_with_string(
            "dev/horizon/j2h/AccessWidenerAgent",
            "modFailure",
            "(Ljava/lang/String;)Ljava/lang/String;",
            "badmod",
        )
        .unwrap()
        .contains("official")
    );
}
