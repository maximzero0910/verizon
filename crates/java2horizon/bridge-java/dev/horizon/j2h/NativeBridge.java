package dev.horizon.j2h;

public final class NativeBridge {
    private NativeBridge() {}

    public static native void log(String message);

    public static native void registerBlock(
            String id, String name, String template, float hardness, int light, boolean withItem);

    public static native void registerItem(
            String id, String name, String template, int maxStack);

    public static native void registerEntityType(
            String id, String name, String template, float width);

    public static native void registerMobEffect(
            String id, String name, int color, String template);

    public static native void registerCreativeTab(
            String id, String name, String icon, String parent);

    public static native void registerMenuType(
            String id, int rows, int slotCount, String title);

    public static native void registerCommand(String name, String permission);

    public static native void lifecycleEvent(String event, String modId);

    public static native void entrypointComplete(String modId);
}
