package com.example.libmod;

import com.example.testmod.NativeBridge;

/** Dependency mod: registers a shared block before testmod runs.
 *  Fabric entrypoints are instance methods, so this one is not static. */
public final class LibMod {
    public void onInitialize() {
        NativeBridge.log("LibMod initializing");
        NativeBridge.registerBlock(
            "libmod:shared_block", "Shared Block", "minecraft:oak_planks", 2.0f, -1, true);
        NativeBridge.entrypointComplete("libmod");
    }
}
