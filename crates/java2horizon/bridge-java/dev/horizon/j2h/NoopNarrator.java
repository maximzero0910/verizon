package dev.horizon.j2h;

/**
 * A {@code com.mojang.text2speech.Narrator} with nothing to say. The
 * constructor-free Minecraft singleton has no working native text-to-speech
 * engine, so narration must report inactive: widgets that check
 * {@code Minecraft.getNarrator().isActive()} (e.g. {@code Screen.init}'s
 * narration pass) take the off path instead of touching native TTS.
 */
public final class NoopNarrator implements com.mojang.text2speech.Narrator {
    public static final NoopNarrator INSTANCE = new NoopNarrator();

    private NoopNarrator() {
    }

    @Override
    public void say(String text, boolean interrupt, float pitch) {
    }

    @Override
    public void clear() {
    }

    @Override
    public boolean active() {
        return false;
    }

    @Override
    public void destroy() {
    }
}
