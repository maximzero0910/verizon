package dev.horizon.j2h;

import com.mojang.blaze3d.platform.Window;

/**
 * A text input manager that only records state in memory.
 *
 * Vanilla screen code (the search box among others) calls
 * {@code TextInputManager.setTextInputArea} while extracting, and the real
 * manager then pushes the rect through LWJGL to the raw SDL window handle,
 * which does not exist in the embedded JVM. {@code J2HMinecraft} installs
 * this stub instead so the calls are recorded no-ops.
 */
public final class StubTextInputManager extends com.mojang.blaze3d.platform.TextInputManager {
    public StubTextInputManager(Window window) {
        super(window);
    }

    @Override
    public void setTextInputArea(int x, int y, int width, int height) {
    }

    @Override
    public void startTextInput(Object owner) {
    }

    @Override
    public void stopTextInput(Object owner) {
    }

    @Override
    public void stopTextInput() {
    }
}
