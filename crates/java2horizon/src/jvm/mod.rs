//! JVM initialization and class loading (jni 0.21).
//!
//! A process has one JVM: `JavaVM::new` cannot be called twice, and Fabric
//! mods expect one class loader for the whole session. `JvmRuntime::start`
//! therefore keeps the first VM and hands out handles to it; a later start
//! with another classpath is reported rather than silently ignored.

use jni::objects::{JObject, JString, JValue};
use jni::{InitArgsBuilder, JNIEnv, JNIVersion, JavaVM};
use log::{info, warn};
use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum JvmError {
    #[error("JNI: {0}")]
    Jni(#[from] jni::errors::Error),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// How to start the compatibility JVM.
#[derive(Clone, Debug)]
pub struct JvmConfig {
    pub classpath: Vec<PathBuf>,
    /// Extra `-D`/`-X` options, before the classpath option.
    pub options: Vec<String>,
    pub max_heap_mb: u32,
    pub min_heap_mb: u32,
}

impl Default for JvmConfig {
    fn default() -> Self {
        Self {
            classpath: Vec::new(),
            options: Vec::new(),
            max_heap_mb: 1024,
            min_heap_mb: 64,
        }
    }
}

impl JvmConfig {
    pub fn with_classpath(classpath: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            classpath: classpath.into_iter().collect(),
            ..Self::default()
        }
    }
}

struct Started {
    vm: Arc<JavaVM>,
    classpath: Vec<PathBuf>,
}

static STARTED: Lazy<Mutex<Option<Started>>> = Lazy::new(|| Mutex::new(None));

#[derive(Clone)]
pub struct JvmRuntime {
    vm: Arc<JavaVM>,
    classpath: Vec<PathBuf>,
}

impl JvmRuntime {
    /// Start the process JVM, or reuse the one already running.
    pub fn start(config: JvmConfig) -> Result<Self, JvmError> {
        let mut started = STARTED.lock().unwrap();
        if let Some(existing) = started.as_ref() {
            if existing.classpath != config.classpath {
                warn!(
                    "J2H: the JVM is already running with {} classpath entr{}; \
                     new jars need a restart",
                    existing.classpath.len(),
                    if existing.classpath.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                );
            }
            return Ok(Self {
                vm: existing.vm.clone(),
                classpath: existing.classpath.clone(),
            });
        }

        let cp = config
            .classpath
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        info!(
            "J2H: starting JVM ({} classpath entries)",
            config.classpath.len()
        );
        let mut args = InitArgsBuilder::new()
            .version(JNIVersion::V8)
            .option(format!("-Xms{}m", config.min_heap_mb.max(16)))
            .option(format!("-Xmx{}m", config.max_heap_mb.max(64)))
            .option("-Djava.awt.headless=true")
            .option("-Dfile.encoding=UTF-8");
        for option in &config.options {
            args = args.option(option);
        }
        let args = args
            .option(format!("-Djava.class.path={cp}"))
            .build()
            .map_err(|e| JvmError::Other(format!("JVM arguments: {e:?}")))?;
        let vm =
            JavaVM::new(args).map_err(|e| JvmError::Other(format!("starting the JVM: {e:?}")))?;
        let vm = Arc::new(vm);
        *started = Some(Started {
            vm: vm.clone(),
            classpath: config.classpath.clone(),
        });
        info!("J2H: JVM ready");
        Ok(Self {
            vm,
            classpath: config.classpath,
        })
    }

    pub fn new(classpath: &[PathBuf]) -> Result<Self, JvmError> {
        Self::start(JvmConfig::with_classpath(classpath.iter().cloned()))
    }

    pub fn with_env<F, R>(&self, f: F) -> Result<R, JvmError>
    where
        F: FnOnce(&mut JNIEnv) -> Result<R, JvmError>,
    {
        let mut env = self.vm.attach_current_thread()?;
        f(&mut env)
    }

    pub fn call_static_void(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
    ) -> Result<(), JvmError> {
        self.with_env(|env| {
            let class = match env.find_class(class_name) {
                Ok(class) => class,
                Err(error) => return Err(java_call_error(env, error, class_name, method, sig)),
            };
            if let Err(error) = env.call_static_method(class, method, sig, &[]) {
                return Err(java_call_error(env, error, class_name, method, sig));
            }
            check_exception(env, class_name, method, sig)
        })
    }

    /// Invoke a Fabric entrypoint. The milestone test mods expose a static
    /// method; Fabric's `ModInitializer`/`ClientModInitializer` are instance
    /// methods on an object the loader constructs.
    pub fn call_entrypoint(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
    ) -> Result<(), JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let is_static = match env.get_static_method_id(&class, method, sig) {
                Ok(_) => true,
                Err(_) => {
                    env.exception_clear()?;
                    false
                }
            };
            let result = if is_static {
                env.call_static_method(&class, method, sig, &[])
            } else {
                match env.new_object(&class, "()V", &[]) {
                    Ok(instance) => env.call_method(&instance, method, sig, &[]),
                    Err(error) => Err(error),
                }
            };
            if let Err(error) = result {
                if env.exception_check()? {
                    return Err(pending_java_exception(env, class_name, method, sig));
                }
                return Err(error.into());
            }
            check_exception(env, class_name, method, sig)
        })
    }

    /// Call a static method taking one `String` argument.
    pub fn call_static_void_with_string(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
        value: &str,
    ) -> Result<(), JvmError> {
        self.with_env(|env| {
            let class = match env.find_class(class_name) {
                Ok(class) => class,
                Err(error) => return Err(java_call_error(env, error, class_name, method, sig)),
            };
            let value = match env.new_string(value) {
                Ok(value) => value,
                Err(error) => return Err(java_call_error(env, error, class_name, method, sig)),
            };
            let result = env.call_static_method(
                class,
                method,
                sig,
                &[JValue::Object(&JObject::from(value))],
            );
            if let Err(error) = result {
                return Err(java_call_error(env, error, class_name, method, sig));
            }
            check_exception(env, class_name, method, sig)
        })
    }

    /// Call a static method with two `String` arguments.
    pub fn call_static_void_with_two_strings(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
        first: &str,
        second: &str,
    ) -> Result<(), JvmError> {
        self.with_env(|env| {
            let class = match env.find_class(class_name) {
                Ok(class) => class,
                Err(error) => return Err(java_call_error(env, error, class_name, method, sig)),
            };
            let first = match env.new_string(first) {
                Ok(value) => value,
                Err(error) => return Err(java_call_error(env, error, class_name, method, sig)),
            };
            let second = match env.new_string(second) {
                Ok(value) => value,
                Err(error) => return Err(java_call_error(env, error, class_name, method, sig)),
            };
            if let Err(error) = env.call_static_method(
                class,
                method,
                sig,
                &[
                    JValue::Object(&JObject::from(first)),
                    JValue::Object(&JObject::from(second)),
                ],
            ) {
                return Err(java_call_error(env, error, class_name, method, sig));
            }
            check_exception(env, class_name, method, sig)
        })
    }

    /// Call a static method with no arguments returning `boolean`.
    pub fn call_static_bool(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
    ) -> Result<bool, JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let r = env.call_static_method(class, method, sig, &[])?;
            check_exception(env, class_name, method, sig)?;
            Ok(r.z()?)
        })
    }

    /// Call a static method with no arguments returning `String`.
    pub fn call_static_string(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
    ) -> Result<String, JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let r = env.call_static_method(class, method, sig, &[])?;
            check_exception(env, class_name, method, sig)?;
            let obj = r.l()?;
            if obj.is_null() {
                return Ok(String::new());
            }
            Ok(env.get_string(&obj.into())?.into())
        })
    }

    /// Call a static method taking one `String` argument and returning `String`.
    pub fn call_static_string_with_string(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
        value: &str,
    ) -> Result<String, JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let value = env.new_string(value)?;
            let result = env.call_static_method(
                class,
                method,
                sig,
                &[JValue::Object(&JObject::from(value))],
            )?;
            check_exception(env, class_name, method, sig)?;
            let object = result.l()?;
            if object.is_null() {
                return Ok(String::new());
            }
            let result: JString = object.into();
            Ok(env.get_string(&result)?.into())
        })
    }

    /// Call a static method taking one `String` argument returning `boolean`.
    pub fn call_static_bool_with_string(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
        value: &str,
    ) -> Result<bool, JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let value = env.new_string(value)?;
            let r = env.call_static_method(
                class,
                method,
                sig,
                &[JValue::Object(&JObject::from(value))],
            )?;
            check_exception(env, class_name, method, sig)?;
            Ok(r.z()?)
        })
    }

    /// Call a static method with two `String` arguments returning `boolean`.
    pub fn call_static_bool_with_two_strings(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
        first: &str,
        second: &str,
    ) -> Result<bool, JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let first = env.new_string(first)?;
            let second = env.new_string(second)?;
            let result = env.call_static_method(
                class,
                method,
                sig,
                &[
                    JValue::Object(&JObject::from(first)),
                    JValue::Object(&JObject::from(second)),
                ],
            )?;
            check_exception(env, class_name, method, sig)?;
            Ok(result.z()?)
        })
    }

    /// Call a static `void` method with a positional list of int/double/float
    /// arguments (the small, typed forwarders the GUI bridge needs).
    pub fn call_void_args(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
        values: &[jni::objects::JValue],
    ) -> Result<(), JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            env.call_static_method(class, method, sig, values)?;
            check_exception(env, class_name, method, sig)
        })
    }

    /// Call a static method with no arguments returning `int`.
    pub fn call_static_i32(
        &self,
        class_name: &str,
        method: &str,
        sig: &str,
    ) -> Result<i32, JvmError> {
        self.with_env(|env| {
            let class = env.find_class(class_name)?;
            let r = env.call_static_method(class, method, sig, &[])?;
            check_exception(env, class_name, method, sig)?;
            Ok(r.i()?)
        })
    }

    /// Whether `class_name` (binary name, `/`-separated) loads.
    pub fn has_class(&self, class_name: &str) -> Result<bool, JvmError> {
        self.with_env(|env| match env.find_class(class_name) {
            Ok(_) => Ok(true),
            Err(_) => {
                let _ = env.exception_clear();
                Ok(false)
            }
        })
    }

    pub fn classpath(&self) -> &[PathBuf] {
        &self.classpath
    }
}

fn check_exception(
    env: &mut JNIEnv,
    class_name: &str,
    method: &str,
    sig: &str,
) -> Result<(), JvmError> {
    if env.exception_check()? {
        return Err(pending_java_exception(env, class_name, method, sig));
    }
    Ok(())
}

/// Preserve a Java throwable (including class-definition failures raised while
/// resolving the called class) and attach any fatal Mixin transformer cause
/// retained by the instrumentation agent.
fn java_call_error(
    env: &mut JNIEnv,
    error: jni::errors::Error,
    class_name: &str,
    method: &str,
    sig: &str,
) -> JvmError {
    if !env.exception_check().unwrap_or(false) {
        return error.into();
    }
    let mut failure = pending_java_exception(env, class_name, method, sig);
    if let Some(mixin_failure) = mixin_transform_failure(env) {
        if let JvmError::Other(message) = &mut failure {
            message.push_str("\nOriginal fatal Mixin transformer failure:\n");
            message.push_str(&mixin_failure);
        }
    }
    failure
}

/// Read the instrumentation agent's first fatal Mixin cause after clearing the
/// pending JNI exception. The agent is optional in JVM unit-test configurations.
fn mixin_transform_failure(env: &mut JNIEnv) -> Option<String> {
    let class = match env.find_class("dev/horizon/j2h/AccessWidenerAgent") {
        Ok(class) => class,
        Err(_) => {
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            return None;
        }
    };
    let result =
        env.call_static_method(class, "mixinTransformFailure", "()Ljava/lang/String;", &[]);
    let value = match result {
        Ok(value) => match value.l() {
            Ok(value) if !value.is_null() => {
                let value: JString = value.into();
                env.get_string(&value).ok().map(Into::into)
            }
            _ => None,
        },
        Err(_) => None,
    };
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    value.filter(|message: &String| !message.is_empty())
}

/// Turn a pending Java exception into an owned Rust error and clear it from
/// the attached JNI thread. JNI calls that throw return `JavaException` before
/// normal post-call checks run, so entrypoint invocation handles that case as
/// well as exceptions left pending by calls that returned normally.
fn pending_java_exception(env: &mut JNIEnv, class_name: &str, method: &str, sig: &str) -> JvmError {
    let throwable = env.exception_occurred().ok();
    let _ = env.exception_clear();

    let stack = throwable.as_ref().and_then(|throwable| {
        let rendered = (|| {
            let writer = env.new_object("java/io/StringWriter", "()V", &[]).ok()?;
            let printer = env
                .new_object(
                    "java/io/PrintWriter",
                    "(Ljava/io/Writer;)V",
                    &[JValue::Object(&writer)],
                )
                .ok()?;
            env.call_method(
                throwable,
                "printStackTrace",
                "(Ljava/io/PrintWriter;)V",
                &[JValue::Object(&printer)],
            )
            .ok()?;
            let value = env
                .call_method(&writer, "toString", "()Ljava/lang/String;", &[])
                .ok()?
                .l()
                .ok()?;
            let text: JString = value.into();
            env.get_string(&text).ok().map(Into::into)
        })();
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        rendered.filter(|message: &String| !message.is_empty())
    });

    let detail = stack
        .or_else(|| {
            let throwable = throwable.as_ref()?;
            let value = env
                .call_method(throwable, "toString", "()Ljava/lang/String;", &[])
                .ok()?
                .l()
                .ok()?;
            let text: JString = value.into();
            env.get_string(&text).ok().map(Into::into)
        })
        .filter(|message: &String| !message.is_empty())
        .unwrap_or_else(|| "Java exception (message unavailable)".to_owned());

    // `Throwable.toString()` is user-overridable and can itself throw.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    JvmError::Other(format!(
        "Java exception in {class_name}.{method}{sig}: {detail}"
    ))
}

/// Read a Java string without keeping the handle.
pub fn jstring(env: &mut JNIEnv, value: JString) -> String {
    env.get_string(&value)
        .map(|js| js.into())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_bounded() {
        let c = JvmConfig::default();
        assert!(c.min_heap_mb >= 16);
        assert!(c.max_heap_mb >= c.min_heap_mb);
        assert!(c.classpath.is_empty());
    }
}
