package fixture.jade;

import dev.horizon.j2h.NativeBridge;

/** Live assertion for the Minecraft registry entry created by the real Jade initializer. */
public final class JadeRuleProbe {
    private JadeRuleProbe() {
    }

    public static void onInitialize() {
        System.err.println("J2H_COMPAT_PROBE_ENTERED:jade_rule_probe");
        try {
            Class<?> builtInRegistries = Class.forName("net.minecraft.core.registries.BuiltInRegistries");
            Object gameRuleRegistry = builtInRegistries.getField("GAME_RULE").get(null);

            Class<?> identifierType = Class.forName("net.minecraft.resources.Identifier");
            Object key = identifierType.getMethod("parse", String.class).invoke(null, "jade:max_position_deviation");

            Class<?> registryType = Class.forName("net.minecraft.core.Registry");
            Object gameRule = registryType.getMethod("getValue", identifierType).invoke(gameRuleRegistry, key);
            if (gameRule == null) {
                throw new IllegalStateException("jade:max_position_deviation is absent from GAME_RULE");
            }

            Class<?> gameRuleType = Class.forName("net.minecraft.world.level.gamerules.GameRule");
            Object defaultValue = gameRuleType.getMethod("defaultValue").invoke(gameRule);
            if (!Integer.valueOf(21).equals(defaultValue)) {
                throw new IllegalStateException("unexpected Jade rule default: " + defaultValue);
            }

            System.err.println("J2H_COMPAT_ASSERTION:jade_rule_registered:default=21");
            NativeBridge.log("J2H_COMPAT_ASSERTION:jade_rule_registered:default=21");
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("could not inspect Jade's registered game rule", e);
        }
    }
}
