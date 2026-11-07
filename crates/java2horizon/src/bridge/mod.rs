//! Typed Java <-> Rust bridge.
//!
//! A mod calls the static methods of `dev.horizon.j2h.NativeBridge` (or one
//! of the compatibility names kept for the milestone test mods). The native
//! methods only copy Java strings into Rust-owned values and append an event;
//! no Java object handle is kept after the call.
//!
//! Content registrations become a Horizon content-mod tree in
//! [`crate::horizon_shim`], which is validated with the real `horizon-mods`
//! loader. Lifecycle callbacks stay in Rust for the staged loader.

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jfloat, jint};
use log::{debug, info};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

use crate::jvm::{JvmError, JvmRuntime, jstring};

#[derive(Error, Debug)]
pub enum BridgeError {
    #[error("JVM: {0}")]
    Jvm(#[from] JvmError),
    #[error("JNI: {0}")]
    Jni(#[from] jni::errors::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub enum NativeCallback {
    Message(String),
    RegisterBlock {
        id: String,
        name: String,
        template: Option<String>,
        hardness: Option<f32>,
        light: Option<u8>,
        with_item: bool,
    },
    RegisterItem {
        id: String,
        name: String,
        template: Option<String>,
        max_stack_size: Option<u8>,
    },
    RegisterEntityType {
        id: String,
        template: Option<String>,
        item: Option<String>,
        damage: Option<f32>,
    },
    RegisterMobEffect {
        id: String,
        name: String,
        color: u32,
        category: String,
    },
    RegisterCreativeTab {
        id: String,
        title: String,
        icon: String,
        items: Vec<String>,
    },
    RegisterMenuType {
        id: String,
        slots: u16,
        data_slots: u16,
        results: Vec<u16>,
    },
    RegisterCommand {
        name: String,
        description: String,
    },
    LifecycleEvent {
        name: String,
        detail: String,
    },
    EntrypointComplete {
        mod_id: String,
    },
}

/// Registrations produced by one mod entrypoint group. The transaction is an
/// owned value; JNI callbacks only borrow it through the scoped context below.
#[derive(Clone)]
pub struct RegistrationTransaction {
    mod_id: Arc<str>,
    events: Arc<Mutex<Vec<NativeCallback>>>,
    status: Arc<AtomicU8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RegistrationTransactionStatus {
    Pending = 0,
    Committed = 1,
    RolledBack = 2,
}

impl RegistrationTransaction {
    pub fn new(mod_id: impl Into<Arc<str>>) -> Self {
        Self {
            mod_id: mod_id.into(),
            events: Arc::new(Mutex::new(Vec::new())),
            status: Arc::new(AtomicU8::new(RegistrationTransactionStatus::Pending as u8)),
        }
    }

    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub fn events(&self) -> Vec<NativeCallback> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }

    pub fn status(&self) -> RegistrationTransactionStatus {
        match self.status.load(Ordering::Acquire) {
            1 => RegistrationTransactionStatus::Committed,
            2 => RegistrationTransactionStatus::RolledBack,
            _ => RegistrationTransactionStatus::Pending,
        }
    }

    pub fn commit(&self) {
        let _ = self.status.compare_exchange(
            RegistrationTransactionStatus::Pending as u8,
            RegistrationTransactionStatus::Committed as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Drop every staged callback. Already committed Horizon state is never
    /// changed by this operation.
    pub fn rollback(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
        let _ = self.status.compare_exchange(
            RegistrationTransactionStatus::Pending as u8,
            RegistrationTransactionStatus::RolledBack as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn record(&self, event: NativeCallback) -> bool {
        if self.status() != RegistrationTransactionStatus::Pending {
            return false;
        }
        self.events
            .lock()
            .map(|mut events| {
                events.push(event);
            })
            .is_ok()
    }
}

thread_local! {
    /// NativeBridge's existing JNI signatures have no transaction argument.
    /// The synchronous entrypoint call scopes its explicit transaction on the
    /// calling thread; callbacks on any other thread have no owner and are
    /// rejected instead of being attributed to another mod.
    static ACTIVE_TRANSACTION: RefCell<Option<RegistrationTransaction>> = const { RefCell::new(None) };
}

fn stage_event(ev: NativeCallback) -> bool {
    ACTIVE_TRANSACTION.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_some_and(|transaction| transaction.record(ev))
    })
}

fn push(env: &mut JNIEnv, ev: NativeCallback) {
    if !stage_event(ev) {
        log::error!("J2H: rejected an unowned or inactive NativeBridge callback");
        if let Err(error) = env.throw_new(
            "java/lang/IllegalStateException",
            "NativeBridge registration requires an active mod initialization transaction",
        ) {
            log::error!("J2H: could not report rejected NativeBridge callback: {error}");
        }
    }
}

struct RestoreTransaction(Option<RegistrationTransaction>);

impl Drop for RestoreTransaction {
    fn drop(&mut self) {
        ACTIVE_TRANSACTION.with(|active| {
            active.replace(self.0.take());
        });
    }
}

pub struct Bridge {
    classes: Vec<String>,
}

/// The bridge classes J2H registers natives on. The first is the stable
/// compatibility runtime; the second keeps the milestone test mods working.
pub const DEFAULT_BRIDGE_CLASSES: &[&str] = &[
    "dev/horizon/j2h/NativeBridge",
    "com/example/testmod/NativeBridge",
];

impl Bridge {
    pub fn new() -> Self {
        Self {
            classes: DEFAULT_BRIDGE_CLASSES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }

    pub fn with_classes(classes: impl IntoIterator<Item = String>) -> Self {
        Self {
            classes: classes.into_iter().collect(),
        }
    }

    /// Run one synchronous Java entrypoint under its explicit registration
    /// transaction. The previous context is restored even if Rust unwinds.
    pub fn with_transaction<T>(
        &self,
        transaction: &RegistrationTransaction,
        f: impl FnOnce() -> T,
    ) -> T {
        let previous = ACTIVE_TRANSACTION.with(|active| active.replace(Some(transaction.clone())));
        let _restore = RestoreTransaction(previous);
        f()
    }

    /// Register the native methods on every bridge class that loads. A class
    /// that is not in the classpath is skipped: a content-only mod need not
    /// carry the bridge.
    pub fn register_natives(&self, jvm: &JvmRuntime) -> Result<usize, BridgeError> {
        let mut registered = 0;
        for class_name in &self.classes {
            if !jvm.has_class(class_name)? {
                debug!("J2H: bridge class {class_name} not present");
                continue;
            }
            jvm.with_env(|env| {
                let class = env.find_class(class_name.as_str())?;
                let natives = native_methods_for(class_name);
                env.register_native_methods(class, &natives)?;
                Ok(())
            })?;
            registered += 1;
        }
        if registered == 0 {
            info!(
                "J2H: no bridge class found (looked for {})",
                self.classes.join(", ")
            );
        } else {
            info!("J2H: bridge natives registered on {registered} class(es)");
        }
        Ok(registered)
    }
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

fn native_methods_for(class_name: &str) -> Vec<jni::NativeMethod> {
    if class_name == "com/example/testmod/NativeBridge" {
        // The milestone jars predate the extended bridge: their class only
        // declares these four natives.
        return milestone_methods().into_iter().collect();
    }
    native_methods().into_iter().collect()
}

fn milestone_methods() -> [jni::NativeMethod; 4] {
    [
        jni::NativeMethod {
            name: "log".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_example_testmod_NativeBridge_log as *mut _,
        },
        jni::NativeMethod {
            name: "registerBlock".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;FIZ)V".into(),
            fn_ptr: Java_com_example_testmod_NativeBridge_registerBlock as *mut _,
        },
        jni::NativeMethod {
            name: "registerItem".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;I)V".into(),
            fn_ptr: Java_com_example_testmod_NativeBridge_registerItem as *mut _,
        },
        jni::NativeMethod {
            name: "entrypointComplete".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_com_example_testmod_NativeBridge_entrypointComplete as *mut _,
        },
    ]
}

fn native_methods() -> [jni::NativeMethod; 10] {
    [
        jni::NativeMethod {
            name: "log".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_log as *mut _,
        },
        jni::NativeMethod {
            name: "registerBlock".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;FIZ)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerBlock as *mut _,
        },
        jni::NativeMethod {
            name: "registerItem".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;I)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerItem as *mut _,
        },
        jni::NativeMethod {
            name: "registerEntityType".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;F)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerEntityType as *mut _,
        },
        jni::NativeMethod {
            name: "registerMobEffect".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerMobEffect as *mut _,
        },
        jni::NativeMethod {
            name: "registerCreativeTab".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"
                .into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerCreativeTab as *mut _,
        },
        jni::NativeMethod {
            name: "registerMenuType".into(),
            sig: "(Ljava/lang/String;IILjava/lang/String;)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerMenuType as *mut _,
        },
        jni::NativeMethod {
            name: "registerCommand".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_registerCommand as *mut _,
        },
        jni::NativeMethod {
            name: "lifecycleEvent".into(),
            sig: "(Ljava/lang/String;Ljava/lang/String;)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_lifecycleEvent as *mut _,
        },
        jni::NativeMethod {
            name: "entrypointComplete".into(),
            sig: "(Ljava/lang/String;)V".into(),
            fn_ptr: Java_dev_horizon_j2h_NativeBridge_entrypointComplete as *mut _,
        },
    ]
}

fn optional_string(env: &mut JNIEnv, value: JString) -> Option<String> {
    let text = jstring(env, value);
    (!text.is_empty()).then_some(text)
}

fn parse_u16_list(csv: &str) -> Vec<u16> {
    csv.split(',')
        .filter_map(|s| s.trim().parse::<u16>().ok())
        .collect()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_log<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    msg: JString<'local>,
) {
    let text = jstring(&mut env, msg);
    info!("[Java->Rust] log: {text}");
    push(&mut env, NativeCallback::Message(text));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerBlock<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    name: JString<'local>,
    template: JString<'local>,
    hardness: jfloat,
    light: jint,
    with_item: jboolean,
) {
    let id_s = jstring(&mut env, id);
    let name_s = jstring(&mut env, name);
    let template = optional_string(&mut env, template);
    let light = (0..=15).contains(&light).then_some(light as u8);
    info!(
        "[Java->Rust] registerBlock id={id_s} name={name_s} template={template:?} \
         hardness={hardness} light={light:?}"
    );
    push(
        &mut env,
        NativeCallback::RegisterBlock {
            id: id_s,
            name: name_s,
            template,
            hardness: Some(hardness),
            light,
            with_item: with_item != 0,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerItem<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    name: JString<'local>,
    template: JString<'local>,
    max_stack: jint,
) {
    let id_s = jstring(&mut env, id);
    let name_s = jstring(&mut env, name);
    let template = optional_string(&mut env, template);
    let max_stack_size = (1..=99).contains(&max_stack).then_some(max_stack as u8);
    info!(
        "[Java->Rust] registerItem id={id_s} name={name_s} template={template:?} \
         stack={max_stack_size:?}"
    );
    push(
        &mut env,
        NativeCallback::RegisterItem {
            id: id_s,
            name: name_s,
            template,
            max_stack_size,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerEntityType<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    template: JString<'local>,
    item: JString<'local>,
    damage: jfloat,
) {
    let id_s = jstring(&mut env, id);
    let template = optional_string(&mut env, template);
    let item = optional_string(&mut env, item);
    let damage = (damage.is_finite() && damage >= 0.0).then_some(damage);
    info!("[Java->Rust] registerEntityType id={id_s} template={template:?} item={item:?}");
    push(
        &mut env,
        NativeCallback::RegisterEntityType {
            id: id_s,
            template,
            item,
            damage,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerMobEffect<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    name: JString<'local>,
    color: jint,
    category: JString<'local>,
) {
    let id_s = jstring(&mut env, id);
    let name_s = jstring(&mut env, name);
    let category = jstring(&mut env, category);
    let color = u32::try_from(color).unwrap_or(0) & 0xFFFFFF;
    info!("[Java->Rust] registerMobEffect id={id_s} color=#{color:06x} category={category}");
    push(
        &mut env,
        NativeCallback::RegisterMobEffect {
            id: id_s,
            name: name_s,
            color,
            category,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerCreativeTab<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    title: JString<'local>,
    icon: JString<'local>,
    items: JString<'local>,
) {
    let id_s = jstring(&mut env, id);
    let title_s = jstring(&mut env, title);
    let icon_s = jstring(&mut env, icon);
    let items_s = jstring(&mut env, items);
    let items: Vec<String> = items_s
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    info!(
        "[Java->Rust] registerCreativeTab id={id_s} items={}",
        items.len()
    );
    push(
        &mut env,
        NativeCallback::RegisterCreativeTab {
            id: id_s,
            title: title_s,
            icon: icon_s,
            items,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerMenuType<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    slots: jint,
    data_slots: jint,
    results: JString<'local>,
) {
    let id_s = jstring(&mut env, id);
    let results = parse_u16_list(&jstring(&mut env, results));
    info!("[Java->Rust] registerMenuType id={id_s} slots={slots} data={data_slots}");
    push(
        &mut env,
        NativeCallback::RegisterMenuType {
            id: id_s,
            slots: u16::try_from(slots).unwrap_or(0),
            data_slots: u16::try_from(data_slots).unwrap_or(0),
            results,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_registerCommand<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
    description: JString<'local>,
) {
    let name_s = jstring(&mut env, name);
    let description_s = jstring(&mut env, description);
    info!("[Java->Rust] registerCommand {name_s}");
    push(
        &mut env,
        NativeCallback::RegisterCommand {
            name: name_s,
            description: description_s,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_lifecycleEvent<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    name: JString<'local>,
    detail: JString<'local>,
) {
    let name_s = jstring(&mut env, name);
    let detail_s = jstring(&mut env, detail);
    debug!("[Java->Rust] lifecycle {name_s} {detail_s}");
    push(
        &mut env,
        NativeCallback::LifecycleEvent {
            name: name_s,
            detail: detail_s,
        },
    );
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_horizon_j2h_NativeBridge_entrypointComplete<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    mod_id: JString<'local>,
) {
    let id = jstring(&mut env, mod_id);
    info!("[Java->Rust] entrypointComplete {id}");
    push(&mut env, NativeCallback::EntrypointComplete { mod_id: id });
}

// The milestone test jars were compiled against `com.example.testmod.NativeBridge`.
// Registering the same functions under that name keeps them loadable without
// changing the already-built archives.

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_testmod_NativeBridge_log<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    msg: JString<'local>,
) {
    Java_dev_horizon_j2h_NativeBridge_log(env, class, msg)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_testmod_NativeBridge_registerBlock<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    id: JString<'local>,
    name: JString<'local>,
    template: JString<'local>,
    hardness: jfloat,
    light: jint,
    with_item: jboolean,
) {
    Java_dev_horizon_j2h_NativeBridge_registerBlock(
        env, class, id, name, template, hardness, light, with_item,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_testmod_NativeBridge_registerItem<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    id: JString<'local>,
    name: JString<'local>,
    template: JString<'local>,
    max_stack: jint,
) {
    Java_dev_horizon_j2h_NativeBridge_registerItem(env, class, id, name, template, max_stack)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_testmod_NativeBridge_entrypointComplete<'local>(
    env: JNIEnv<'local>,
    class: JClass<'local>,
    mod_id: JString<'local>,
) {
    Java_dev_horizon_j2h_NativeBridge_entrypointComplete(env, class, mod_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_parse_without_panicking() {
        assert_eq!(parse_u16_list("0,3,99999"), vec![0, 3]);
    }

    #[test]
    fn events_are_scoped_to_their_transaction() {
        let bridge = Bridge::new();
        let first = RegistrationTransaction::new("first");
        let second = RegistrationTransaction::new("second");
        bridge.with_transaction(&first, || {
            stage_event(NativeCallback::Message("first".into()))
        });
        bridge.with_transaction(&second, || {
            stage_event(NativeCallback::Message("second".into()))
        });
        assert!(!stage_event(NativeCallback::Message("unowned".into())));
        assert_eq!(first.mod_id(), "first");
        assert!(matches!(first.events().as_slice(), [NativeCallback::Message(s)] if s == "first"));
        assert!(
            matches!(second.events().as_slice(), [NativeCallback::Message(s)] if s == "second")
        );
        first.rollback();
        assert!(first.events().is_empty());
        assert_eq!(first.status(), RegistrationTransactionStatus::RolledBack);
        first.commit();
        assert_eq!(first.status(), RegistrationTransactionStatus::RolledBack);
        assert_eq!(second.events().len(), 1);
        second.commit();
        assert_eq!(second.status(), RegistrationTransactionStatus::Committed);
    }
}
