//! The Java bridge's real Minecraft screens (`Screen::JavaMods`): the JVM runs
//! the unmodified Fabric GUI (Mod Menu's `ModsScreen`), and Horizon paints the
//! extractor commands that `GuiGraphicsExtractor` emits.
//!
//! This is the terminal render pass only: screen construction, widget layout,
//! focus, scrolling and input all happen in real Minecraft code via
//! `java2horizon::gui::GuiBridge`. Here the extracted `GuiCommand`s become
//! `HudBatch` quads / text using Horizon's existing GUI atlas and font.

use horizon_client::hud::{HudAtlas, HudBatch};
use java2horizon::GuiCommand;
use java2horizon::gui;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct JavaModIcons {
    slots: HashMap<String, usize>,
    missing: HashSet<String>,
}

static JAVA_MOD_ICONS: OnceLock<Mutex<JavaModIcons>> = OnceLock::new();

fn java_mod_icons() -> &'static Mutex<JavaModIcons> {
    JAVA_MOD_ICONS.get_or_init(|| Mutex::new(JavaModIcons::default()))
}

/// Convert Minecraft's ARGB colour to Horizon's ABGR `u32`.
fn abgr(argb: u32) -> u32 {
    let a = (argb >> 24) & 0xFF;
    let r = (argb >> 16) & 0xFF;
    let g = (argb >> 8) & 0xFF;
    let b = argb & 0xFF;
    (a << 24) | (b << 16) | (g << 8) | r
}

/// The Horizon atlas key for a GUI atlas sprite (`blitSprite`). Vanilla and mod
/// GUI sprites live under `textures/gui/sprites/`, packed relative to
/// `textures/`: `minecraft:widget/button` → `gui/sprites/widget/button`.
fn sprite_key(id: &str) -> String {
    let path = id.split_once(':').map_or(id, |(_, p)| p);
    format!("gui/sprites/{path}")
}

/// The Horizon atlas key for a raw texture blit. The id is a texture path
/// (`minecraft:textures/gui/menu_list_background.png`), packed relative to
/// `textures/` without the extension: → `gui/menu_list_background`.
fn texture_key(id: &str) -> String {
    let path = id.split_once(':').map_or(id, |(_, p)| p);
    let path = path.strip_prefix("textures/").unwrap_or(path);
    path.strip_suffix(".png").unwrap_or(path).to_owned()
}

impl super::Game<'_> {
    /// Whether the Java bridge has a screen to host.
    pub(super) fn java_gui_active(&self) -> bool {
        self.screen == super::Screen::JavaMods
    }

    /// Open Mod Menu through its registered keybinding (the real Fabric
    /// client tick handler reacts), then start hosting it.
    pub(super) fn open_java_mods_screen(&mut self) {
        if !crate::game::java_mods::runtime_enabled(false) {
            return;
        }
        // The GuiBridge must have prepared the singleton (font, window) before
        // the tick handler constructs a screen.
        let _ = gui::prepare();
        let opened = gui::open_by_key("modmenu");
        if !opened {
            log::warn!("Horizon: Mod Menu keybinding did not open a Java screen");
        }
        if opened {
            self.java_gui_previous_screen = Some(self.screen);
            self.screen = super::Screen::JavaMods;
            self.capture(false);
            gui::resize(self.gui_width(), self.gui_height());
        }
    }

    /// Close the bridge screen and return to the world.
    pub(super) fn close_java_mods_screen(&mut self) {
        let _ = gui::close();
        if self.screen == super::Screen::JavaMods {
            self.screen = self
                .java_gui_previous_screen
                .take()
                .unwrap_or(super::Screen::Playing);
        }
        self.capture(self.screen == super::Screen::Playing);
    }

    fn gui_width(&self) -> i32 {
        self.gui_size().0 as i32
    }
    fn gui_height(&self) -> i32 {
        self.gui_size().1 as i32
    }

    /// Draw the hosted screen: tick and ask the JVM to render the real screen,
    /// then paint the extracted commands.
    pub(super) fn draw_java_mods(&mut self, batch: &mut HudBatch) {
        self.tick_java_mods();
        let atlas = &self.gui.atlas;
        // The translucent menu background (menus dim the world underneath).
        batch.fill(atlas, 0.0, 0.0, batch.width, batch.height, 0xF0001010);
        let (mx, my) = self.cursor;
        let partial = self.clock.fraction();
        if let Err(e) = gui::render_frame(mx as i32, my as i32, partial as f32) {
            log::warn!("Java mod screen render failed: {e:?}");
        }
        let mut icons = java_mod_icons().lock().unwrap_or_else(|e| e.into_inner());
        upload_java_mod_icons(batch, atlas, &mut icons);
        let commands = gui::drain();
        paint_commands(batch, atlas, &commands, &mut icons);
    }

    /// Forward a key press/release to the real screen.
    pub(super) fn j2h_key(&mut self, key: winit::keyboard::KeyCode, pressed: bool) {
        let code = winit_keycode_to_glfw(key);
        let mods = super::glfw_modifiers(self.modifiers) as i32;
        if pressed {
            gui::key_pressed(code, 0, mods);
        } else {
            gui::key_released(code, 0, mods);
        }
    }

    /// Forward typed text to the real screen (search fields).
    pub(super) fn j2h_char(&mut self, ch: char) {
        gui::char_typed(ch as u32 as i32);
    }

    /// The bridge screen tick (screen.tick plus Fabric client-tick listeners).
    pub(super) fn tick_java_mods(&mut self) {
        let _ = gui::tick();
    }

    /// Whether a JVM with the GUI bridge is running, so a key can open a real
    /// Minecraft screen.
    pub(super) fn has_java_gui(&self) -> bool {
        crate::game::java_mods::runtime_enabled(false) && java2horizon::gui::runtime().is_some()
    }

    /// A window event while the Java screen is open: forward mouse and keyboard
    /// to the real Minecraft screen and stop the rest of the game from eating it.
    pub(super) fn java_screen_event(&mut self, event: &winit::event::WindowEvent) {
        use winit::event::{ElementState, MouseScrollDelta, WindowEvent as W};
        use winit::keyboard::PhysicalKey;
        match event {
            W::CursorMoved { position, .. } => {
                let previous = self.cursor;
                if let Some(r) = &self.renderer {
                    let b = HudBatch::new([r.config.width, r.config.height]);
                    self.cursor = (position.x as f32 / b.scale, position.y as f32 / b.scale);
                }
                gui::mouse_moved(self.cursor.0 as f64, self.cursor.1 as f64);
                if let Some(button) = self
                    .drag_button
                    .filter(|button| self.physical_buttons.contains(button))
                {
                    gui::mouse_dragged(
                        f64::from(self.cursor.0),
                        f64::from(self.cursor.1),
                        f64::from(self.cursor.0 - previous.0),
                        f64::from(self.cursor.1 - previous.1),
                        j2h_mouse_button(button),
                        super::glfw_modifiers(self.modifiers) as i32,
                    );
                }
            }
            W::MouseInput { state, button, .. } => {
                self.drag_button = (*state == ElementState::Pressed).then_some(*button);
                let id = j2h_mouse_button(*button);
                let (x, y) = self.cursor;
                let mods = super::glfw_modifiers(self.modifiers) as i32;
                if *state == ElementState::Pressed {
                    gui::mouse_clicked(f64::from(x), f64::from(y), id, mods);
                } else {
                    gui::mouse_released(f64::from(x), f64::from(y), id, mods);
                }
            }
            W::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y * 20.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                let (x, y) = self.cursor;
                gui::mouse_scrolled(f64::from(x), f64::from(y), 0.0, f64::from(dy));
            }
            W::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(key) = event.physical_key {
                    self.j2h_key(key, pressed);
                }
                // Vanilla's GLFW char callback fires after the key callback, on
                // press, for every text-producing key. It is independent of the
                // physical key: `A` is both Code(KeyA) and the character "a".
                // Text fields (EditBox) insert only through charTyped; deletion
                // keys are handled by the real screen's keyPressed path above.
                if pressed && let Some(text) = event.text.as_deref() {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        self.j2h_char(ch);
                    }
                }
            }
            _ => {}
        }
        // A screen can close itself (Escape → Screen.onClose → setScreen(null),
        // a mod's own close button, …). When it does, Horizon must stop hosting
        // it, or it keeps painting an empty screen and eating gameplay input.
        if self.screen == super::Screen::JavaMods && !gui::is_open() {
            self.close_java_mods_screen();
        }
    }
}

fn j2h_mouse_button(button: winit::event::MouseButton) -> i32 {
    match button {
        winit::event::MouseButton::Left => 0,
        winit::event::MouseButton::Right => 1,
        winit::event::MouseButton::Middle => 2,
        winit::event::MouseButton::Other(n) => n as i32,
        winit::event::MouseButton::Back => 3,
        winit::event::MouseButton::Forward => 4,
    }
}

/// Paint the extracted GUI commands in order, honouring the scissor stack.
fn paint_commands(
    batch: &mut HudBatch,
    atlas: &HudAtlas,
    commands: &[GuiCommand],
    icons: &mut JavaModIcons,
) {
    // The current scissor: (vertex start, rect) so content pushed between a
    // Scissor and its ScissorEnd is clipped to that rect.
    let mut scissors: Vec<(usize, [f32; 4])> = Vec::new();
    for command in commands {
        match command {
            GuiCommand::Fill {
                x0,
                y0,
                x1,
                y1,
                color,
            } => {
                batch.fill(atlas, *x0, *y0, x1 - x0, y1 - y0, abgr(*color));
            }
            GuiCommand::FillGradient {
                x0,
                y0,
                x1,
                y1,
                top,
                bottom,
            } => {
                batch.fill_gradient(
                    atlas,
                    (*x0, *y0, x1 - x0, y1 - y0),
                    abgr(*top),
                    abgr(*bottom),
                );
            }
            GuiCommand::Text {
                x,
                y,
                color,
                shadow,
                text,
            } => {
                batch.text(atlas, text, *x, *y, abgr(*color), *shadow);
            }
            GuiCommand::Sprite { id, x, y, w, h } => {
                let key = sprite_key(id);
                horizon_client::ui::blit_sprite(batch, atlas, &key, *x, *y, *w, *h, u32::MAX);
            }
            GuiCommand::Blit {
                id,
                x,
                y,
                w,
                h,
                u0,
                v0,
                u1,
                v1,
            } => {
                let key = texture_key(id);
                if let Some(r) = atlas.sprites.get(&key) {
                    // The command's u/v are tiling phase (normalised by the
                    // texture's repeat size), not a sub-region of the atlas
                    // sprite, so draw the whole sprite over the destination.
                    batch.rect_uv(*x, *y, *w, *h, *r, u32::MAX);
                } else if let Some(slot) = icons.slots.get(id) {
                    let key = format!("horizon/java-mod-icon{slot}");
                    if let Some(mut r) = atlas.sprites.get(&key).copied() {
                        let du = r.u1 - r.u0;
                        let dv = r.v1 - r.v0;
                        let left = r.u0;
                        let top = r.v0;
                        r.u0 = left + du * u0;
                        r.u1 = left + du * u1;
                        r.v0 = top + dv * v0;
                        r.v1 = top + dv * v1;
                        batch.rect_uv(*x, *y, *w, *h, r, u32::MAX);
                    }
                } else if icons.missing.insert(id.clone()) {
                    log::warn!("Horizon: Java GUI texture {id} has no transferred pixels");
                }
            }
            GuiCommand::TextureImage { .. } => {}
            GuiCommand::Scissor { x0, y0, x1, y1 } => {
                scissors.push((batch.vertices.len(), [*x0, *y0, *x1, *y1]));
            }
            GuiCommand::ScissorEnd => {
                if let Some((start, [x0, y0, x1, y1])) = scissors.pop() {
                    batch.clip_since(start, x0, y0, x1, y1);
                }
            }
            GuiCommand::Tooltip { lines, x, y } => {
                let mut width = 0.0f32;
                let mut n = 0;
                let max = *y;
                let rows: Vec<&str> = lines.split('\n').filter(|s| !s.is_empty()).collect();
                for line in &rows {
                    width = width.max(HudBatch::text_width(atlas, line));
                    n += 1;
                }
                if n == 0 {
                    continue;
                }
                let height = n as f32 * 9.0 + 4.0;
                let (box_x, box_y, bw, bh) =
                    clamp_tooltip(*x, *y, width, height, batch.width, batch.height);
                batch.fill(atlas, box_x, box_y, bw, bh, 0xF0101010);
                batch.fill(atlas, box_x, box_y, bw, 1.0, 0xFFFFFFFF);
                batch.fill(atlas, box_x, box_y + bh - 1.0, bw, 1.0, 0xFFFFFFFF);
                batch.fill(atlas, box_x, box_y, 1.0, bh, 0xFFFFFFFF);
                batch.fill(atlas, box_x + bw - 1.0, box_y, 1.0, bh, 0xFFFFFFFF);
                let mut ty = box_y + 2.0;
                for line in &rows {
                    batch.text(atlas, line, box_x + 2.0, ty, 0xFFFFFFFF, true);
                    ty += 9.0;
                }
                let _ = max;
            }
        }
    }
}

fn upload_java_mod_icons(batch: &mut HudBatch, atlas: &HudAtlas, icons: &mut JavaModIcons) {
    for update in gui::take_texture_updates() {
        let GuiCommand::TextureImage {
            id,
            width,
            height,
            rgba,
        } = update
        else {
            continue;
        };
        let slot = if let Some(slot) = icons.slots.get(&id).copied() {
            slot
        } else {
            let slot = icons.slots.len();
            if slot >= horizon_client::hud::JAVA_MOD_ICON_SLOTS {
                log::warn!("Horizon: Java GUI icon slots are full; cannot display texture {id}");
                continue;
            }
            icons.slots.insert(id.clone(), slot);
            slot
        };
        let key = format!("horizon/java-mod-icon{slot}");
        let Some(rect) = atlas.sprites.get(&key) else {
            log::error!("Horizon: reserved Java mod icon atlas slot is missing: {key}");
            continue;
        };
        let Some(rgba) = resize_java_mod_icon(width, height, rgba) else {
            log::warn!("Horizon: invalid Java GUI icon pixels for {id} ({width}x{height})");
            continue;
        };
        batch.patches.push(horizon_client::font::Patch {
            x: (rect.u0 * atlas.width as f32).round() as u32,
            y: (rect.v0 * atlas.height as f32).round() as u32,
            width: horizon_client::hud::JAVA_MOD_ICON_SIZE,
            height: horizon_client::hud::JAVA_MOD_ICON_SIZE,
            rgba,
        });
        icons.missing.remove(&id);
        log::info!("Horizon: Java GUI texture {id} uploaded to atlas slot {slot}");
    }
}

fn resize_java_mod_icon(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let size = horizon_client::hud::JAVA_MOD_ICON_SIZE;
    if width == 0 || height == 0 || rgba.len() != (width * height * 4) as usize {
        return None;
    }
    if width == size && height == size {
        return Some(rgba);
    }
    let mut resized = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let src_y = y * height / size;
        for x in 0..size {
            let src_x = x * width / size;
            let at = ((src_y * width + src_x) * 4) as usize;
            resized.extend_from_slice(&rgba[at..at + 4]);
        }
    }
    Some(resized)
}

#[cfg(test)]
mod tests {
    use super::resize_java_mod_icon;

    #[test]
    fn dynamic_java_icon_pixels_are_resized_to_the_atlas_slot() {
        let pixels = vec![17, 34, 51, 255];
        let resized = resize_java_mod_icon(1, 1, pixels).unwrap();
        assert_eq!(resized.len(), 64 * 64 * 4);
        assert!(
            resized
                .chunks_exact(4)
                .all(|pixel| pixel == [17, 34, 51, 255])
        );
    }

    #[test]
    fn dynamic_java_icon_rejects_mismatched_pixel_buffers() {
        assert!(resize_java_mod_icon(2, 2, vec![0; 15]).is_none());
    }
}

/// Clamp the tooltip box so it fits on screen next to the cursor.
fn clamp_tooltip(x: f32, y: f32, w: f32, h: f32, sw: f32, sh: f32) -> (f32, f32, f32, f32) {
    let pad = 4.0;
    // Prefer the cursor's right; fall back to the cursor's left.
    let mut bx = x;
    if bx + w + pad > sw {
        bx = (x - w - pad).max(0.0);
    }
    let mut by = y - h;
    if by < 0.0 {
        by = (y + pad).min((sh - h).max(0.0));
    }
    (bx, by, w + pad * 2.0, h)
}

/// Best-effort winit → GLFW keycode, for the navigation keys a screen needs
/// (arrows, tab, enter, backspace). Text entry goes through `j2h_char`.
fn winit_keycode_to_glfw(key: winit::keyboard::KeyCode) -> i32 {
    use winit::keyboard::KeyCode as K;
    match key {
        K::Escape => 256,
        K::Enter => 257,
        K::Tab => 258,
        K::Backspace => 259,
        K::Insert => 260,
        K::Delete => 261,
        K::ArrowRight => 262,
        K::ArrowLeft => 263,
        K::ArrowDown => 264,
        K::ArrowUp => 265,
        K::PageUp => 266,
        K::PageDown => 267,
        K::Home => 268,
        K::End => 269,
        K::Space => 32,
        K::KeyA => 65,
        K::KeyB => 66,
        K::KeyC => 67,
        K::KeyD => 68,
        K::KeyE => 69,
        K::KeyF => 70,
        K::KeyG => 71,
        K::KeyH => 72,
        K::KeyI => 73,
        K::KeyJ => 74,
        K::KeyK => 75,
        K::KeyL => 76,
        K::KeyM => 77,
        K::KeyN => 78,
        K::KeyO => 79,
        K::KeyP => 80,
        K::KeyQ => 81,
        K::KeyR => 82,
        K::KeyS => 83,
        K::KeyT => 84,
        K::KeyU => 85,
        K::KeyV => 86,
        K::KeyW => 87,
        K::KeyX => 88,
        K::KeyY => 89,
        K::KeyZ => 90,
        K::Digit0 => 48,
        K::Digit1 => 49,
        K::Digit2 => 50,
        K::Digit3 => 51,
        K::Digit4 => 52,
        K::Digit5 => 53,
        K::Digit6 => 54,
        K::Digit7 => 55,
        K::Digit8 => 56,
        K::Digit9 => 57,
        K::F1 => 290,
        K::F2 => 291,
        K::F3 => 292,
        K::F4 => 293,
        K::F5 => 294,
        K::F6 => 295,
        K::F7 => 296,
        K::F8 => 297,
        K::F9 => 298,
        K::F10 => 299,
        K::F11 => 300,
        K::F12 => 301,
        _ => 0,
    }
}
