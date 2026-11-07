package com.example.testmod;

/** Content mod depending on libmod: block + item with properties. */
public final class TestMod {
    public static void onInitialize() {
        NativeBridge.log("TestMod initializing");
        NativeBridge.registerBlock(
            "testmod:example_block", "Example Block", "minecraft:stone", 2.0f, 7, true);
        NativeBridge.registerItem(
            "testmod:example_item", "Example Item", "minecraft:stick", 16);
        NativeBridge.entrypointComplete("testmod");
    }
}
