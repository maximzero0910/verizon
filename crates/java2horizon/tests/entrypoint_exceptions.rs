use java2horizon::{JvmConfig, JvmRuntime};
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn java_exceptions_keep_cause_and_clear_pending_exception_for_both_call_paths() {
    let scratch = tempdir().expect("scratch directory");
    let entrypoint_source = scratch.path().join("EntrypointFailure.java");
    fs::write(
        &entrypoint_source,
        r#"
public final class EntrypointFailure {
    public static void fail() {
        throw new IllegalStateException("j2h-entrypoint-boom");
    }

    public static void succeed() {}
}
"#,
    )
    .expect("write entrypoint Java fixture");
    let static_call_source = scratch.path().join("StaticCallFailure.java");
    fs::write(
        &static_call_source,
        r#"
public final class StaticCallFailure {
    public static void fail(String message) {
        throw new IllegalArgumentException(message);
    }

    public static void succeed(String value) {
        if (!"after-error".equals(value)) {
            throw new AssertionError("unexpected value: " + value);
        }
    }
}
"#,
    )
    .expect("write static-call Java fixture");
    let javac = Command::new("javac")
        .arg("--release")
        .arg("17")
        .arg("-d")
        .arg(scratch.path())
        .arg(&entrypoint_source)
        .arg(&static_call_source)
        .status()
        .expect("javac available");
    assert!(javac.success(), "javac failed to compile Java fixtures");

    let jvm = JvmRuntime::start(JvmConfig::with_classpath([scratch.path().to_owned()]))
        .expect("start JVM");
    let entrypoint_error = jvm
        .call_entrypoint("EntrypointFailure", "fail", "()V")
        .expect_err("throwing Java entrypoint should fail")
        .to_string();
    assert!(
        entrypoint_error.contains("IllegalStateException"),
        "{entrypoint_error}"
    );
    assert!(
        entrypoint_error.contains("j2h-entrypoint-boom"),
        "{entrypoint_error}"
    );
    assert!(
        entrypoint_error.contains("EntrypointFailure.fail"),
        "{entrypoint_error}"
    );
    jvm.call_entrypoint("EntrypointFailure", "succeed", "()V")
        .expect("entrypoint JNI call should recover after clearing exception");

    // Keep the thread attached across the failing and succeeding calls. This
    // prevents detach/reattach behavior from masking a pending JNI exception.
    let (static_error, pending_exception, recovery_error) = jvm
        .with_env(|env| {
            let error = jvm
                .call_static_void_with_string(
                    "StaticCallFailure",
                    "fail",
                    "(Ljava/lang/String;)V",
                    "j2h-static-call-boom",
                )
                .expect_err("throwing Java static call should fail")
                .to_string();
            let pending_exception = env.exception_check().unwrap_or(true);
            let recovery = jvm
                .call_static_void_with_string(
                    "StaticCallFailure",
                    "succeed",
                    "(Ljava/lang/String;)V",
                    "after-error",
                )
                .map_err(|error| error.to_string());
            Ok((error, pending_exception, recovery))
        })
        .expect("run both calls on one attached JNI thread");
    let mut failures = Vec::new();
    for expected in [
        "IllegalArgumentException",
        "j2h-static-call-boom",
        "at StaticCallFailure.fail(",
    ] {
        if !static_error.contains(expected) {
            failures.push(format!(
                "static-call error omitted {expected:?}: {static_error}"
            ));
        }
    }
    if pending_exception {
        failures.push("JNI exception remained pending after the static wrapper returned".into());
    }
    if let Err(error) = recovery_error {
        failures.push(format!(
            "subsequent JNI call failed after Java exception: {error}"
        ));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
