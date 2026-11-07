package dev.horizon.j2h;

import net.minecraft.client.gui.screens.Screen;

/**
 * The real {@code net.minecraft.client.gui.Gui}, subclassed only so its
 * {@code setScreen} hands the opened screen to the GUI bridge (Horizon paints
 * it; the real {@code Gui} has no GL to drive). Mod handlers call
 * {@code client.getGui().setScreen(screen)} directly, so the singleton must
 * route there, not leave a null-{@code minecraft} field behind.
 */
public class J2HGui extends net.minecraft.client.gui.Gui {
    /** Not called; the instance is allocated constructor-free. */
    public J2HGui(net.minecraft.client.Minecraft minecraft,
            net.minecraft.client.gui.Hud hud,
            net.minecraft.client.renderer.state.gui.GuiRenderState renderState) {
        super(minecraft, hud, renderState);
    }

    /** Open a screen: store it for the bridge (and its own screen field). */
    @Override
    public void setScreen(Screen screen) {
        GuiBridge.acceptScreen(screen);
    }
}
