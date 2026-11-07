package dev.horizon.j2h;

import java.lang.reflect.Field;
import java.lang.reflect.Modifier;

/**
 * Shared {@code sun.misc.Unsafe} access for instantiating Minecraft classes
 * whose constructors need an OpenGL/RenderPearl context Horizon cannot drive.
 * The object is a real instance of the class because its superclass contract
 * and virtual dispatch remain intact; only the fields that would require a GL
 * context stay at their JVM defaults, and the instance is driven exclusively
 * through overridden methods that never touch those internals.
 */
final class J2HUnsafe {
    private J2HUnsafe() {
    }

    private static final Object UNSAFE;

    static {
        try {
            Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
            f.setAccessible(true);
            UNSAFE = f.get(null);
        } catch (Throwable t) {
            throw new IllegalStateException("no Unsafe", t);
        }
    }

    static Object allocate(Class<?> type) throws Exception {
        java.lang.reflect.Method allocate = sun.misc.Unsafe.class
                .getDeclaredMethod("allocateInstance", Class.class);
        return allocate.invoke(UNSAFE, type);
    }

    /** Set a constructed value into a final instance field on an allocated host object. */
    static void setInstanceField(Object instance, Field field, Object value) throws Exception {
        if (Modifier.isStatic(field.getModifiers()) || !field.getDeclaringClass().isInstance(instance)) {
            throw new IllegalArgumentException("field is not an instance field of the target object");
        }
        if (value != null && !field.getType().isInstance(value)) {
            throw new IllegalArgumentException("value does not match field type " + field.getType().getName());
        }
        Class<?> unsafeType = Class.forName("sun.misc.Unsafe");
        long offset = (long) unsafeType.getMethod("objectFieldOffset", Field.class).invoke(UNSAFE, field);
        unsafeType.getMethod("putObject", Object.class, long.class, Object.class)
                .invoke(UNSAFE, instance, offset, value);
    }
}
