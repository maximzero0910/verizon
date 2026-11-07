package fixture.optional;

import dev.horizon.j2h.NativeBridge;

public final class OptionalMod {
    public static void onInitialize() {
        NativeBridge.registerBlock("optionalmod:survives", "Optional", "minecraft:stone", 1.0f, 0, true);
    }
}
