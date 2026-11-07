package dev.horizon.j2h;

/**
 * A real {@code Minecraft} whose {@code renderFrame} is Horizon's (the bridge
 * singleton is only there for Fabric mods to talk to, not to run a GL frame),
 * whose {@code setScreenAndShow} hands the opened screen to the GUI bridge
 * so Horizon renders it on its own frame, and whose text-focus changes are
 * no-ops because the constructor-free singleton has no GLFW text input.
 *
 * <p>Like {@link GuiBridge.RecordingExtractor}, this is a subclass of the real
 * Minecraft class, never a faked replacement; the methods that would otherwise
 * drive an OpenGL/RenderPearl frame or text input are redirected to Horizon.</p>
 */
public class J2HMinecraft extends net.minecraft.client.Minecraft {
    /** Not called; the singleton is allocated constructor-free. */
    public J2HMinecraft(net.minecraft.client.main.GameConfig config) {
        super(config);
    }

    /** Horizon owns frame generation; the client singleton never runs a GL frame. */
    @Override
    public void renderFrame(boolean framePauseTimeNotAdjust) {
        // no-op: Horizon renders on its own frame in Rust.
    }

    /** Opening a screen stores it for the bridge; Horizon paints it next frame. */
    @Override
    public void setScreenAndShow(net.minecraft.client.gui.screens.Screen screen) {
        GuiBridge.acceptScreen(screen);
    }

    /** Widgets call this on focus changes; the singleton has no real text
     * input manager, and Horizon owns text input. */
    @Override
    public void onTextInputFocusChange(
            net.minecraft.client.gui.components.events.GuiEventListener owner, boolean focused) {
        // no-op
    }

    private static net.minecraft.client.GameNarrator narrator;
    private static NoopSoundManager soundManager;

    private StubTextInputManager textInputStub;

    /** Screens read and write the input-area rect through the singleton; the
     * stub records the rect in memory instead of driving SDL text input. */
    @Override
    public com.mojang.blaze3d.platform.TextInputManager textInputManager() {
        if (textInputStub == null) {
            textInputStub = new StubTextInputManager(getWindow());
        }
        return textInputStub;
    }

    /** Screens narrate during init; the no-op returns the off path, so the
     * allocation never touches the native TTS engine. */
    @Override
    public net.minecraft.client.GameNarrator getNarrator() {
        if (narrator == null) {
            try {
                net.minecraft.client.GameNarrator n = (net.minecraft.client.GameNarrator)
                        J2HUnsafe.allocate(net.minecraft.client.GameNarrator.class);
                java.lang.reflect.Field f = net.minecraft.client.GameNarrator.class
                        .getDeclaredField("narrator");
                f.setAccessible(true);
                f.set(n, NoopNarrator.INSTANCE);
                java.lang.reflect.Field m = net.minecraft.client.GameNarrator.class
                        .getDeclaredField("minecraft");
                m.setAccessible(true);
                m.set(n, this);
                narrator = n;
            } catch (Throwable t) {
                GuiBridge.log("getNarrator: " + t);
                return null;
            }
        }
        return narrator;
    }

    /** GUI button sounds are intentionally silent; Horizon owns audio output. */
    @Override
    public net.minecraft.client.sounds.SoundManager getSoundManager() {
        if (soundManager == null) {
            try {
                soundManager = (NoopSoundManager) J2HUnsafe.allocate(NoopSoundManager.class);
            } catch (Exception e) {
                throw new IllegalStateException("could not install no-audio sound manager", e);
            }
        }
        return soundManager;
    }
}
