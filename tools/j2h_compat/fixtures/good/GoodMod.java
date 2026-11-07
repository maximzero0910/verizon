package fixture.good;

import dev.horizon.j2h.NativeBridge;

public final class GoodMod {
    public static void onInitialize() {
        NativeBridge.registerBlock("goodmod:survives", "Good", "minecraft:stone", 1.0f, 0, true);
    }
}
