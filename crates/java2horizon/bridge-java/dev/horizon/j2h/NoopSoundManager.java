package dev.horizon.j2h;

/**
 * The embedded Minecraft client has no audio device; this real SoundManager
 * subtype keeps vanilla widget clicks functional while deliberately dropping
 * sound playback at the J2H boundary. It is allocated constructor-free because
 * SoundManager's constructor starts the native audio engine.
 */
final class NoopSoundManager extends net.minecraft.client.sounds.SoundManager {
    NoopSoundManager() {
        super(null);
    }

    @Override
    public net.minecraft.client.sounds.SoundEngine.PlayResult play(
            net.minecraft.client.resources.sounds.SoundInstance sound) {
        return net.minecraft.client.sounds.SoundEngine.PlayResult.NOT_STARTED;
    }
}
