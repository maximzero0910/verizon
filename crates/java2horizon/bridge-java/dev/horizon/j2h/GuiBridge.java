package dev.horizon.j2h;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.ArrayDeque;
import java.util.HashSet;
import java.util.Locale;
import java.util.Map;
import java.nio.file.Path;
import java.util.Set;

import org.joml.Matrix3x2fStack;

import com.mojang.blaze3d.platform.InputConstants;
import com.mojang.blaze3d.platform.Window;

import net.minecraft.SharedConstants;
import net.minecraft.client.Minecraft;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Options;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.GlyphSource;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.screens.Screen;
import net.minecraft.client.renderer.state.gui.GuiRenderState;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.FontDescription;
import net.minecraft.resources.Identifier;
import net.minecraft.network.chat.FormattedText;
import net.minecraft.util.FormattedCharSequence;
import net.minecraft.world.item.ItemStack;

/**
 * Bridges Minecraft's real GUI screen system to Horizon.
 *
 * <p>Horizon has no OpenGL/RenderPearl, so it cannot run the real
 * {@code GuiGraphicsExtractor} internals. This bridge renders the real,
 * unmodified Minecraft {@code Screen} (here: Mod Menu) through a
 * {@code RecordingExtractor} that extends the real {@code GuiGraphicsExtractor}
 * and re-emits every draw call as a small platform-neutral command consumed by
 * Horizon. Widgets, layout, focus, tooltips and input all remain the real
 * Minecraft code; only the last hop from {@code Screen} to pixels is Horizon's.
 *
 * <p>The {@code Minecraft} singleton is the real object published by
 * {@link MinecraftBootstrap}. This class makes it usable for a screen by
 * installing a real {@link Font} (via Minecraft's own {@link Font.Provider}
 * extension point) and a {@link Window} sized to Horizon's scaled GUI.
 */
public final class GuiBridge {
    private GuiBridge() {
    }

    // ---- native emission (implemented in Rust) ----

    private static native void emitFill(int x0, int y0, int x1, int y1, int color);

    private static native void emitFillGradient(int x0, int y0, int x1, int y1, int top, int bottom);

    private static native void emitText(int x, int y, int color, boolean shadow, String text);

    private static native void emitBlit(
            String identifier, int x0, int y0, int w, int h, float u0, float v0, float u1, float v1);

    private static native void emitTextureImage(String identifier, int width, int height, byte[] rgba);

    private static native void emitSprite(String identifier, int x0, int y0, int w, int h);

    private static native void emitScissor(int x0, int y0, int x1, int y1);

    private static native void emitScissorEnd();

    private static native void emitTooltip(String lines, int x, int y);

    // ---- state ----

    private static boolean prepared;

    /** The screen Horizon is currently showing, or {@code null}. */
    private static Screen current;

    private static final GuiRenderState RENDER_STATE = new GuiRenderState();
    private static final int[] FRAME_TEXT_X = new int[256];
    private static final int[] FRAME_TEXT_Y = new int[256];
    private static final String[] FRAME_TEXT = new String[256];
    private static int frameTextCount;
    private static final Set<Identifier> SENT_TEXTURES = new HashSet<>();
    private static boolean loggedReiRenderStates;

    private static Object screenOf(Minecraft mc) {
        net.minecraft.client.gui.Gui g = gui();
        return g == null ? null : g.screen();
    }

    private static void checkPrepared() {
        if (!prepared) {
            prepare();
        }
    }

    /**
     * Make the {@code Minecraft} singleton usable for a screen: a real
     * {@link Font} (metrics only, glyph pixels are Horizon's) and a real
     * {@link Window} whose scaled size follows Horizon's last UI size.
     */
    public static void prepare() {
        mark("P0 prepare begin");
        Minecraft mc = Minecraft.getInstance();
        if (mc != null) {
            setWindowSize(mc, 1280, 720, 2);
            if (mc.font == null) {
                setFont(mc);
            }
            // Widgets read getLastInputType() during init; a null value causes NPE.
            mc.setLastInputType(net.minecraft.client.InputType.MOUSE);
            // Load Minecraft English translations so Component.translatable()
            // resolves keys to readable text instead of raw codes.
            loadLocale(mc);
            // Mod handlers read and set screens through Minecraft.gui
            // (getGui().screen()), so a real Gui must host the open screen and
            if (gui() == null) {
                try {
                    net.minecraft.client.gui.Gui g =
                            (net.minecraft.client.gui.Gui) J2HUnsafe.allocate(J2HGui.class);
                    field(Minecraft.class, "gui").set(mc, g);
                } catch (Exception e) {
                    log("install gui: " + e);
                }
            }
        }
        prepared = true;
        mark("P1 prepare done mc=" + (mc != null) + " font=" + (mc != null && mc.font != null)
                + " gui=" + (gui() != null) + " window="
                + (mc != null && mc.getWindow() != null));
        try {
            mark("K1a modmenu binding registered=" + KeyBindingOpener.registered("modmenu"));
        } catch (Throwable t) {
            mark("K1a exception " + t);
        }
    }

    private static net.minecraft.client.gui.Gui gui() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return null;
        }
        try {
            return (net.minecraft.client.gui.Gui) field(Minecraft.class, "gui").get(mc);
        } catch (IllegalAccessException e) {
            return null;
        }
    }

    private static void setGuiScreen(Screen screen) {
        net.minecraft.client.gui.Gui g = gui();
        if (g != null) {
            try {
                field(net.minecraft.client.gui.Gui.class, "screen").set(g, screen);
            } catch (IllegalAccessException e) {
                log("gui.screen: " + e);
            }
        }
    }

    /** Remember the size Horizon reports and size the singleton's window. */
    public static void size(int width, int height) {
        prepare();
        setWindowSize(Minecraft.getInstance(), width, height, 1);
    }

    // ---- screen lifecycle ----

    /** Whether a screen is currently open in the bridge. */
    public static boolean isOpen() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return false;
        }
        return current != null && screenOf(mc) == current;
    }

    public static String currentScreen() {
        if (!isOpen()) {
            return "";
        }
        return current.getClass().getName();
    }

    /**
     * Open a screen by pressing a Fabric keybinding and letting the real
     * {@code ClientTickEvents.END_CLIENT_TICK} handler react, exactly as a
     * player pressing the key would. Returns true if a screen opened.
     */
    public static boolean pressBinding(String categoryId) {
        checkPrepared();
        mark("K0 pressBinding \"" + categoryId + "\"");
        return KeyBindingOpener.openByCategory(categoryId);
    }

    public static boolean bindingRegistered(String categoryId) {
        checkPrepared();
        return KeyBindingOpener.registered(categoryId);
    }

    /** Store a real screen opened by a Fabric mod (via {@code J2HMinecraft}). */
    public static void acceptScreen(Screen screen) {
        checkPrepared();
        ensureOptions();
        mark("S2 acceptScreen "
                + (screen == null ? "null" : screen.getClass().getName()));
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return;
        }
        int w = mc.getWindow().getGuiScaledWidth();
        int h = mc.getWindow().getGuiScaledHeight();
        if (screen != null) {
            screen.init(w, h);
            mark("S3 init done w=" + w + " h=" + h + " screen.width=" + screen.width
                    + " screen.height=" + screen.height);
            setGuiScreen(screen);
        } else {
            setGuiScreen(null);
        }
        current = screen;
        loggedReiRenderStates = false;
        if (screen != null && screen.getClass().getName().equals(
                "me.shedaniel.rei.impl.client.gui.config.REIConfigScreen")) {
            logReiConfigurationModel(screen);
        }
        log("screen accepted: " + currentScreen());
        mark("S4 acceptScreen done current=" + currentScreen());
    }

    private static void logReiConfigurationModel(Screen screen) {
        if (!Boolean.getBoolean("j2h.debug.gui")) {
            return;
        }
        try {
            java.lang.reflect.Field categoriesField = screen.getClass().getDeclaredField("categories");
            categoriesField.setAccessible(true);
            java.util.List<?> categories = (java.util.List<?>) categoriesField.get(screen);
            StringBuilder detail = new StringBuilder("REI config model categories=").append(categories.size());
            for (Object category : categories) {
                java.util.List<?> groups = (java.util.List<?>) category.getClass()
                        .getMethod("getGroups").invoke(category);
                int options = 0;
                for (Object group : groups) {
                    options += ((java.util.List<?>) group.getClass()
                            .getMethod("getOptions").invoke(group)).size();
                }
                detail.append(" [").append(category.getClass().getMethod("getKey").invoke(category))
                        .append(" groups=").append(groups.size()).append(" options=").append(options).append(']');
            }
            java.lang.reflect.Field widgetsField = screen.getClass().getDeclaredField("widgets");
            widgetsField.setAccessible(true);
            detail.append(" widgets=").append(((java.util.List<?>) widgetsField.get(screen)).size());
            java.lang.reflect.Field activeField = screen.getClass().getDeclaredField("activeCategory");
            activeField.setAccessible(true);
            Object active = activeField.get(screen);
            detail.append(" active=").append(active == null ? "null"
                    : active.getClass().getMethod("getKey").invoke(active));
            if (active != null) {
                for (Object group : (java.util.List<?>) active.getClass()
                        .getMethod("getGroups").invoke(active)) {
                    detail.append(" group[").append(group.getClass().getMethod("getId").invoke(group))
                            .append(':');
                    for (Object option : (java.util.List<?>) group.getClass()
                            .getMethod("getOptions").invoke(group)) {
                        Object name = option.getClass().getMethod("getName").invoke(option);
                        Object entry = option.getClass().getMethod("getEntry").invoke(option);
                        detail.append(name).append("/").append(entry == null ? "null"
                                : entry.getClass().getSimpleName()).append(',');
                    }
                    detail.append(']');
                }
            }
            for (String fieldName : new String[] { "options", "defaultOptions" }) {
                java.lang.reflect.Field valueField = screen.getClass().getDeclaredField(fieldName);
                valueField.setAccessible(true);
                Object value = valueField.get(screen);
                detail.append(' ').append(fieldName).append('=')
                        .append(value instanceof java.util.Map<?, ?> map ? map.size() : value);
            }
            log(detail.toString());
        } catch (Throwable error) {
            log("REI config model inspection failed: " + error);
        }
    }

    public static boolean close() {
        mark("C1 close open=" + isOpen());
        if (current != null) {
            Screen closing = current;
            try {
                closing.onClose();
            } catch (Throwable t) {
                log("close: " + t);
            }
            // Some real screens (for example REI's config screen) install
            // their parent from onClose. Keep that replacement; only clear
            // the bridge if onClose left the same screen active.
            if (current == closing) {
                current = null;
                setGuiScreen(null);
            }
            log("screen closed; current=" + currentScreen());
            return true;
        }
        return false;
    }

    public static void tick() {
        if (isOpen()) {
            try {
                current.tick();
            } catch (Throwable t) {
                log("tick: " + t);
            }
        }
    }

    /** Render the current screen into recorded commands. */
    public static void renderFrame(int mouseX, int mouseY, float delta) {
        Minecraft r1mc = Minecraft.getInstance();
        Object sof = null;
        try { sof = screenOf(r1mc); } catch (Throwable t) {}
        mark("R1 renderFrame open=" + isOpen() + " mouse=" + mouseX + "," + mouseY
                + " current=" + (current == null ? "null" : current.getClass().getSimpleName())
                + " screenOf=" + (sof == null ? "null" : sof.getClass().getSimpleName()));
        if (!isOpen()) {
            return;
        }
        Screen screen = current;
        if (screen.width == 0) {
            // init() has not run at this width yet.
            Minecraft mc = Minecraft.getInstance();
            int w = mc.getWindow().getGuiScaledWidth();
            int h = mc.getWindow().getGuiScaledHeight();
            screen.init(w, h);
        }
        ensureOptions();
        updateMouse(mouseX, mouseY);
        cFill = cGrad = cText = cSprite = cBlit = cRStateText = 0;
        for (int i = 0; i < frameTextCount; i++) {
            FRAME_TEXT[i] = null;
        }
        frameTextCount = 0;
        try {
            RecordingExtractor ex = RecordingExtractor.create(
                    Minecraft.getInstance(), mouseX, mouseY);
            RENDER_STATE.reset();
            // The real orchestrator (Screen.extractRenderStateWithTooltipAndSubtitles)
            // is: nextStratum, extractBackground, nextStratum, extractRenderState,
            // extractDeferredElements. Its menu branch needs Minecraft internals the
            // embedded JVM does not have (gameRenderer, level, options), so use the
            // safe in-game pieces directly: the transparent dim gradient.
            ex.nextStratum();
            screen.extractTransparentBackground(ex);
            ex.nextStratum();
            screen.extractRenderState(ex, mouseX, mouseY, delta);
            ex.extractDeferredElements(mouseX, mouseY, delta);
            emitCollectedElements();
            emitCollectedText();
            emitModMenuActionLabels(screen);
            mark("R2 renderFrame done screen=" + screen.getClass().getName()
                    + " fill=" + cFill + " grad=" + cGrad + " text=" + cText
                    + " sprite=" + cSprite + " blit=" + cBlit + " rstate=" + cRStateText);
        } catch (Throwable t) {
            java.io.StringWriter sw = new java.io.StringWriter();
            t.printStackTrace(new java.io.PrintWriter(sw));
            java.lang.String trace = sw.toString();
            log("extractRenderState: " + t + "\n"
                    + trace.substring(0, Math.min(trace.length(), 2500)));
            mark("R2 renderFrame exception screen=" + screen.getClass().getName() + " " + t);
        }
    }

    /**
     * Some Fabric screens draw through adapters which wrap this extractor
     * (REI's GuiGraphics is one example). Those calls submit immutable GUI
     * render states directly instead of dispatching to RecordingExtractor's
     * overrides. Forward the vanilla colored geometry that was collected so
     * adapted screens retain their panels and controls in Horizon.
     */
    private static void emitCollectedElements() {
        RENDER_STATE.forEachElement(element -> {
            if (!(element instanceof net.minecraft.client.renderer.state.gui.ColoredRectangleRenderState rect)) {
                return;
            }
            net.minecraft.client.gui.render.TextureSetup setup = rect.textureSetup();
            if (setup.texure0() != null || setup.texure1() != null || setup.texure2() != null) {
                return;
            }
            net.minecraft.client.gui.navigation.ScreenRectangle bounds = rect.bounds();
            int x0 = bounds.left();
            int y0 = bounds.top();
            int x1 = bounds.right();
            int y1 = bounds.bottom();
            if (x1 <= x0 || y1 <= y0) {
                return;
            }
            int top = rect.col1();
            int bottom = rect.col2();
            if (top == bottom) {
                emitFill(x0, y0, x1, y1, top);
            } else {
                emitFillGradient(x0, y0, x1, y1, top, bottom);
            }
            cFill++;
        }, net.minecraft.client.renderer.state.gui.GuiRenderState.TraverseRange.ALL);
    }

    /** Re-emit the GuiTextRenderState entries that vanilla's text pipeline
     * collected in GuiRenderState instead of calling our text() overrides. */
    private static void emitCollectedText() {
        RENDER_STATE.forEachText(state -> {
            try {
                int x = TEXT_X.getInt(state);
                int y = TEXT_Y.getInt(state);
                org.joml.Matrix3x2fc pose = state.pose;
                int transformedX = Math.round(pose.m00() * x + pose.m10() * y + pose.m20());
                int transformedY = Math.round(pose.m01() * x + pose.m11() * y + pose.m21());
                int color = TEXT_COLOR.getInt(state);
                boolean shadow = TEXT_SHADOW.getBoolean(state);
                FormattedCharSequence seq = (FormattedCharSequence) TEXT_SEQ.get(state);
                if (seq == null) {
                    return;
                }
                String str = fcs(seq);
                if (!str.isEmpty()) {
                    if (!loggedReiRenderStates && current != null
                            && current.getClass().getName().equals(
                                    "me.shedaniel.rei.impl.client.gui.config.REIConfigScreen")) {
                        mark("REI collected text x=" + transformedX + " y=" + transformedY + " color="
                                + Integer.toHexString(color) + " value=" + str);
                    }
                    emitText(transformedX, transformedY, color, shadow, str);
                    recordFrameText(transformedX, transformedY, str);
                    cRStateText++;
                }
            } catch (Throwable ignored) {
                log("deferred text extraction: " + ignored);
            }
        });
        if (current != null && current.getClass().getName().equals(
                "me.shedaniel.rei.impl.client.gui.config.REIConfigScreen")) {
            loggedReiRenderStates = true;
        }
    }

    /** Keep Mod Menu's action labels visible when its widget sprites are unavailable. */
    private static void emitModMenuActionLabels(Screen screen) {
        if (!screen.getClass().getName().equals("com.terraformersmc.modmenu.gui.ModsScreen")) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (mc == null || mc.font == null) {
            return;
        }
        for (String fieldName : new String[] { "configureButton", "websiteButton", "issuesButton" }) {
            try {
                java.lang.reflect.Field field = screen.getClass().getDeclaredField(fieldName);
                field.setAccessible(true);
                Object widget = field.get(screen);
                if (!(widget instanceof net.minecraft.client.gui.components.Button button)) {
                    continue;
                }
                String label = button.getMessage().getString();
                if (label.isBlank() && fieldName.equals("websiteButton")) {
                    label = modMenuLabel("WEBSITE");
                } else if (label.isBlank() && fieldName.equals("issuesButton")) {
                    label = modMenuLabel("ISSUES");
                }
                if (label.isBlank()) {
                    continue;
                }
                // Vanilla extraction and many screen implementations already
                // emitted this label. Compare the widget's rendered area rather
                // than its text: formatting and localization can change the
                // extracted plain string while leaving the visible label there.
                if (hasRenderedText(button, mc.font)) {
                    continue;
                }
                int x = button.getX() + (button.getWidth() - mc.font.width(label)) / 2;
                int y = button.getY() + (button.getHeight() - mc.font.lineHeight) / 2;
                emitText(x, y, 0xFFC8C8C8, false, label);
                recordFrameText(x, y, label);
            } catch (ReflectiveOperationException error) {
                log("Mod Menu action label " + fieldName + ": " + error);
            }
        }
    }

    private static boolean hasRenderedText(
            net.minecraft.client.gui.components.Button button, Font font) {
        int x0 = button.getX();
        int y0 = button.getY();
        int x1 = x0 + button.getWidth();
        int y1 = y0 + button.getHeight();
        for (int i = 0; i < frameTextCount; i++) {
            String text = FRAME_TEXT[i];
            if (text != null
                    && !text.isEmpty()
                    && FRAME_TEXT_X[i] < x1
                    && FRAME_TEXT_X[i] + font.width(text) > x0
                    && FRAME_TEXT_Y[i] < y1
                    && FRAME_TEXT_Y[i] + font.lineHeight > y0) {
                return true;
            }
        }
        return false;
    }

    private static void recordFrameText(int x, int y, String text) {
        if (text.isEmpty() || frameTextCount == FRAME_TEXT.length) {
            return;
        }
        FRAME_TEXT_X[frameTextCount] = x;
        FRAME_TEXT_Y[frameTextCount] = y;
        FRAME_TEXT[frameTextCount] = text;
        frameTextCount++;
    }

    private static String modMenuLabel(String fieldName) {
        try {
            Class<?> texts = Class.forName("com.terraformersmc.modmenu.util.ModMenuScreenTexts");
            java.lang.reflect.Field field = texts.getDeclaredField(fieldName);
            field.setAccessible(true);
            Object label = field.get(null);
            return label instanceof Component component ? component.getString() : "";
        } catch (ReflectiveOperationException error) {
            log("Mod Menu label " + fieldName + ": " + error);
            return "";
        }
    }

    private static final java.lang.reflect.Field TEXT_SEQ, TEXT_X, TEXT_Y, TEXT_COLOR, TEXT_SHADOW;
    static {
        try {
            Class<?> cls = Class.forName(
                    "net.minecraft.client.renderer.state.gui.GuiTextRenderState");
            TEXT_SEQ = cls.getDeclaredField("text");
            TEXT_SEQ.setAccessible(true);
            TEXT_X = cls.getDeclaredField("x");
            TEXT_X.setAccessible(true);
            TEXT_Y = cls.getDeclaredField("y");
            TEXT_Y.setAccessible(true);
            TEXT_COLOR = cls.getDeclaredField("color");
            TEXT_COLOR.setAccessible(true);
            TEXT_SHADOW = cls.getDeclaredField("dropShadow");
            TEXT_SHADOW.setAccessible(true);
        } catch (Throwable t) {
            throw new IllegalStateException("text state fields: " + t, t);
        }
    }

    private static int cFill, cGrad, cText, cSprite, cBlit, cRStateText;

    /** The bridge's own resize entrypoint. */
    public static void resize(int width, int height) {
        size(width, height);
        if (current != null) {
            Minecraft mc = Minecraft.getInstance();
            current.init(width, height);
            current.resize(width, height);
        }
    }

    // ---- input (forwarded to the real screen) ----

    public static void mouseMoved(double x, double y) {
        if (isOpen()) {
            current.mouseMoved(x, y);
            current.afterMouseMove();
        }
    }

    public static void mouseClicked(double x, double y, int button, int mods) {
        if (isOpen()) {
            net.minecraft.client.input.MouseButtonEvent e =
                    new net.minecraft.client.input.MouseButtonEvent(x, y,
                            new net.minecraft.client.input.MouseButtonInfo(button + 1, mods));
            if (!current.mouseClicked(e, false)) {
                current.afterMouseAction();
            }
        }
    }

    public static void mouseReleased(double x, double y, int button, int mods) {
        if (isOpen()) {
            net.minecraft.client.input.MouseButtonEvent e =
                    new net.minecraft.client.input.MouseButtonEvent(x, y,
                            new net.minecraft.client.input.MouseButtonInfo(button + 1, mods));
            current.mouseReleased(e);
            current.afterMouseAction();
        }
    }

    public static void mouseDragged(double x, double y, double dx, double dy, int button, int mods) {
        if (isOpen()) {
            net.minecraft.client.input.MouseButtonEvent e =
                    new net.minecraft.client.input.MouseButtonEvent(x, y,
                            new net.minecraft.client.input.MouseButtonInfo(button + 1, mods));
            current.mouseDragged(e, dx, dy);
        }
    }

    public static void mouseScrolled(double x, double y, double dx, double dy) {
        if (isOpen()) {
            current.mouseScrolled(x, y, dx, dy);
            current.afterMouseAction();
        }
    }

    public static void keyPressed(int keyCode, int scanCode, int mods) {
        if (isOpen()) {
            Screen screen = current;
            boolean handled = screen.keyPressed(new net.minecraft.client.input.KeyEvent(
                    keyCode, shortcutKeyCode(keyCode), mods));
            if (!handled && keyCode == 256 && screen.shouldCloseOnEsc()) {
                close();
            }
            if (isOpen() && current == screen) {
                screen.afterKeyboardAction();
            }
        }
    }

    private static int shortcutKeyCode(int keyCode) {
        return switch (keyCode) {
            case 259 -> 8;
            case 261 -> 127;
            case 256 -> 27;
            case 257 -> 13;
            case 258 -> 9;
            case 262 -> 1073741903;
            case 263 -> 1073741904;
            case 264 -> 1073741905;
            case 265 -> 1073741906;
            case 268 -> 1073741898;
            case 269 -> 1073741901;
            default -> keyCode;
        };
    }

    public static void keyReleased(int keyCode, int scanCode, int mods) {
        if (isOpen()) {
            current.keyReleased(new net.minecraft.client.input.KeyEvent(
                    keyCode, shortcutKeyCode(keyCode), mods));
            current.afterKeyboardAction();
        }
    }

    /** Convert a FormattedCharSequence to plain text for Horizon. */
    private static String fcs(FormattedCharSequence seq) {
        StringBuilder sb = new StringBuilder();
        seq.accept((index, style, codepoint) -> {
            sb.appendCodePoint(codepoint);
            return true;
        });
        return sb.toString();
    }

    public static void charTyped(int codePoint) {
        if (isOpen()) {
            try {
                current.charTyped(new net.minecraft.client.input.CharacterEvent(codePoint));
            } catch (Throwable t) {
                log("charTyped: " + t);
            }
            current.afterKeyboardAction();
        }
    }

    // ---- internals ----

    static void log(String message) {
        System.err.println("[j2h-gui] " + message);
    }

    /** Distinct marker for tracing the M-key screen path in the game log. */
    static void mark(String marker) {
        if (Boolean.getBoolean("j2h.debug.gui")) {
            System.err.println("[j2h-marker] " + marker);
        }
    }

    private static void setWindowSize(Minecraft mc, int width, int height, int scale) {
        try {
            Window win = mc.getWindow();
            if (win == null) {
                win = (Window) J2HUnsafe.allocate(Window.class);
                field(Minecraft.class, "window").set(mc, win);
            }
            // Keep a fixed scale of 1 so bridge coordinates == pixel coordinates;
            // Horizon scales the final GUI itself.
            reflect(win, "width", int.class, width);
            reflect(win, "height", int.class, height);
            reflect(win, "guiScale", int.class, 1);
            reflect(win, "guiScaledWidth", int.class, width);
            reflect(win, "guiScaledHeight", int.class, height);
            reflect(win, "windowedWidth", int.class, width);
            reflect(win, "windowedHeight", int.class, height);
        } catch (Throwable t) {
            log("setWindowSize: " + t);
        }
    }

    /** Load Minecraft en_us.json into the Language singleton so translated
     * Components show real text instead of raw keys. */
    private static void loadLocale(Minecraft mc) {
        java.util.Map<String, String> tr = new java.util.HashMap<>();
        // Read from the known cache paths since the embedded Minecraft has no
        // gameDirectory.
        String libDir = System.getProperty("j2h.lib_dir",
                ".horizon-cache/java2horizon-libraries");
        java.util.List<java.nio.file.Path> clientJars = java.util.List.of(
                java.nio.file.Path.of(libDir, "minecraft/26.3/client.jar"),
                java.nio.file.Path.of(libDir, "minecraft/26.3/client-26.3.jar"),
                java.nio.file.Path.of(libDir, "minecraft/26.3/jars/client.jar"),
                java.nio.file.Path.of(".local/minecraft/26.3/jars/client.jar"));
        for (java.nio.file.Path clientJar : clientJars) {
            if (!java.nio.file.Files.isRegularFile(clientJar)) {
                continue;
            }
            try (java.util.zip.ZipFile zf = new java.util.zip.ZipFile(clientJar.toFile())) {
                java.util.zip.ZipEntry ze = zf.getEntry("assets/minecraft/lang/en_us.json");
                if (ze != null) {
                    try (java.io.InputStream in = zf.getInputStream(ze)) {
                        net.minecraft.locale.Language.loadFromJson(in, tr::put);
                    }
                }
                break;
            } catch (Throwable t) {
                log("mc lang " + clientJar + ": " + t);
            }
        }
        String modsDir = System.getProperty("j2h.mods_dir", "mods");
        java.nio.file.Path modsPath = java.nio.file.Path.of(modsDir);
        if (java.nio.file.Files.isDirectory(modsPath)) {
            try (java.nio.file.DirectoryStream<java.nio.file.Path> ds =
                    java.nio.file.Files.newDirectoryStream(modsPath, "*.jar")) {
                for (java.nio.file.Path jar : ds) {
                    try (java.util.zip.ZipFile mzf =
                            new java.util.zip.ZipFile(jar.toFile())) {
                        String namespace = modNamespaceFromJar(mzf, jar);
                        java.util.zip.ZipEntry ze =
                                mzf.getEntry("assets/" + namespace + "/lang/en_us.json");
                        if (ze != null) {
                            java.io.InputStream in = mzf.getInputStream(ze);
                            net.minecraft.locale.Language.loadFromJson(in, tr::put);
                            in.close();
                            mark("S9a mod lang " + namespace + ": " + ze.getSize());
                        }
                    } catch (Throwable t) {
                    }
                }
            } catch (Throwable t) {
                log("mods dir: " + t);
            }
        }
        if (!tr.isEmpty()) {
            injectLocale(tr);
        }
    }

    private static String modNamespaceFromJar(java.util.zip.ZipFile jar, java.nio.file.Path fallback) {
        String fallbackId = fallback.getFileName().toString();
        fallbackId = fallbackId.replaceFirst("-\\d.*$", "");
        try {
            java.util.zip.ZipEntry fabricEntry = jar.getEntry("fabric.mod.json");
            if (fabricEntry == null) {
                return fallbackId;
            }
            String json;
            try (java.io.InputStream in = jar.getInputStream(fabricEntry)) {
                json = new String(in.readAllBytes(), java.nio.charset.StandardCharsets.UTF_8);
            }
            java.util.regex.Matcher matcher =
                    java.util.regex.Pattern.compile("\\\"id\\\"\\s*:\\s*\\\"([^\\\"]+)\\\"")
                            .matcher(json);
            if (matcher.find()) {
                return matcher.group(1);
            }
        } catch (Throwable ignored) {
        }
        return fallbackId;
    }

    private static void injectLocale(java.util.Map<String, String> tr) {
        net.minecraft.locale.Language.inject(new net.minecraft.locale.Language() {
            private final java.util.Map<String, String> map = tr;
            @Override
            public String getOrDefault(String key, String fallback) {
                String value = map.get(key);
                if (value != null) return value;
                return net.minecraft.locale.Language.DEFAULT_INSTANCE
                        .getOrDefault(key, fallback);
            }
            @Override
            public boolean has(String key) {
                return map.containsKey(key) || net.minecraft.locale.Language.DEFAULT_INSTANCE.has(key);
            }
            @Override
            public boolean isDefaultRightToLeft() { return false; }
            @Override
            public net.minecraft.util.FormattedCharSequence getVisualOrder(
                    net.minecraft.network.chat.FormattedText text) {
                return net.minecraft.locale.Language.DEFAULT_INSTANCE
                        .getVisualOrder(text);
            }
        });
        mark("S9 lang loaded keys=" + tr.size());
    }


    private static void setFont(Minecraft mc) {
        Font font = new Font(new MetricsProvider());
        try {
            field(Minecraft.class, "font").set(mc, font);
        } catch (IllegalAccessException e) {
            log("set font: " + e);
        }
    }

    /** Screen extraction reads client.options (guiScale for icon sizes at least).
     * The embedded JVM's fake Minecraft has none: install a stub with the
     * option instances screens read, extended lazily as the need appears. */
    private static Options clientOptions;

    private static void ensureOptions() {
        Minecraft mc = Minecraft.getInstance();
        if (mc == null) {
            return;
        }
        try {
            Options options = (Options) field(Minecraft.class, "options").get(mc);
            if (options == null) {
                options = (Options) J2HUnsafe.allocate(Options.class);
                field(Options.class, "guiScale").set(options, intOption(1));
                field(Options.class, "narrator").set(options, narratorOption());
                field(Options.class, "textureFiltering").set(options, textureFilteringOption());
                field(Options.class, "mipmapLevels").set(options, intOption(0));
                field(Minecraft.class, "options").set(mc, options);
                mark("S6 options installed");
            }
            if (field(Options.class, "textureFiltering").get(options) == null) {
                field(Options.class, "textureFiltering").set(options, textureFilteringOption());
            }
            if (field(Options.class, "mipmapLevels").get(options) == null) {
                field(Options.class, "mipmapLevels").set(options, intOption(0));
            }
            if (field(Minecraft.class, "mouseHandler").get(mc) == null) {
                field(Minecraft.class, "mouseHandler").set(
                        mc, new net.minecraft.client.MouseHandler(mc));
                mark("S6 mouse handler installed");
            }
            clientOptions = options;
        } catch (Throwable t) {
            log("ensureOptions: " + t);
        }
        GpuStub.install();
        ensureTextureManager();
        ensureAtlasManager();
    }

    /** Minecraft's menu widgets query this option when selecting a mod row.
     * Horizon has no speech backend, so expose vanilla's disabled state. */
    private static OptionInstance<net.minecraft.client.NarratorStatus> narratorOption() {
        return new OptionInstance<>(
                "options.narrator",
                OptionInstance.noTooltip(),
                (caption, status) -> net.minecraft.network.chat.Component.translatable(
                        "options.narrator.notavailable"),
                new OptionInstance.Enum<>(
                        java.util.Arrays.asList(net.minecraft.client.NarratorStatus.values()),
                        net.minecraft.client.NarratorStatus.LEGACY_CODEC),
                net.minecraft.client.NarratorStatus.OFF,
                status -> {});
    }

    private static OptionInstance<net.minecraft.client.TextureFilteringMethod> textureFilteringOption() {
        return new OptionInstance<>(
                "options.textureFiltering",
                OptionInstance.noTooltip(),
                (caption, method) -> method.caption(),
                new OptionInstance.Enum<>(
                        java.util.Arrays.asList(net.minecraft.client.TextureFilteringMethod.values()),
                        net.minecraft.client.TextureFilteringMethod.LEGACY_CODEC),
                net.minecraft.client.TextureFilteringMethod.NONE,
                method -> {});
    }

    /** Texture-backed paths (fabric mod icons) register DynamicTextures on the
     * client's texture manager. The embedded Minecraft has none: install one
     * with only the maps {@code register} uses, so registration is inert. */
    /** Keep REI's PointHelper.ofMouse() in sync with the scaled GUI cursor. */
    private static void updateMouse(int mouseX, int mouseY) {
        try {
            Minecraft mc = Minecraft.getInstance();
            net.minecraft.client.MouseHandler mouse = mc.mouseHandler;
            if (mouse == null) {
                return;
            }
            com.mojang.blaze3d.platform.Window window = mc.getWindow();
            int guiWidth = window.getGuiScaledWidth();
            int guiHeight = window.getGuiScaledHeight();
            double scaleX = guiWidth == 0 ? 1.0 : (double) window.getScreenWidth() / guiWidth;
            double scaleY = guiHeight == 0 ? 1.0 : (double) window.getScreenHeight() / guiHeight;
            field(net.minecraft.client.MouseHandler.class, "xpos").set(mouse, mouseX * scaleX);
            field(net.minecraft.client.MouseHandler.class, "ypos").set(mouse, mouseY * scaleY);
        } catch (Throwable t) {
            log("updateMouse: " + t);
        }
    }

    private static void ensureTextureManager() {
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc.getTextureManager() != null) {
                return;
            }
            net.minecraft.client.renderer.texture.TextureManager stub =
                    (net.minecraft.client.renderer.texture.TextureManager)
                            J2HUnsafe.allocate(
                                    net.minecraft.client.renderer.texture.TextureManager.class);
            java.lang.reflect.Field byPath =
                    net.minecraft.client.renderer.texture.TextureManager.class
                            .getDeclaredField("byPath");
            byPath.setAccessible(true);
            byPath.set(stub, new java.util.HashMap<>());
            java.lang.reflect.Field tickable =
                    net.minecraft.client.renderer.texture.TextureManager.class
                            .getDeclaredField("tickableTextures");
            tickable.setAccessible(true);
            tickable.set(stub, new java.util.HashSet<>());
            field(Minecraft.class, "textureManager").set(mc, stub);
            mark("S8 texture manager installed");
        } catch (Throwable t) {
            log("ensureTextureManager: " + t);
        }
    }

    private static boolean atlasLoaded;
    private static boolean atlasLoadAttempted;

    /** Install vanilla's atlas registry against Horizon's inert GPU and
     * texture-manager adapters so GuiGraphicsExtractor can resolve atlas IDs. */
    private static void ensureAtlasManager() {
        try {
            Minecraft mc = Minecraft.getInstance();
            if (mc.getAtlasManager() == null) {
                net.minecraft.client.resources.model.sprite.AtlasManager atlasManager =
                        new net.minecraft.client.resources.model.sprite.AtlasManager(
                                mc.getTextureManager(), 0);
                field(Minecraft.class, "atlasManager").set(mc, atlasManager);
                mark("S10 atlas manager installed");
            }
            if (!atlasLoadAttempted && mc.getAtlasManager() != null) {
                atlasLoadAttempted = true;
                loadAtlas(mc);
            }
        } catch (Throwable t) {
            log("ensureAtlasManager: " + t);
        }
    }

    /**
     * Feed the prepared resource root and the Java mod jars through a real
     * reload. Constructing AtlasManager alone registers texture ids; it does
     * not stitch the sprites, so GuiGraphicsExtractor.getSprite() throws
     * "atlas is not initialized" until a reload has filled TextureAtlas.
     */
    private static void loadAtlas(Minecraft mc) {
        java.nio.file.Path root = resourceRoot();
        if (root == null) {
            log("atlas reload: no j2h.resources_dir");
            return;
        }
        java.util.List<net.minecraft.server.packs.PackResources> packs =
                new java.util.ArrayList<>();
        addModPacks(packs, System.getProperty("j2h.mods_dir", "mods"));
        packs.add(new net.minecraft.server.packs.PathPackResources(
                packLocation("j2h_resources", root.getFileName() == null
                        ? "J2H resources" : root.getFileName().toString()), root));
        net.minecraft.server.packs.resources.ReloadableResourceManager manager =
                new net.minecraft.server.packs.resources.ReloadableResourceManager(
                        net.minecraft.server.packs.PackType.CLIENT_RESOURCES);
        manager.registerReloadListener(mc.getAtlasManager());
        try {
            field(net.minecraft.client.renderer.texture.TextureManager.class,
                    "resourceManager").set(mc.getTextureManager(), manager);
        } catch (Throwable t) {
            log("texture manager resourceManager: " + t);
        }
        Field renderThreadField = field(
                com.mojang.blaze3d.systems.RenderSystem.class, "renderThread");
        Thread previousRenderThread = null;
        try {
            previousRenderThread = (Thread) renderThreadField.get(null);
            renderThreadField.set(null, Thread.currentThread());
            net.minecraft.server.packs.resources.ReloadInstance reload =
                    manager.createReload(
                            Runnable::run,
                            Runnable::run,
                            java.util.concurrent.CompletableFuture.completedFuture(
                                    net.minecraft.util.Unit.INSTANCE),
                            packs);
            reload.done().join();
            reload.checkExceptions();
            atlasLoaded = true;
            mark("S11 atlas loaded root=" + root + " packs=" + packs.size());
        } catch (Throwable t) {
            java.io.StringWriter sw = new java.io.StringWriter();
            t.printStackTrace(new java.io.PrintWriter(sw));
            java.lang.String trace = sw.toString();
            log("atlas reload: " + t + "\n"
                    + trace.substring(0, Math.min(trace.length(), 2500)));
        } finally {
            try {
                renderThreadField.set(null, previousRenderThread);
            } catch (Throwable t) {
                log("atlas renderThread restore: " + t);
            }
        }
    }

    private static java.nio.file.Path resourceRoot() {
        String configured = System.getProperty("j2h.resources_dir");
        if (configured == null || configured.isBlank()) {
            return null;
        }
        java.nio.file.Path root = java.nio.file.Path.of(configured);
        if (!java.nio.file.Files.isDirectory(root.resolve("assets"))) {
            log("atlas reload: no assets in " + root);
            return null;
        }
        return root;
    }

    /** Mod jars carry their own GUI sprites (REI has several), so they are
     * packs in the same reload as the prepared vanilla root. */
    private static void addModPacks(
            java.util.List<net.minecraft.server.packs.PackResources> packs, String modsDir) {
        java.nio.file.Path dir = java.nio.file.Path.of(modsDir);
        if (!java.nio.file.Files.isDirectory(dir)) {
            return;
        }
        try (java.util.stream.Stream<java.nio.file.Path> stream =
                java.nio.file.Files.list(dir)) {
            int index = 0;
            for (java.nio.file.Path jar : stream
                    .filter(path -> path.toString().endsWith(".jar"))
                    .sorted()
                    .toList()) {
                try {
                    String name = jar.getFileName().toString();
                    net.minecraft.server.packs.PackLocationInfo location =
                            packLocation("j2h_mod_" + index++, name);
                    net.minecraft.server.packs.repository.Pack.Metadata metadata =
                            new net.minecraft.server.packs.repository.Pack.Metadata(
                                    Component.literal(name),
                                    net.minecraft.server.packs.repository.PackCompatibility.COMPATIBLE,
                                    net.minecraft.world.flag.FeatureFlags.DEFAULT_FLAGS,
                                    java.util.List.of());
                    try (java.util.stream.Stream<net.minecraft.server.packs.PackResources> modPacks =
                            new net.minecraft.server.packs.FilePackResources.FileResourcesSupplier(jar)
                                    .openResources(location, metadata)) {
                        modPacks.forEach(packs::add);
                    }
                } catch (Throwable t) {
                    log("atlas mod pack " + jar + ": " + t);
                }
            }
        } catch (Throwable t) {
            log("atlas mods dir " + dir + ": " + t);
        }
    }

    private static net.minecraft.server.packs.PackLocationInfo packLocation(
            String id, String title) {
        return new net.minecraft.server.packs.PackLocationInfo(
                id,
                Component.literal(title),
                net.minecraft.server.packs.repository.PackSource.DEFAULT,
                java.util.Optional.empty());
    }

    /** Transfer a DynamicTexture once so Horizon can patch it into a GUI atlas slot. */
    private static void captureDynamicTexture(Identifier id) {
        if (SENT_TEXTURES.contains(id)) {
            return;
        }
        try {
            Minecraft mc = Minecraft.getInstance();
            @SuppressWarnings("unchecked")
            Map<Identifier, net.minecraft.client.renderer.texture.AbstractTexture> registered =
                    (Map<Identifier, net.minecraft.client.renderer.texture.AbstractTexture>)
                            field(net.minecraft.client.renderer.texture.TextureManager.class, "byPath")
                                    .get(mc.getTextureManager());
            net.minecraft.client.renderer.texture.AbstractTexture texture = registered.get(id);
            if (!(texture instanceof net.minecraft.client.renderer.texture.DynamicTexture dynamic)) {
                return;
            }
            com.mojang.blaze3d.platform.NativeImage image = dynamic.getPixels();
            if (image == null) {
                return;
            }
            int width = image.getWidth();
            int height = image.getHeight();
            if (width <= 0 || height <= 0 || width > 256 || height > 256) {
                log("dynamic GUI texture has unsupported dimensions " + id + " " + width + "x" + height);
                SENT_TEXTURES.add(id);
                return;
            }
            int[] argb = image.getPixels();
            byte[] rgba = new byte[argb.length * 4];
            for (int i = 0; i < argb.length; i++) {
                int pixel = argb[i];
                int at = i * 4;
                rgba[at] = (byte) (pixel >>> 16);
                rgba[at + 1] = (byte) (pixel >>> 8);
                rgba[at + 2] = (byte) pixel;
                rgba[at + 3] = (byte) (pixel >>> 24);
            }
            emitTextureImage(id.toString(), width, height, rgba);
            SENT_TEXTURES.add(id);
            mark("dynamic GUI texture captured " + id + " " + width + "x" + height);
        } catch (Throwable t) {
            log("capture dynamic GUI texture " + id + ": " + t);
        }
    }

    private static Object intOption(int value) throws Exception {
        Object instance = J2HUnsafe.allocate(OptionInstance.class);
        java.lang.reflect.Field f =
                OptionInstance.class.getDeclaredField("value");
        f.setAccessible(true);
        f.set(instance, Integer.valueOf(value));
        return instance;
    }

    private static Field field(Class<?> owner, String name) {
        try {
            Field f = owner.getDeclaredField(name);
            f.setAccessible(true);
            return f;
        } catch (NoSuchFieldException e) {
            throw new IllegalStateException("client layout changed: " + owner.getName() + "." + name, e);
        }
    }

    private static void reflect(Object target, String name, Class<?> type, Object value) {
        try {
            Field f = field(target.getClass(), name);
            if (f.getType().isAssignableFrom(type) || f.getType() == type) {
                f.set(target, value);
            }
        } catch (IllegalAccessException e) {
            log("reflect " + name + ": " + e);
        }
    }

    /**
     * A {@link Font.Provider} that supplies glyph metrics from vanilla's
     * default 8px bitmap advance. Glyph pixels are never produced here: Horizon
     * owns text rendering, so it only needs credible advances for layout.
     */
    static final class MetricsProvider implements Font.Provider {
        @Override
        public GlyphSource glyphs(FontDescription fontDescription) {
            return new GlyphSource() {
                @Override
                public net.minecraft.client.gui.font.glyphs.BakedGlyph getGlyph(int codePoint) {
                    return metricGlyph();
                }

                @Override
                public net.minecraft.client.gui.font.glyphs.BakedGlyph getRandomGlyph(
                        net.minecraft.util.RandomSource random, int codePoint) {
                    return metricGlyph();
                }
            };
        }

        @Override
        public net.minecraft.client.gui.font.glyphs.EffectGlyph effect() {
            return new net.minecraft.client.gui.font.glyphs.EffectGlyph() {
                @Override
                public net.minecraft.client.gui.font.TextRenderable createEffect(
                        float x, float y, float width, float height, float shift,
                        int color, int z, float scale) {
                    return new net.minecraft.client.gui.font.TextRenderable() {
                        @Override
                        public void render(org.joml.Matrix4fc pose,
                                com.mojang.blaze3d.vertex.VertexConsumer vertices,
                                int packedLight, boolean seeThrough) {
                        }

                        @Override
                        public net.minecraft.client.renderer.rendertype.RenderType renderType(
                                Font.DisplayMode displayMode) {
                            return null;
                        }

                        @Override
                        public com.mojang.renderpearl.api.textures.GpuTextureView textureView() {
                            return null;
                        }

                        @Override
                        public com.mojang.renderpearl.api.pipeline.RenderPipeline guiPipeline() {
                            return null;
                        }

                        @Override public float left() { return x; }
                        @Override public float top() { return y; }
                        @Override public float right() { return x + width; }
                        @Override public float bottom() { return y + height; }
                    };
                }
            };
        }
    }

    private static net.minecraft.client.gui.font.glyphs.BakedGlyph metricGlyph() {
        return new net.minecraft.client.gui.font.glyphs.BakedGlyph() {
            @Override
            public com.mojang.blaze3d.font.GlyphInfo info() {
                return com.mojang.blaze3d.font.GlyphInfo.simple(7.0f);
            }
            @Override
            public net.minecraft.client.gui.font.TextRenderable.Styled createGlyph(
                    float advance, float x, int y, int color,
                    net.minecraft.network.chat.Style style, float z, float scale) {
                return new net.minecraft.client.gui.font.TextRenderable.Styled() {
                    private final float right = x + advance;
                    private final float bottom = y + 9.0f;

                    @Override
                    public void render(org.joml.Matrix4fc pose,
                            com.mojang.blaze3d.vertex.VertexConsumer vertices,
                            int packedLight, boolean seeThrough) {
                    }

                    @Override
                    public net.minecraft.client.renderer.rendertype.RenderType renderType(
                            Font.DisplayMode displayMode) {
                        return null;
                    }

                    @Override
                    public com.mojang.renderpearl.api.textures.GpuTextureView textureView() {
                        return null;
                    }

                    @Override
                    public com.mojang.renderpearl.api.pipeline.RenderPipeline guiPipeline() {
                        return null;
                    }

                    @Override public float left() { return x; }
                    @Override public float top() { return y; }
                    @Override public float right() { return right; }
                    @Override public float bottom() { return bottom; }
                    @Override public net.minecraft.network.chat.Style style() { return style; }
                    @Override public float activeLeft() { return left(); }
                    @Override public float activeTop() { return top(); }
                    @Override public float activeRight() { return right(); }
                    @Override public float activeBottom() { return bottom(); }
                };
            }
        };
    }

    /**
     * The recording extractor: a real {@code GuiGraphicsExtractor} instance
     * (allocated without running the RenderPearl constructor, which Horizon
     * cannot drive) whose draw methods re-emit every operation to the native
     * commands Horizon renders. Every public draw method is overridden so the
     * un-used real internals are never touched.
     */
    static final class RecordingExtractor extends GuiGraphicsExtractor {
        private int width;
        private int height;
        private final Matrix3x2fStack poseStack = new Matrix3x2fStack(4);
        private int scissorDepth;
        private final StringBuilder tooltip = new StringBuilder();
        private int tooltipX = Integer.MIN_VALUE;
        private int tooltipY = Integer.MIN_VALUE;

        private RecordingExtractor(int gw, int gh) {
            super(Minecraft.getInstance(), RENDER_STATE, gw, gh);
            this.width = gw;
            this.height = gh;
        }

        static RecordingExtractor create(Minecraft mc, int mouseX, int mouseY) {
            int w = mc.getWindow().getGuiScaledWidth();
            int h = mc.getWindow().getGuiScaledHeight();
            try {
                RecordingExtractor rec =
                        (RecordingExtractor) J2HUnsafe.allocate(RecordingExtractor.class);
                rec.width = w;
                rec.height = h;
                // Unsafe allocation skips GuiGraphicsExtractor's constructor, which
                // sets every field vanilla code reads (both our overrides' super
                // paths and the text collector). Install the full set.
                // Unsafe allocation skips the constructor AND the field
                // initializers, so rec.poseStack is null too: allocate a real
                // stack and install it into both fields.
                Matrix3x2fStack stack = new Matrix3x2fStack(16);
                java.lang.reflect.Field ps =
                        RecordingExtractor.class.getDeclaredField("poseStack");
                ps.setAccessible(true);
                ps.set(rec, stack);
                install(rec, "pose", stack);
                // Same reason: instance initializers do not run either.
                java.lang.reflect.Field tooltip =
                        RecordingExtractor.class.getDeclaredField("tooltip");
                tooltip.setAccessible(true);
                tooltip.set(rec, new StringBuilder());
                install(rec, "scissorStack", newScissorStack(w, h));
                install(rec, "minecraft", mc);
                install(rec, "guiRenderState", RENDER_STATE);
                install(rec, "mouseX", mouseX);
                install(rec, "mouseY", mouseY);
                install(rec, "pendingCursor",
                    com.mojang.blaze3d.platform.cursor.CursorType.DEFAULT);
                try {
                    Object atlasManager = mc.getAtlasManager();
                    if (atlasManager != null) {
                    install(rec, "sprites", atlasManager);
                    install(rec, "guiSprites", atlasManager.getClass()
                        .getMethod("getAtlasOrThrow",
                            net.minecraft.resources.Identifier.class)
                        .invoke(atlasManager, net.minecraft.data.AtlasIds.GUI));
                    }
                } catch (Throwable t) {
                    log("atlas fields: " + t);
                }
                return rec;
            } catch (Throwable t) {
                throw new IllegalStateException("allocate recorder: " + t, t);
            }
        }

        private static Object newScissorStack(int w, int h) throws Exception {
            java.lang.reflect.Constructor<?> ssCtor = Class.forName(
                    "net.minecraft.client.gui.GuiGraphicsExtractor$ScissorStack")
                    .getDeclaredConstructor(
                            net.minecraft.client.gui.navigation.ScreenRectangle.class);
            ssCtor.setAccessible(true);
            net.minecraft.client.gui.navigation.ScreenRectangle screen =
                new net.minecraft.client.gui.navigation.ScreenRectangle(0, 0, w, h);
            Object stack = ssCtor.newInstance(screen);
            java.lang.reflect.Method push = stack.getClass().getDeclaredMethod(
                "push", net.minecraft.client.gui.navigation.ScreenRectangle.class);
            push.setAccessible(true);
            push.invoke(stack, screen);
            return stack;
        }

        private static void install(Object target, String name, Object value) throws Exception {
            java.lang.reflect.Field f = GuiGraphicsExtractor.class.getDeclaredField(name);
            f.setAccessible(true);
            f.set(target, value);
        }

        private static void emitTextC(int x, int y, int color, boolean shadow, String text) {
            cText++;
            recordFrameText(x, y, text);
            emitText(x, y, color, shadow, text);
        }

        @Override
        public int guiWidth() {
            return width;
        }

        @Override
        public int guiHeight() {
            return height;
        }

        @Override
        public Matrix3x2fStack pose() {
            return poseStack;
        }

        private void fillRect(int x0, int y0, int x1, int y1, int color) {
            if (x1 <= x0 || y1 <= y0) {
                return;
            }
            cFill++;
            emitFill(x0, y0, x1, y1, color);
        }

        /** counters so the R2 marker can audit what ran this frame */

        @Override
        public void fill(int x0, int y0, int x1, int y1, int color) {
            fillRect(x0, y0, x1, y1, color);
        }

        @Override
        public void fill(com.mojang.renderpearl.api.pipeline.RenderPipeline p,
                int x0, int y0, int x1, int y1, int color) {
            fillRect(x0, y0, x1, y1, color);
        }

        @Override
        public void fill(com.mojang.renderpearl.api.pipeline.RenderPipeline p,
                net.minecraft.client.gui.render.TextureSetup setup,
                int x0, int y0, int x1, int y1) {
            fillRect(x0, y0, x1, y1, 0xFFFFFFFF);
        }

        @Override
        public void fillGradient(int x0, int y0, int x1, int y1, int top, int bottom) {
            if (x1 <= x0 || y1 <= y0) {
                return;
            }
            cGrad++; emitFillGradient(x0, y0, x1, y1, top, bottom);
        }

        @Override
        public void horizontalLine(int x0, int y0, int x1, int color) {
            fillRect(x0, y0, x1, y0 + 1, color);
        }

        @Override
        public void verticalLine(int x0, int y0, int y1, int color) {
            fillRect(x0, y0, x0 + 1, y1, color);
        }

        @Override
        public void outline(int x0, int y0, int x1, int y1, int color) {
            fillRect(x0, y0, x1, y0 + 1, color);
            fillRect(x0, y1 - 1, x1, y1, color);
            fillRect(x0, y0, x0 + 1, y1, color);
            fillRect(x1 - 1, y0, x1, y1, color);
        }

        @Override
        public void textHighlight(int x0, int y0, int x1, int y1, boolean centered) {
            fillRect(x0, y0, x1, y1, 0x8010A050);
        }

        @Override
        public void text(Font font, String text, int x, int y, int color) {
            emitTextC(x, y, color, false, text);
        }

        @Override
        public void text(Font font, String text, int x, int y, int color, boolean shadow) {
            emitTextC(x, y, color, shadow, text);
        }

        @Override
        public void text(Font font, FormattedCharSequence seq, int x, int y, int color) {
            emitTextC(x, y, color, false, fcs(seq));
        }

        @Override
        public void text(Font font, FormattedCharSequence seq, int x, int y, int color, boolean shadow) {
            emitTextC(x, y, color, shadow, fcs(seq));
        }

        @Override
        public void text(Font font, Component component, int x, int y, int color) {
            emitTextC(x, y, color, false, component.getString());
        }

        @Override
        public void text(Font font, Component component, int x, int y, int color, boolean shadow) {
            emitTextC(x, y, color, shadow, component.getString());
        }

        @Override
        public void centeredText(Font font, String text, int x, int y, int color) {
            emitTextC(x - font.width(text) / 2, y, color, false, text);
        }

        @Override
        public void centeredText(Font font, Component component, int x, int y, int color) {
            emitTextC(x - font.width(component) / 2, y, color, false, component.getString());
        }

        @Override
        public void centeredText(Font font, FormattedCharSequence seq, int x, int y, int color) {
            emitTextC(x - font.width(seq) / 2, y, color, false, fcs(seq));
        }

        @Override
        public int textWithWordWrap(Font font, FormattedText text, int x, int y, int color, int maxWidth) {
            for (FormattedCharSequence line : font.split(text, maxWidth)) {
                emitTextC(x, y, color, false, fcs(line));
                y += font.lineHeight;
            }
            return y;
        }

        @Override
        public int textWithWordWrap(Font font, FormattedText text, int x, int y, int color,
                int maxWidth, boolean shadow) {
            for (FormattedCharSequence line : font.split(text, maxWidth)) {
                emitTextC(x, y, color, shadow, fcs(line));
                y += font.lineHeight;
            }
            return y;
        }

        @Override
        public void textWithBackdrop(Font font, Component component, int x, int y, int color,
                int backdropColor) {
            int w = font.width(component);
            fillRect(x - 2, y - 1, x + w + 1, y + font.lineHeight + 1, backdropColor);
            emitTextC(x, y, color, true, component.getString());
        }

        @Override
        public void blit(Identifier id, int x0, int y0, int w, int h, float u0, float v0,
                float u1, float v1) {
            captureDynamicTexture(id);
            cBlit++; emitBlit(id.toString(), x0, y0, w, h, u0, v0, u1, v1);
        }

        @Override
        public void blit(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, float u0, float v0, int w, int h, int tw, int th, int color) {
            captureDynamicTexture(id);
            cBlit++; emitBlit(id.toString(), x0, y0, w, h, u0 / tw, v0 / th, (u0 + w) / tw, (v0 + h) / th);
        }

        @Override
        public void blit(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, float u0, float v0, int w, int h, int tw, int th) {
            captureDynamicTexture(id);
            cBlit++; emitBlit(id.toString(), x0, y0, w, h, u0 / tw, v0 / th, (u0 + w) / tw, (v0 + h) / th);
        }

        @Override
        public void blit(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, float u0, float v0, int w, int h, int tw, int th, int u1, int v1) {
            captureDynamicTexture(id);
            cBlit++; emitBlit(id.toString(), x0, y0, w, h, u0 / tw, v0 / th, u1 / tw, v1 / th);
        }

        @Override
        public void blit(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, float u0, float v0, int w, int h, int tw, int th, int u1, int v1,
                int color) {
            captureDynamicTexture(id);
            cBlit++; emitBlit(id.toString(), x0, y0, w, h, u0 / tw, v0 / th, u1 / tw, v1 / th);
        }

        @Override
        public void blit(com.mojang.renderpearl.api.textures.GpuTextureView view,
                com.mojang.renderpearl.api.textures.GpuSampler sampler,
                int x0, int y0, int w, int h, float u0, float v0, float u1, float v1) {
            emitBlit("<gpu>", x0, y0, w, h, u0, v0, u1, v1);
        }

        @Override
        public void blitSprite(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, int w, int h) {
            cSprite++; emitSprite(id.toString(), x0, y0, w, h);
        }

        @Override
        public void blitSprite(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, int w, int h, int color) {
            cSprite++; emitSprite(id.toString(), x0, y0, w, h);
        }

        @Override
        public void blitSprite(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, int w, int h, int u0, int v0, int u1, int v1) {
            cSprite++; emitSprite(id.toString(), x0, y0, w, h);
        }

        @Override
        public void blitSprite(com.mojang.renderpearl.api.pipeline.RenderPipeline p, Identifier id,
                int x0, int y0, int w, int h, int u0, int v0, int u1, int v1, int color) {
            cSprite++; emitSprite(id.toString(), x0, y0, w, h);
        }

        @Override
        public void blitSprite(com.mojang.renderpearl.api.pipeline.RenderPipeline p,
                net.minecraft.client.renderer.texture.TextureAtlasSprite sprite,
                int x0, int y0, int w, int h) {
            cSprite++; emitSprite(sprite.contents().name().toString(), x0, y0, w, h);
        }

        @Override
        public void blitSprite(com.mojang.renderpearl.api.pipeline.RenderPipeline p,
                net.minecraft.client.renderer.texture.TextureAtlasSprite sprite,
                int x0, int y0, int w, int h, int color) {
            cSprite++; emitSprite(sprite.contents().name().toString(), x0, y0, w, h);
        }

        @Override
        public void enableScissor(int x0, int y0, int x1, int y1) {
            if (scissorDepth > 0) {
                emitScissorEnd();
            }
            super.enableScissor(x0, y0, x1, y1);
            scissorDepth++;
            emitCurrentScissor();
        }

        @Override
        public void disableScissor() {
            if (scissorDepth == 0) {
                return;
            }
            emitScissorEnd();
            super.disableScissor();
            scissorDepth--;
            if (scissorDepth > 0) {
                emitCurrentScissor();
            }
        }

        @Override
        public boolean containsPointInScissor(int x, int y) {
            return super.containsPointInScissor(x, y);
        }

        private void emitCurrentScissor() {
            try {
                java.lang.reflect.Field field = GuiGraphicsExtractor.class
                        .getDeclaredField("scissorStack");
                field.setAccessible(true);
                Object stack = field.get(this);
                java.lang.reflect.Method peek = stack.getClass().getDeclaredMethod("peek");
                peek.setAccessible(true);
                Object rect = peek.invoke(stack);
                net.minecraft.client.gui.navigation.ScreenRectangle screenRect =
                        (net.minecraft.client.gui.navigation.ScreenRectangle) rect;
                emitScissor(screenRect.left(), screenRect.top(), screenRect.right(), screenRect.bottom());
            } catch (ReflectiveOperationException e) {
                throw new IllegalStateException("read transformed GUI scissor", e);
            }
        }

        private void emitTooltipInternal() {
            if (tooltipX != Integer.MIN_VALUE && tooltip.length() > 0) {
                emitTooltip(tooltip.toString().stripTrailing(), tooltipX, tooltipY);
            }
            tooltip.setLength(0);
            tooltipX = Integer.MIN_VALUE;
            tooltipY = Integer.MIN_VALUE;
        }

        @Override
        public void setTooltipForNextFrame(Component component, int x, int y) {
            tooltip.append(component.getString()).append('\n');
            tooltipX = x;
            tooltipY = y;
        }

        @Override
        public void setTooltipForNextFrame(java.util.List<FormattedCharSequence> lines, int x, int y) {
            for (FormattedCharSequence line : lines) {
                tooltip.append(fcs(line)).append('\n');
            }
            tooltipX = x;
            tooltipY = y;
        }

        @Override
        public void setTooltipForNextFrame(Font font, Component component, int x, int y) {
            setTooltipForNextFrame(component, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font, Component component, int x, int y,
                Identifier icon) {
            setTooltipForNextFrame(component, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<net.minecraft.network.chat.Component> lines,
                java.util.Optional<net.minecraft.world.inventory.tooltip.TooltipComponent> component,
                int x, int y) {
            for (net.minecraft.network.chat.Component line : lines) {
                tooltip.append(line.getString()).append('\n');
            }
            tooltipX = x;
            tooltipY = y;
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<net.minecraft.network.chat.Component> lines,
                java.util.Optional<net.minecraft.world.inventory.tooltip.TooltipComponent> component,
                int x, int y, Identifier icon) {
            setTooltipForNextFrame(font, lines, component, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<net.minecraft.network.chat.Component> lines,
                java.util.Optional<net.minecraft.world.inventory.tooltip.TooltipComponent> component,
                int x, int y, Identifier icon, boolean iconAtFront) {
            setTooltipForNextFrame(font, lines, component, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<FormattedCharSequence> lines,
                java.util.Optional<net.minecraft.world.inventory.tooltip.TooltipComponent> component,
                net.minecraft.client.gui.screens.inventory.tooltip.ClientTooltipPositioner positioner,
                int x, int y, boolean iconAtFront, Identifier icon) {
            setTooltipForNextFrame(lines, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<? extends FormattedCharSequence> lines, int x, int y) {
            for (FormattedCharSequence line : lines) {
                tooltip.append(fcs(line)).append('\n');
            }
            tooltipX = x;
            tooltipY = y;
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<? extends FormattedCharSequence> lines, int x, int y,
                Identifier icon) {
            setTooltipForNextFrame(font, lines, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font,
                java.util.List<FormattedCharSequence> lines,
                net.minecraft.client.gui.screens.inventory.tooltip.ClientTooltipPositioner positioner,
                int x, int y, boolean iconAtFront) {
            setTooltipForNextFrame(lines, x, y);
        }

        @Override
        public void setTooltipForNextFrame(Font font, net.minecraft.world.item.ItemStack stack,
                int x, int y) {
            tooltip.append(stack.getHoverName().getString()).append('\n');
            tooltipX = x;
            tooltipY = y;
        }

        @Override
        public void setComponentTooltipForNextFrame(Font font,
                java.util.List<net.minecraft.network.chat.Component> lines, int x, int y) {
            setTooltipForNextFrame(font, lines, java.util.Optional.empty(), x, y);
        }

        @Override
        public void setComponentTooltipForNextFrame(Font font,
                java.util.List<net.minecraft.network.chat.Component> lines, int x, int y,
                Identifier icon) {
            setTooltipForNextFrame(font, lines, java.util.Optional.empty(), x, y);
        }

        @Override
        public void tooltip(Font font,
                java.util.List<net.minecraft.client.gui.screens.inventory.tooltip.ClientTooltipComponent> lines,
                int x, int y,
                net.minecraft.client.gui.screens.inventory.tooltip.ClientTooltipPositioner positioner,
                Identifier icon, boolean iconAtFront) {
            tooltipX = x;
            tooltipY = y;
            for (net.minecraft.client.gui.screens.inventory.tooltip.ClientTooltipComponent line : lines) {
                tooltip.append(line.toString()).append('\n');
            }
        }

        @Override
        public void extractDeferredElements(int mouseX, int mouseY, float partialTick) {
            emitTooltipInternal();
            while (scissorDepth > 0) {
                disableScissor();
            }
        }
    }
    /** Locate a Fabric keybinding by category and fire the tick so its real
     * handler opens its screen, exactly like a player pressing it. */
    static final class KeyBindingOpener {
        static boolean openByCategory(String categoryId) {
            try {
                Object binding = find(categoryId);
                mark("K1 find(\"" + categoryId + "\") -> "
                        + (binding == null ? "null" : binding.getClass().getName()));
                if (binding == null) {
                    log("no keybinding matched category " + categoryId);
                    return false;
                }
                press(binding);
                mark("K2 pressed isDown=" + isDownOf(binding));
                Object minecraft = Minecraft.getInstance();
                mark("K3 end-client-tick begin");
                net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents.END_CLIENT_TICK
                        .invoker()
                        .onEndTick((Minecraft) minecraft);
                mark("K4 end-client-tick done open=" + (current != null)
                        + " screen=" + currentScreen());
                if (current == null) {
                    log("keybinding " + categoryId + " was pressed but its end-client-tick handler did not open a screen");
                }
                return current != null;
            } catch (Throwable t) {
                System.err.println("[j2h-gui] keybinding open: " + t);
                mark("KX keybinding open exception " + t);
                t.printStackTrace();
                return false;
            }
        }

        /** Whether any registered keybinding's category or name mentions it. */
        static boolean registered(String categoryId) {
            try {
                return find(categoryId) != null;
            } catch (Throwable t) {
                mark("KX registered exception " + t);
                return false;
            }
        }

        private static boolean isDownOf(Object b) {
            try {
                Field f = b.getClass().getDeclaredField("isDown");
                f.setAccessible(true);
                return (Boolean) f.get(b);
            } catch (Throwable t) {
                return false;
            }
        }

        private static Object find(String categoryId) throws Exception {
            String needle = categoryId == null ? "" : categoryId.trim();
            String lowerNeedle = needle.toLowerCase(Locale.ROOT);
            String shortNeedle = lowerNeedle.startsWith("key.") ? lowerNeedle.substring(4) : lowerNeedle;
            String exactName = lowerNeedle.startsWith("key.") ? lowerNeedle : "key." + lowerNeedle;
            java.util.List<Object> all = ALL();
            for (Object b : all) {
                Object category = categoryOf(b);
                String catName = category == null ? "" : String.valueOf(category);
                String name = String.valueOf(nameOf(b));
                String lowerName = name.toLowerCase(Locale.ROOT);
                String lowerCat = catName.toLowerCase(Locale.ROOT);
                boolean match = lowerName.equals(lowerNeedle)
                        || lowerName.equals(exactName)
                        || lowerName.startsWith("key." + shortNeedle)
                        || lowerName.endsWith("." + shortNeedle)
                        || lowerName.contains(shortNeedle)
                        || lowerCat.contains(lowerNeedle)
                        || lowerCat.contains(shortNeedle)
                        || lowerName.contains(lowerNeedle);
                if (match) {
                    return b;
                }
            }
            if (!needle.isEmpty() && !lowerNeedle.startsWith("key.")) {
                return find("key." + needle);
            }
            return null;
        }

        private static java.util.List<Object> ALL() throws Exception {
            Field all = Class.forName("net.minecraft.client.KeyMapping")
                    .getDeclaredField("ALL");
            all.setAccessible(true);
            @SuppressWarnings("unchecked")
            Map<String, Object> map = (Map<String, Object>) all.get(null);
            return new java.util.ArrayList<>(map.values());
        }

        private static Object categoryOf(Object b) throws Exception {
            Method m = b.getClass().getMethod("getCategory");
            return m.invoke(b);
        }

        private static Object nameOf(Object b) throws Exception {
            Method m = b.getClass().getMethod("getName");
            return m.invoke(b);
        }

        private static void press(Object binding) throws Exception {
            Class<?> km = binding.getClass();
            // Read the mapping's bound key and press it through the loader's own
            // static entry points: click() bumps the click count the handler
            // reads (consumeClick()), set() flips isDown, exactly as a real key.
            Field keyField = km.getDeclaredField("key");
            keyField.setAccessible(true);
            Object key = keyField.get(binding);
            Method click = km.getMethod("click", key.getClass());
            click.invoke(null, key);
            Method set = km.getMethod("set", key.getClass(), boolean.class);
            set.invoke(null, key, true);
            Method setDown = km.getMethod("setDown", boolean.class);
            setDown.invoke(binding, true);
        }
    }
}
