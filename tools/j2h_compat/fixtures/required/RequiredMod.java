package fixture.required;

import dev.horizon.j2h.NativeBridge;

public final class RequiredMod {
    public static void onInitialize() {
        NativeBridge.registerBlock("requiredmod:must_not_load", "Required", "minecraft:stone", 1.0f, 0, true);
    }
}
