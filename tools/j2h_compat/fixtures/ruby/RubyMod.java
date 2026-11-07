package fixture.ruby;

import dev.horizon.j2h.NativeBridge;

public final class RubyMod {
    public static void onInitialize() {
        NativeBridge.registerBlock("ruby:ruby_block", "Ruby Block", "minecraft:diamond_block", 1.0f, 0, true);
        NativeBridge.registerItem("ruby:ruby_item", "Ruby", "minecraft:diamond", 64);
    }
}
