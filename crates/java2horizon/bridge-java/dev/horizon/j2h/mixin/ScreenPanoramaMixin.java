package dev.horizon.j2h.mixin;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Screen;
import org.spongepowered.asm.mixin.Final;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

/** Horizon draws the screen background in Rust and has no vanilla GameRenderer. */
@Mixin(Screen.class)
public abstract class ScreenPanoramaMixin {
    @Shadow @Final protected Minecraft minecraft;

    @Inject(method = "extractPanorama", at = @At("HEAD"), cancellable = true)
    private void j2h$skipPanoramaWithoutGameRenderer(
            GuiGraphicsExtractor graphics, float partialTick, CallbackInfo callback) {
        if (this.minecraft != null && this.minecraft.gameRenderer == null) {
            callback.cancel();
        }
    }
}
