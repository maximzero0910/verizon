use java2horizon::{JvmConfig, JvmRuntime};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn library_classpath(dir: &std::path::Path) -> Vec<String> {
    let mut jars = Vec::new();
    fn walk(path: &std::path::Path, jars: &mut Vec<String>) {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, jars);
                } else if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("jar")
                {
                    jars.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
    walk(dir, &mut jars);
    jars.sort();
    jars
}

#[test]
fn ui_debug_harness_requires_loaded_screen_and_input_working() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root.parent().and_then(|p| p.parent()).unwrap();
    let scratch = tempdir().expect("scratch dir for J2H UI harness");
    let classes = scratch.path().join("classes");
    fs::create_dir_all(&classes).expect("class output dir");

    let java_source = r#"
package dev.horizon.j2h;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.EditBox;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.network.chat.Component;

public final class UiDebugHarness {
    private UiDebugHarness() {}

    public static String bridgeLocation() {
        return GuiBridge.class.getProtectionDomain().getCodeSource().getLocation().toString();
    }

    public static boolean verify() {
        try {
            if (Minecraft.getInstance() == null) {
                return false;
            }

            GuiBridge.prepare();
            DebugScreen screen = new DebugScreen();
            GuiBridge.acceptScreen(screen);
            if (!GuiBridge.isOpen()) {
                return false;
            }
            if (!GuiBridge.currentScreen().contains("UiDebugHarness$DebugScreen")) {
                return false;
            }

            GuiBridge.renderFrame(120, 120, 0.0f);

            screen.search.setValue("stone");
            GuiBridge.mouseClicked(30.0, 70.0, 0, 0);
            if (!screen.search.isFocused()) {
                return false;
            }
            screen.search.moveCursorToEnd(false);
            GuiBridge.keyPressed(259, 0, 0);
            GuiBridge.charTyped('x');

            if (!"stonx".equals(screen.search.getValue())) {
                System.err.println("[j2h-test] expected search=stonx actual=" + screen.search.getValue());
                return false;
            }
            screen.search.moveCursorToEnd(false);
            GuiBridge.keyPressed(263, 0, 1);
            GuiBridge.keyPressed(259, 0, 1);
            if (!"ston".equals(screen.search.getValue())) {
                System.err.println("[j2h-test] shifted Backspace did not delete the selection: " + screen.search.getValue());
                return false;
            }
            GuiBridge.charTyped('x');

            GuiBridge.mouseClicked(50.0, 110.0, 0, 0);
            if (!screen.platePressed) {
                System.err.println("[j2h-test] plate button click was not delivered");
                return false;
            }
            GuiBridge.mouseDragged(50.0, 110.0, 5.0, 0.0, 0, 0);
            if (!screen.dragReceived) {
                System.err.println("[j2h-test] mouse drag was not delivered");
                return false;
            }

            if (!screen.buttonText.equals("Stone plate")) {
                System.err.println("[j2h-test] unexpected button label=" + screen.buttonText);
                return false;
            }

            GuiBridge.resize(960, 540);
            if (screen.width != 960 || screen.height != 540) {
                System.err.println("[j2h-test] resize did not rebuild the screen: " + screen.width + "x" + screen.height);
                return false;
            }

            GuiBridge.keyPressed(256, 0, 0);
            if (GuiBridge.isOpen()) {
                System.err.println("[j2h-test] Escape did not close the screen");
                return false;
            }
            GuiBridge.acceptScreen(screen);
            if (!GuiBridge.isOpen()) {
                System.err.println("[j2h-test] screen did not reopen");
                return false;
            }
            GuiBridge.close();
            if (GuiBridge.isOpen()) {
                return false;
            }

            return true;
        } catch (Throwable t) {
            t.printStackTrace();
            return false;
        }
    }

    private static final class DebugScreen extends Screen {
        private EditBox search;
        private final String buttonText = "Stone plate";
        private boolean platePressed;
        private boolean dragReceived;

        private DebugScreen() {
            super(Component.literal("J2H UI debug"));
        }

        @Override
        protected void init() {
            this.search = new EditBox(this.font, 24, 64, 220, 20, Component.literal("Search"));
            this.search.setMaxLength(64);
            this.search.setHint(Component.literal("Search"));
            this.search.setValue("stone");
            this.addRenderableWidget(this.search);
                this.addRenderableWidget(Button.builder(Component.literal(buttonText), button -> platePressed = true)
                    .bounds(24, 100, 200, 20)
                    .build());
                this.addRenderableWidget(Button.builder(Component.literal("Bottom row"), button -> {})
                    .bounds(24, 680, 200, 20)
                    .build());
        }

        @Override
        public boolean mouseDragged(MouseButtonEvent event, double dx, double dy) {
            this.dragReceived = true;
            return true;
        }

        @Override
        public void extractRenderState(GuiGraphicsExtractor graphics, int mouseX, int mouseY, float delta) {
            super.extractRenderState(graphics, mouseX, mouseY, delta);
            graphics.enableScissor(0, 200, 100, 300);
            graphics.fill(0, 200, 100, 300, 0xFFFFFFFF);
            graphics.enableScissor(10, 220, 90, 280);
            graphics.fill(10, 220, 90, 280, 0xFFFFFFFF);
            graphics.disableScissor();
            graphics.fill(0, 280, 100, 300, 0xFFFFFFFF);
            graphics.disableScissor();
        }
    }
}
"#;

    let java_file = classes.join("UiDebugHarness.java");
    fs::write(&java_file, java_source).expect("source file");
    let bridge_source = crate_root.join("bridge-java/dev/horizon/j2h/GuiBridge.java");
    let minecraft_bootstrap_source =
        crate_root.join("bridge-java/dev/horizon/j2h/MinecraftBootstrap.java");
    let minecraft_source = crate_root.join("bridge-java/dev/horizon/j2h/J2HMinecraft.java");
    let gui_source = crate_root.join("bridge-java/dev/horizon/j2h/J2HGui.java");
    let no_sound_source = crate_root.join("bridge-java/dev/horizon/j2h/NoopSoundManager.java");
    let tick_events_file = classes.join("ClientTickEvents.java");
    fs::write(
        &tick_events_file,
        r#"package net.fabricmc.fabric.api.client.event.lifecycle.v1;

import net.minecraft.client.Minecraft;

public final class ClientTickEvents {
    public static final Event END_CLIENT_TICK = new Event();

    public static final class Event {
        public Invoker invoker() {
            return minecraft -> {};
        }
    }

    @FunctionalInterface
    public interface Invoker {
        void onEndTick(Minecraft minecraft);
    }
}
"#,
    )
    .expect("Fabric tick API compile shim");

    let client_jar = repo_root.join(".local/minecraft/26.3/jars/client.jar");
    let runtime_jar = crate_root.join("test_resources/j2h-runtime.jar");
    let library_jars = library_classpath(&repo_root.join(".local/minecraft/26.3/libraries"));
    let fabric_dir = scratch.path().join("fabric-libraries");
    let fabric_jars = java2horizon::fabric::libraries::ensure_libraries(&fabric_dir, &[])
        .expect("Fabric Loader dependencies resolved for J2H bridge");
    let classpath = [
        vec![client_jar.to_string_lossy().to_string()],
        library_jars,
        fabric_jars
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        vec![runtime_jar.to_string_lossy().to_string()],
    ]
    .concat()
    .join(":");

    let javac = Command::new("javac")
        .arg("--release")
        .arg("17")
        .arg("-classpath")
        .arg(&classpath)
        .arg("-d")
        .arg(&classes)
        .arg(&java_file)
        .arg(&bridge_source)
        .arg(&minecraft_bootstrap_source)
        .arg(&minecraft_source)
        .arg(&gui_source)
        .arg(&no_sound_source)
        .arg(&tick_events_file)
        .status()
        .expect("javac available");
    assert!(javac.success(), "javac failed to compile Debug UI harness");

    let rebuild_runtime = std::env::var_os("J2H_REBUILD_RUNTIME").is_some();
    if rebuild_runtime {
        let update_root = scratch.path().join("runtime-update");
        let package = update_root.join("dev/horizon/j2h");
        fs::create_dir_all(&package).expect("runtime update package");
        for entry in fs::read_dir(classes.join("dev/horizon/j2h")).expect("compiled bridge classes")
        {
            let entry = entry.expect("compiled bridge class entry");
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "GuiBridge.class"
                || name.starts_with("GuiBridge$")
                || matches!(
                    name.as_ref(),
                    "MinecraftBootstrap.class"
                        | "J2HMinecraft.class"
                        | "J2HGui.class"
                        | "NoopSoundManager.class"
                )
            {
                fs::copy(entry.path(), package.join(name.as_ref())).expect("copy runtime class");
            }
        }
        let update = Command::new("jar")
            .arg("--update")
            .arg("--file")
            .arg(&runtime_jar)
            .arg("-C")
            .arg(&update_root)
            .arg("dev/horizon/j2h")
            .status()
            .expect("jar available");
        assert!(
            update.success(),
            "failed to update embedded J2H runtime jar"
        );
    }

    let mut cp = if rebuild_runtime {
        vec![runtime_jar.clone(), classes.clone(), client_jar]
    } else {
        vec![classes.clone(), runtime_jar.clone(), client_jar]
    };
    cp.extend(
        library_classpath(&repo_root.join(".local/minecraft/26.3/libraries"))
            .into_iter()
            .map(PathBuf::from),
    );
    cp.extend(fabric_jars);

    let jvm = JvmRuntime::start(JvmConfig::with_classpath(cp)).expect("JVM started for UI harness");
    java2horizon::gui::attach_runtime(&jvm);

    java2horizon::gui::register_natives(&jvm).expect("GUI bridge natives registered");
    let player_name = "J2HProbe";
    let player_uuid = "8b3a1f16-5248-4d01-982c-8c7a1f0db911";
    jvm.call_static_void_with_two_strings(
        "dev/horizon/j2h/MinecraftBootstrap",
        "installWithIdentity",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        player_name,
        player_uuid,
    )
    .expect("Minecraft client singleton bootstrapped with local identity");
    assert!(
        jvm.call_static_bool_with_two_strings(
            "dev/horizon/j2h/MinecraftBootstrap",
            "identityMatches",
            "(Ljava/lang/String;Ljava/lang/String;)Z",
            player_name,
            player_uuid,
        )
        .expect("check seeded Minecraft identity")
    );

    let ok = jvm
        .call_static_bool("dev/horizon/j2h/UiDebugHarness", "verify", "()Z")
        .expect("UI harness verification should not throw");

    if rebuild_runtime {
        let source = jvm
            .call_static_string(
                "dev/horizon/j2h/UiDebugHarness",
                "bridgeLocation",
                "()Ljava/lang/String;",
            )
            .expect("read loaded bridge code source");
        assert!(
            source.contains("j2h-runtime.jar"),
            "updated bridge did not load from the embedded runtime: {source}"
        );
    }

    assert!(
        ok,
        "UI debug harness failed: screen never opened or the input pipeline broke"
    );
    let commands = java2horizon::gui::drain();
    let rendered_text = commands
        .iter()
        .filter_map(|command| match command {
            java2horizon::GuiCommand::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for expected in ["Stone plate", "stone", "Bottom row"] {
        assert!(
            rendered_text.iter().any(|text| text.contains(expected)),
            "rendered UI is missing expected text {expected:?}; got {rendered_text:?}"
        );
    }
    let scissor_starts = commands
        .iter()
        .filter(|command| matches!(command, java2horizon::GuiCommand::Scissor { .. }))
        .count();
    let scissor_ends = commands
        .iter()
        .filter(|command| matches!(command, java2horizon::GuiCommand::ScissorEnd))
        .count();
    assert_eq!(
        scissor_starts, 3,
        "nested scissor should emit outer, inner, and restored parent scopes"
    );
    assert_eq!(
        scissor_ends, 3,
        "every nested scissor scope should be closed"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            java2horizon::GuiCommand::Text { text, y, .. }
                if text.contains("Bottom row") && *y >= 680.0
        )),
        "bottom-row text was not emitted at its expected GUI coordinate"
    );
}
