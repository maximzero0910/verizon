package dev.horizon.j2h;

import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.Optional;
import java.util.UUID;

import net.minecraft.SharedConstants;
import net.minecraft.WorldVersion;
import net.minecraft.client.Minecraft;
import net.minecraft.client.User;

/**
 * Establish the minimum genuine Minecraft client state Fabric client mods
 * expect before their entrypoints run: the {@code Minecraft} singleton exists
 * in its pre-options state ({@code options == null}) and {@code SharedConstants}
 * reports the real game version.
 *
 * <p>The windowed client normally creates the singleton inside
 * {@code Minecraft(GameConfig)} and later loads {@code Options}. J2H runs no
 * window/GL initialisation, so it cannot run that constructor. Instead it
 * allocates an instance without running the constructor
 * ({@code Unsafe.allocateInstance}) and publishes it to the static
 * {@code instance} field that {@code Minecraft.getInstance()} reads. The
 * {@code final} {@code options} field therefore stays at its JVM default of
 * {@code null} — exactly the pre-options state Fabric's
 * {@code KeyMappingRegistryImpl.registerKeyMapping} requires. J2H constructs
 * the real {@code User} value for the current Horizon profile and installs it
 * into the singleton; fields that require a window or GL context stay at their
 * JVM defaults. No exception is swallowed and no stand-in class is substituted.
 *
 * <p>Game version detection is the loader's own
 * {@code SharedConstants.tryDetectVersion()}, which reads {@code /version.json}
 * out of the real 26.3 client jar on the classpath; no version object is
 * fabricated.
 */
public final class MinecraftBootstrap {
    private MinecraftBootstrap() {
    }

    private static boolean bootstrapped;

    private static final Field INSTANCE = field(Minecraft.class, "instance");
    private static final Field USER = field(Minecraft.class, "user");
    private static final Method TRY_DETECT_VERSION = method(SharedConstants.class, "tryDetectVersion");

    private static Field field(Class<?> owner, String name) {
        try {
            Field f = owner.getDeclaredField(name);
            f.setAccessible(true);
            return f;
        } catch (NoSuchFieldException e) {
            throw new IllegalStateException("client layout changed: " + owner.getName() + "." + name, e);
        }
    }

    private static Method method(Class<?> owner, String name) {
        try {
            Method m = owner.getDeclaredMethod(name);
            m.setAccessible(true);
            return m;
        } catch (NoSuchMethodException e) {
            throw new IllegalStateException("client layout changed: " + owner.getName() + "." + name, e);
        }
    }

    /**
     * Publish the real game version and the pre-options {@code Minecraft}
     * singleton. Idempotent: nothing is replaced once present.
     */
    public static void install() {
        String uuid = UUID.nameUUIDFromBytes(
                "OfflinePlayer:Player".getBytes(java.nio.charset.StandardCharsets.UTF_8)).toString();
        installWithIdentity("Player", uuid);
    }

    /**
     * Install the real local profile before client entrypoints run. The token
     * and optional online identity fields stay empty: J2H does not provide
     * Minecraft authentication or authenticated networking to Java mods.
     */
    public static void installWithIdentity(String name, String uuidText) {
        if (name == null || name.isBlank()) {
            throw new IllegalArgumentException("local Minecraft profile name is empty");
        }
        UUID profileUuid;
        try {
            profileUuid = UUID.fromString(uuidText);
        } catch (RuntimeException e) {
            throw new IllegalArgumentException("invalid local Minecraft profile UUID: " + uuidText, e);
        }
        try {
            TRY_DETECT_VERSION.invoke(null);
        } catch (InvocationTargetException e) {
            throw new IllegalStateException("could not detect the game version: " + e.getCause(), e.getCause());
        } catch (IllegalAccessException e) {
            throw new IllegalStateException("could not detect the game version: " + e, e);
        }
        if (SharedConstants.getCurrentVersion() == null) {
            throw new IllegalStateException("game version not set after tryDetectVersion");
        }
        if (!bootstrapped) {
            net.minecraft.server.Bootstrap.bootStrap();
            net.minecraft.client.ClientBootstrap.bootstrap();
            net.minecraft.server.Bootstrap.validate();
            bootstrapped = true;
        }
        try {
            Minecraft instance = (Minecraft) INSTANCE.get(null);
            if (instance == null) {
                // A real Minecraft subclass whose renderFrame belongs to Horizon
                // and whose setScreenAndShow hands screens to the GUI bridge.
                instance = (Minecraft) J2HUnsafe.allocate(J2HMinecraft.class);
                J2HUnsafe.setInstanceField(
                        instance, USER, new User(name, profileUuid, "", Optional.empty(), Optional.empty()));
                INSTANCE.set(null, instance);
            } else {
                User user = (User) USER.get(instance);
                if (user == null) {
                    J2HUnsafe.setInstanceField(
                            instance, USER, new User(name, profileUuid, "", Optional.empty(), Optional.empty()));
                } else if (!user.getName().equals(name) || !user.getProfileId().equals(profileUuid)) {
                    throw new IllegalStateException(
                            "Minecraft singleton already has a different local profile");
                }
            }
        } catch (Throwable t) {
            throw new IllegalStateException("could not establish the Minecraft singleton: " + t, t);
        }
    }

    /** Whether the singleton is published and still in its pre-options state. */
    public static boolean minecraftReady() {
        Minecraft instance = Minecraft.getInstance();
        return instance != null && instance.options == null && instance.getUser() != null;
    }

    /** Observable bootstrap assertion used by the embedded-runtime regression. */
    public static boolean identityMatches(String name, String uuidText) {
        Minecraft instance = Minecraft.getInstance();
        if (instance == null || instance.getUser() == null) {
            return false;
        }
        User user = instance.getUser();
        return user.getName().equals(name) && user.getProfileId().toString().equals(uuidText)
                && user.getAccessToken().isEmpty() && user.getXuid().isEmpty() && user.getClientId().isEmpty();
    }

    /** The detected game version id (for example {@code "26.3"}). */
    public static String currentVersionId() {
        WorldVersion version = SharedConstants.getCurrentVersion();
        return version == null ? "" : version.id();
    }
}
