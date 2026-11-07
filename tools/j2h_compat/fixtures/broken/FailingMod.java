package fixture.broken;

import dev.horizon.j2h.NativeBridge;

public final class FailingMod {
    public static void onInitialize() {
        NativeBridge.registerBlock("brokenmod:leaked", "Leaked", "minecraft:stone", 1.0f, 0, true);
        throw new IllegalStateException("live regression intentional failure");
    }
}
