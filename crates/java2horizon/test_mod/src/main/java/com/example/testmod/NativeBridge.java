package com.example.testmod;

public final class NativeBridge {
    private NativeBridge() {}
    public static native void log(String msg);
    public static native void registerBlock(String id, String name, String template, float hardness, int light, boolean withItem);
    public static native void registerItem(String id, String name, String template, int maxStack);
    public static native void entrypointComplete(String modId);
}
