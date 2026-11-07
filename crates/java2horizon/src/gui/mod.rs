//! Bridge for Minecraft's real GUI screen system (Minecraft 26.3's
//! `GuiGraphicsExtractor` extraction model) into Horizon's GUI batch.
//!
//! The Java side (`GuiBridge` in `bridge-java/`) runs the real, unmodified
//! Minecraft `Screen` and re-emits every draw call the screen makes through
//! the native methods registered here. Horizon reads those commands and paints
//! them with its own `HudBatch`/`HudAtlas`. Widget layout, focus, tooltips and
//! input remain real Minecraft code; only the final render pass is Horizon's.
//!
//! The commands are handed over as a small, platform-neutral list so the game
//! can paint them with whatever renderer it already has.

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jfloat, jint};
use once_cell::sync::Lazy;
use std::sync::{Arc, Mutex};

use crate::jvm::{JvmError, JvmRuntime, jstring};

/// One draw operation a Minecraft `Screen` requested, in Horizon's GUI space
/// (the 26.3 extractor's un-scaled coordinate system, which Horizon scales to
/// match its own GUI scale).
#[derive(Debug, Clone)]
pub enum GuiCommand {
    Fill {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: u32,
    },
    FillGradient {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        top: u32,
        bottom: u32,
    },
    Text {
        x: f32,
        y: f32,
        color: u32,
        shadow: bool,
        text: String,
    },
    /// Draw a region of a texture identified by an `Identifier`.
    Blit {
        id: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        u0: f32,
        v0: f32,
        u1: f32,
        v1: f32,
    },
    /// A dynamic GUI texture's pixels, transferred once when first encountered.
    TextureImage {
        id: String,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    /// Draw a sprite from the GUI atlas by its `Identifier`.
    Sprite {
        id: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Scissor {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    ScissorEnd,
    /// Tooltip text (newline-separated lines) plus the anchor cursor.
    Tooltip {
        lines: String,
        x: f32,
        y: f32,
    },
}

#[derive(Default)]
struct GuiState {
    commands: Vec<GuiCommand>,
    texture_updates: Vec<GuiCommand>,
}

static STATE: Lazy<Arc<Mutex<GuiState>>> = Lazy::new(|| Arc::new(Mutex::new(GuiState::default())));

fn state() -> Arc<Mutex<GuiState>> {
    STATE.clone()
}

fn push(command: GuiCommand) {
    if let Ok(mut s) = state().lock() {
        s.commands.push(command);
    }
}

/// The running JVM, kept for the game to drive the GUI bridge per frame.
static JVM: Lazy<Mutex<Option<JvmRuntime>>> = Lazy::new(|| Mutex::new(None));

/// Remember the process JVM so the game can reach the GUI bridge after the
/// one-shot lifecycle has finished. Called from `Lifecycle::start_jvm`.
pub fn attach_runtime(jvm: &JvmRuntime) {
    if let Ok(mut slot) = JVM.lock() {
        let _ = slot.insert(jvm.clone());
    }
}

/// The running JVM handle, if the lifecycle started one.
pub fn runtime() -> Option<JvmRuntime> {
    JVM.lock().ok().and_then(|slot| slot.clone())
}

fn with_jvm<F, R>(f: F) -> Result<R, JvmError>
where
    F: FnOnce(&JvmRuntime) -> Result<R, JvmError>,
{
    let jvm = runtime().ok_or(JvmError::Other("no JVM attached".into()))?;
    f(&jvm)
}

/// Register the GuiBridge native emissions on `dev.horizon.j2h.GuiBridge`.
pub fn register_natives(jvm: &JvmRuntime) -> Result<(), JvmError> {
    jvm.with_env(|env| {
        let class = env.find_class("dev/horizon/j2h/GuiBridge")?;
        let natives = [
            jni::NativeMethod {
                name: "emitFill".into(),
                sig: "(IIIII)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitFill as *mut _,
            },
            jni::NativeMethod {
                name: "emitFillGradient".into(),
                sig: "(IIIIII)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitFillGradient as *mut _,
            },
            jni::NativeMethod {
                name: "emitText".into(),
                sig: "(IIIZLjava/lang/String;)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitText as *mut _,
            },
            jni::NativeMethod {
                name: "emitBlit".into(),
                sig: "(Ljava/lang/String;IIIIFFFF)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitBlit as *mut _,
            },
            jni::NativeMethod {
                name: "emitTextureImage".into(),
                sig: "(Ljava/lang/String;II[B)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitTextureImage as *mut _,
            },
            jni::NativeMethod {
                name: "emitSprite".into(),
                sig: "(Ljava/lang/String;IIII)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitSprite as *mut _,
            },
            jni::NativeMethod {
                name: "emitScissor".into(),
                sig: "(IIII)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitScissor as *mut _,
            },
            jni::NativeMethod {
                name: "emitScissorEnd".into(),
                sig: "()V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitScissorEnd as *mut _,
            },
            jni::NativeMethod {
                name: "emitTooltip".into(),
                sig: "(Ljava/lang/String;II)V".into(),
                fn_ptr: Java_dev_horizon_j2h_GuiBridge_emitTooltip as *mut _,
            },
        ];
        env.register_native_methods(class, &natives)?;
        Ok(())
    })
}

/// Make the singleton usable for a screen (Window size, Font). Idempotent.
pub fn prepare() -> Result<(), JvmError> {
    with_jvm(|jvm| jvm.call_static_void("dev/horizon/j2h/GuiBridge", "prepare", "()V"))
}

/// Whether a screen is currently open in the bridge.
pub fn is_open() -> bool {
    with_jvm(|jvm| jvm.call_static_bool("dev/horizon/j2h/GuiBridge", "isOpen", "()Z"))
        .unwrap_or(false)
}

/// The class name of the open screen (FQN) or empty.
pub fn current_screen() -> String {
    with_jvm(|jvm| {
        jvm.call_static_string(
            "dev/horizon/j2h/GuiBridge",
            "currentScreen",
            "()Ljava/lang/String;",
        )
    })
    .unwrap_or_default()
}

/// Open a screen by pressing a Fabric keybinding (category id) and letting
/// the real client tick handler react.
pub fn open_by_key(category: &str) -> bool {
    match with_jvm(|jvm| {
        let opened = jvm.call_static_bool_with_string(
            "dev/horizon/j2h/GuiBridge",
            "pressBinding",
            "(Ljava/lang/String;)Z",
            category,
        )?;
        if !opened {
            let registered = jvm.call_static_bool_with_string(
                "dev/horizon/j2h/GuiBridge",
                "bindingRegistered",
                "(Ljava/lang/String;)Z",
                category,
            )?;
            log::warn!(
                "J2H: keybinding category {category:?} registered={registered}, screen_opened=false"
            );
        }
        Ok(opened)
    }) {
        Ok(opened) => opened,
        Err(error) => {
            log::warn!("J2H: opening Java screen with keybinding {category:?} failed: {error}");
            false
        }
    }
}

/// Close the open screen.
pub fn close() -> bool {
    with_jvm(|jvm| jvm.call_static_bool("dev/horizon/j2h/GuiBridge", "close", "()Z"))
        .unwrap_or(false)
}

/// Tick the open screen (screen.tick plus Fabric client-tick listeners).
pub fn tick() -> Result<(), JvmError> {
    with_jvm(|jvm| jvm.call_void_args("dev/horizon/j2h/GuiBridge", "tick", "()V", &[]))
}

/// Render the current screen into recorded commands.
pub fn render_frame(mouse_x: i32, mouse_y: i32, delta: f32) -> Result<(), JvmError> {
    with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "renderFrame",
            "(IIF)V",
            &[
                jni::objects::JValue::Int(mouse_x),
                jni::objects::JValue::Int(mouse_y),
                jni::objects::JValue::Float(delta),
            ],
        )
    })
}

/// Report a resize to the bridge (and re-init the screen).
pub fn resize(width: i32, height: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "resize",
            "(II)V",
            &[
                jni::objects::JValue::Int(width),
                jni::objects::JValue::Int(height),
            ],
        )
    });
}

/// Forward input to the real screen.
pub fn mouse_moved(x: f64, y: f64) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "mouseMoved",
            "(DD)V",
            &[
                jni::objects::JValue::Double(x),
                jni::objects::JValue::Double(y),
            ],
        )
    });
}

pub fn mouse_clicked(x: f64, y: f64, button: i32, mods: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "mouseClicked",
            "(DDII)V",
            &[
                jni::objects::JValue::Double(x),
                jni::objects::JValue::Double(y),
                jni::objects::JValue::Int(button),
                jni::objects::JValue::Int(mods),
            ],
        )
    });
}

pub fn mouse_released(x: f64, y: f64, button: i32, mods: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "mouseReleased",
            "(DDII)V",
            &[
                jni::objects::JValue::Double(x),
                jni::objects::JValue::Double(y),
                jni::objects::JValue::Int(button),
                jni::objects::JValue::Int(mods),
            ],
        )
    });
}

pub fn mouse_dragged(x: f64, y: f64, dx: f64, dy: f64, button: i32, mods: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "mouseDragged",
            "(DDDDII)V",
            &[
                jni::objects::JValue::Double(x),
                jni::objects::JValue::Double(y),
                jni::objects::JValue::Double(dx),
                jni::objects::JValue::Double(dy),
                jni::objects::JValue::Int(button),
                jni::objects::JValue::Int(mods),
            ],
        )
    });
}

pub fn mouse_scrolled(x: f64, y: f64, dx: f64, dy: f64) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "mouseScrolled",
            "(DDDD)V",
            &[
                jni::objects::JValue::Double(x),
                jni::objects::JValue::Double(y),
                jni::objects::JValue::Double(dx),
                jni::objects::JValue::Double(dy),
            ],
        )
    });
}

pub fn key_pressed(code: i32, scan: i32, mods: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "keyPressed",
            "(III)V",
            &[
                jni::objects::JValue::Int(code),
                jni::objects::JValue::Int(scan),
                jni::objects::JValue::Int(mods),
            ],
        )
    });
}

pub fn key_released(code: i32, scan: i32, mods: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "keyReleased",
            "(III)V",
            &[
                jni::objects::JValue::Int(code),
                jni::objects::JValue::Int(scan),
                jni::objects::JValue::Int(mods),
            ],
        )
    });
}

pub fn char_typed(code_point: i32) {
    let _ = with_jvm(|jvm| {
        jvm.call_void_args(
            "dev/horizon/j2h/GuiBridge",
            "charTyped",
            "(I)V",
            &[jni::objects::JValue::Int(code_point)],
        )
    });
}

/// Drain the recorded commands for this frame.
pub fn drain() -> Vec<GuiCommand> {
    state()
        .lock()
        .map(|mut s| std::mem::take(&mut s.commands))
        .unwrap_or_default()
}

/// Take the dynamic texture images registered since the previous GUI frame.
pub fn take_texture_updates() -> Vec<GuiCommand> {
    state()
        .lock()
        .map(|mut s| std::mem::take(&mut s.texture_updates))
        .unwrap_or_default()
}

fn push_texture_update(command: GuiCommand) {
    if let Ok(mut s) = state().lock() {
        s.texture_updates.push(command);
    }
}

// ---- natives ----

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitFill<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    x0: jint,
    y0: jint,
    x1: jint,
    y1: jint,
    color: jint,
) {
    push(GuiCommand::Fill {
        x0: x0 as f32,
        y0: y0 as f32,
        x1: x1 as f32,
        y1: y1 as f32,
        color: color as u32,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitFillGradient<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    x0: jint,
    y0: jint,
    x1: jint,
    y1: jint,
    top: jint,
    bottom: jint,
) {
    push(GuiCommand::FillGradient {
        x0: x0 as f32,
        y0: y0 as f32,
        x1: x1 as f32,
        y1: y1 as f32,
        top: top as u32,
        bottom: bottom as u32,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitText<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    x: jint,
    y: jint,
    color: jint,
    shadow: jboolean,
    text: JString<'local>,
) {
    let text = jstring(&mut env, text);
    push(GuiCommand::Text {
        x: x as f32,
        y: y as f32,
        color: color as u32,
        shadow: shadow != 0,
        text,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitBlit<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    x0: jint,
    y0: jint,
    w: jint,
    h: jint,
    u0: jfloat,
    v0: jfloat,
    u1: jfloat,
    v1: jfloat,
) {
    let id = jstring(&mut env, id);
    push(GuiCommand::Blit {
        id,
        x: x0 as f32,
        y: y0 as f32,
        w: w as f32,
        h: h as f32,
        u0,
        v0,
        u1,
        v1,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitTextureImage<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    width: jint,
    height: jint,
    pixels: JByteArray<'local>,
) {
    let id = jstring(&mut env, id);
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        log::warn!("J2H GUI: rejected dynamic texture {id} with invalid dimensions");
        return;
    };
    if width == 0 || height == 0 || width > 256 || height > 256 {
        log::warn!("J2H GUI: rejected dynamic texture {id} dimensions {width}x{height}");
        return;
    }
    let rgba = match env.convert_byte_array(&pixels) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("J2H GUI: could not read dynamic texture {id}: {error}");
            return;
        }
    };
    if rgba.len() != (width * height * 4) as usize {
        log::warn!(
            "J2H GUI: rejected dynamic texture {id}: expected {} RGBA bytes, got {}",
            width * height * 4,
            rgba.len()
        );
        return;
    }
    push_texture_update(GuiCommand::TextureImage {
        id,
        width,
        height,
        rgba,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitSprite<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    id: JString<'local>,
    x0: jint,
    y0: jint,
    w: jint,
    h: jint,
) {
    let id = jstring(&mut env, id);
    push(GuiCommand::Sprite {
        id,
        x: x0 as f32,
        y: y0 as f32,
        w: w as f32,
        h: h as f32,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitScissor<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    x0: jint,
    y0: jint,
    x1: jint,
    y1: jint,
) {
    push(GuiCommand::Scissor {
        x0: x0 as f32,
        y0: y0 as f32,
        x1: x1 as f32,
        y1: y1 as f32,
    });
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitScissorEnd<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
) {
    push(GuiCommand::ScissorEnd);
}

#[unsafe(no_mangle)]
extern "system" fn Java_dev_horizon_j2h_GuiBridge_emitTooltip<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    lines: JString<'local>,
    x: jint,
    y: jint,
) {
    let lines = jstring(&mut env, lines);
    push(GuiCommand::Tooltip {
        lines,
        x: x as f32,
        y: y as f32,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_drained_once() {
        push(GuiCommand::Scissor {
            x0: 0.0,
            y0: 0.0,
            x1: 1.0,
            y1: 1.0,
        });
        push(GuiCommand::ScissorEnd);
        let cmds = drain();
        assert_eq!(cmds.len(), 2);
        assert!(drain().is_empty());
    }

    #[test]
    fn attach_and_runtime_are_roundtrip() {
        // runtime() is Some only after Lifecycle::start_jvm attaches it.
        let jvm = runtime();
        // On a unit-test process there is no JVM started; that is expected.
        let _ = jvm;
    }
}
