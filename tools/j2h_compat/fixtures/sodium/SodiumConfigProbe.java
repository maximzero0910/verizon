package fixture.sodium;

/** Live round-trip assertion for Sodium's isolated configuration file. */
public final class SodiumConfigProbe {
    private SodiumConfigProbe() {
    }

    public static void onInitialize() {
        System.err.println("J2H_COMPAT_PROBE_ENTERED:sodium_config_probe");
        try {
            Class<?> optionsType = Class.forName("net.caffeinemc.mods.sodium.client.gui.SodiumOptions");
            Class<?> sodiumClientType = Class.forName("net.caffeinemc.mods.sodium.client.SodiumClientMod");
            Object initializedOptions = sodiumClientType.getMethod("options").invoke(null);
            Object initializedQuality = optionsType.getField("quality").get(initializedOptions);
            boolean hiddenFluidCulling = initializedQuality.getClass()
                    .getField("hiddenFluidCulling").getBoolean(initializedQuality);
            if (hiddenFluidCulling) {
                throw new IllegalStateException("Sodium client entrypoint did not load the preseeded config override");
            }
            System.err.println("J2H_COMPAT_ASSERTION:sodium_startup_override:hiddenFluidCulling=false");

            Object options = optionsType.getMethod("loadFromDisk").invoke(null);
            Object performance = optionsType.getField("performance").get(options);
            Class<?> performanceType = performance.getClass();
            var option = performanceType.getField("animateOnlyVisibleTextures");
            boolean original = option.getBoolean(performance);
            try {
                option.setBoolean(performance, !original);
                optionsType.getMethod("writeToDisk", optionsType).invoke(null, options);

                Object reloaded = optionsType.getMethod("loadFromDisk").invoke(null);
                Object reloadedPerformance = optionsType.getField("performance").get(reloaded);
                if (option.getBoolean(reloadedPerformance) != !original) {
                    throw new IllegalStateException("Sodium option did not survive a disk round trip");
                }
            } finally {
                option.setBoolean(performance, original);
                optionsType.getMethod("writeToDisk", optionsType).invoke(null, options);
            }

            Object restored = optionsType.getMethod("loadFromDisk").invoke(null);
            Object restoredPerformance = optionsType.getField("performance").get(restored);
            if (option.getBoolean(restoredPerformance) != original) {
                throw new IllegalStateException("Sodium option value was not restored after the probe");
            }
            System.err.println("J2H_COMPAT_ASSERTION:sodium_config_roundtrip:restored=true");
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("could not exercise Sodium's pinned config persistence API", e);
        }
    }
}
