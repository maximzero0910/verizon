package dev.horizon.j2h;

import java.io.InputStream;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;

import net.fabricmc.api.EnvType;
import net.fabricmc.loader.api.ModContainer;
import net.fabricmc.loader.api.Version;
import net.fabricmc.loader.api.VersionParsingException;
import net.fabricmc.loader.impl.FabricLoaderImpl;
import net.fabricmc.loader.impl.ModContainerImpl;
import net.fabricmc.loader.impl.discovery.ModCandidateImpl;
import net.fabricmc.loader.impl.game.GameProvider;
import net.fabricmc.loader.impl.entrypoint.EntrypointStorage;
import net.fabricmc.loader.impl.launch.FabricLauncherBase;
import net.fabricmc.loader.impl.launch.FabricMixinBootstrap;
import net.fabricmc.loader.impl.metadata.DependencyOverrides;
import net.fabricmc.loader.impl.metadata.EntrypointMetadata;
import net.fabricmc.loader.impl.metadata.LoaderModMetadata;
import net.fabricmc.loader.impl.metadata.ModMetadataParser;
import net.fabricmc.loader.impl.metadata.VersionOverrides;
import org.spongepowered.asm.mixin.Mixins;

/**
 * Bootstrap the real Fabric Loader singleton with J2H's own discovery.
 *
 * J2H parses {@code fabric.mod.json} itself, but Fabric mods expect
 * {@code FabricLoader.getInstance()} (the loader's {@code INSTANCE}) to
 * answer {@code isModLoaded}, {@code getAllMods} and
 * {@code getEntrypointContainers}. Instead of re-implementing those answers,
 * this class feeds J2H's jars through the loader's own metadata parser and
 * {@code addMod} path, so the singleton's {@code modMap}, {@code mods} and
 * {@code entrypointStorage} are genuinely populated.
 */
public final class FabricLoaderBootstrap {
    private FabricLoaderBootstrap() {
    }

    private static final Method PARSE_METADATA = method(
            ModMetadataParser.class, "parseMetadata",
            InputStream.class, String.class, List.class,
            VersionOverrides.class, DependencyOverrides.class, boolean.class);
    private static final Method CREATE_PLAIN = method(
            ModCandidateImpl.class, "createPlain",
            List.class, LoaderModMetadata.class, boolean.class, java.util.Collection.class);
    private static final Method CREATE_BUILTIN = method(
            ModCandidateImpl.class, "createBuiltin",
            GameProvider.BuiltinMod.class, VersionOverrides.class, DependencyOverrides.class);
    private static final Method ADD_MOD = method(FabricLoaderImpl.class, "addMod", ModCandidateImpl.class);
    private static final Method SETUP_LANGUAGE_ADAPTERS = method(FabricLoaderImpl.class, "setupLanguageAdapters");
    private static final Method ADD_ENTRYPOINT = method(
            EntrypointStorage.class, "add",
            ModContainerImpl.class, String.class, EntrypointMetadata.class, Map.class);
    private static final Method GET_ENTRYPOINT_CONTAINERS = method(
            EntrypointStorage.class, "getEntrypointContainers", String.class, Class.class);
    private static final Method SET_GAME_DIR = method(FabricLoaderImpl.class, "setGameDir", Path.class);
    private static boolean mixinsInitialized;
    private static boolean gameProviderInitialized;

    private static Method method(Class<?> owner, String name, Class<?>... params) {
        try {
            Method m = owner.getDeclaredMethod(name, params);
            m.setAccessible(true);
            return m;
        } catch (NoSuchMethodException e) {
            throw new IllegalStateException("loader layout changed: " + owner.getName() + "." + name, e);
        }
    }

    /** Register the loader launcher so environment queries do not NPE. */
    public static void installLauncher() {
        try {
            Method setProperties = FabricLauncherBase.class.getDeclaredMethod("setProperties", Map.class);
            setProperties.setAccessible(true);
            setProperties.invoke(null, new java.util.HashMap<String, Object>());
        } catch (Exception e) {
            throw new IllegalStateException("could not initialize Fabric launcher properties: " + e, e);
        }
        FabricLauncherBase.setLauncher(new J2HFabricLauncher());
    }

    /** Point the loader at the game folder; its `config/` subfolder follows. */
    public static void setGameDir(String gameDir) {
        try {
            SET_GAME_DIR.invoke(FabricLoaderImpl.INSTANCE, Path.of(gameDir));
        } catch (InvocationTargetException e) {
            throw new IllegalStateException("loader rejected game dir " + gameDir + ": " + e.getCause(), e.getCause());
        } catch (IllegalAccessException e) {
            throw new IllegalStateException("could not set game dir: " + e, e);
        }
    }

    /**
     * Install the pinned Horizon runtime as Fabric's real built-in Minecraft
     * mod. This follows Fabric Loader's provider -> BuiltinMod -> candidate ->
     * ModContainer path instead of synthesizing a container outside discovery.
     */
    public static synchronized void initializeGameProvider(String version, String clientJarPath) {
        if (gameProviderInitialized) {
            throw new IllegalStateException("J2H game provider was already initialized");
        }
        FabricLoaderImpl loader = FabricLoaderImpl.INSTANCE;
        try {
            J2HGameProvider provider = new J2HGameProvider(
                    version,
                    Path.of(clientJarPath),
                    loader.getGameDir());
            loader.setGameProvider(provider);
            GameProvider.BuiltinMod minecraft = provider.getBuiltinMods().iterator().next();
            ModCandidateImpl candidate = (ModCandidateImpl) CREATE_BUILTIN.invoke(
                    null, minecraft, new VersionOverrides(), emptyDependencyOverrides());
            ADD_MOD.invoke(loader, candidate);
            gameProviderInitialized = true;
            System.err.println("J2H: Fabric Minecraft built-in registered at " + version);
        } catch (InvocationTargetException e) {
            throw new IllegalStateException("could not register Fabric Minecraft built-in: " + e.getCause(), e.getCause());
        } catch (Exception e) {
            throw new IllegalStateException("could not initialize Fabric Minecraft provider: " + e, e);
        }
    }

    private static DependencyOverrides emptyDependencyOverrides() {
        // The loader reads $configDir/dependencyOverrides.json; a path that
        // does not exist yields no overrides.
        return new DependencyOverrides(Path.of("j2h-dependency-overrides-none.json"));
    }

    private static LoaderModMetadata parseMetadata(Path jar) throws Exception {
        try (java.util.zip.ZipFile zf = new java.util.zip.ZipFile(jar.toFile())) {
            java.util.zip.ZipEntry entry = zf.getEntry("fabric.mod.json");
            if (entry == null) {
                throw new IllegalStateException("no fabric.mod.json in " + jar);
            }
            try (InputStream in = zf.getInputStream(entry)) {
                return (LoaderModMetadata) PARSE_METADATA.invoke(
                        null, in, jar.toString() + "!fabric.mod.json",
                        List.of(), new VersionOverrides(), emptyDependencyOverrides(), false);
            }
        }
    }

    /** Register one discovered mod jar on the loader singleton. */
    public static void registerMod(String jarPath) {
        FabricLoaderImpl loader = FabricLoaderImpl.INSTANCE;
        try {
            Path jar = Path.of(jarPath);
            LoaderModMetadata metadata = parseMetadata(jar);
            ModCandidateImpl candidate = (ModCandidateImpl) CREATE_PLAIN.invoke(
                    null, List.of(jar), metadata, false, List.of());
            ADD_MOD.invoke(loader, candidate);
        } catch (InvocationTargetException e) {
            throw new IllegalStateException("loader rejected " + jarPath + ": " + e.getCause(), e.getCause());
        } catch (Exception e) {
            throw new IllegalStateException("could not bootstrap " + jarPath + ": " + e, e);
        }
    }

    /**
     * One shot after every mod is registered: install language adapters and
     * feed each mod's entrypoints into the storage. Called once, the way the
     * loader itself does in {@code load()}, so a mod that declares a custom
     * language adapter cannot collide on a repeated setup pass.
     */
    public static void finalizeLoader() {
        FabricLoaderImpl loader = FabricLoaderImpl.INSTANCE;
        try {
            SETUP_LANGUAGE_ADAPTERS.invoke(loader);
            EntrypointStorage storage = entrypointStorage();
            Map<String, Object> adapters = adapterMap();
            for (ModContainer mc : loader.getMods()) {
                if (!(mc instanceof ModContainerImpl)) {
                    continue;
                }
                LoaderModMetadata metadata = ((ModContainerImpl) mc).getInfo();
                if (metadata == null) {
                    continue;
                }
                for (String key : metadata.getEntrypointKeys()) {
                    for (EntrypointMetadata ep : metadata.getEntrypoints(key)) {
                        ADD_ENTRYPOINT.invoke(storage, mc, key, ep, adapters);
                    }
                }
            }
        } catch (InvocationTargetException e) {
            throw new IllegalStateException("loader finalize: " + e.getCause(), e.getCause());
        } catch (Exception e) {
            throw new IllegalStateException("loader finalize: " + e, e);
        }
    }

    /** Initialize Mixin and attach its transformer before Minecraft classes load. */
    public static synchronized void initializeMixins(String environment) {
        if (mixinsInitialized) return;
        EnvType type = "server".equalsIgnoreCase(environment) ? EnvType.SERVER : EnvType.CLIENT;
        J2HFabricLauncher.setEnvironmentType(type);
        try {
            FabricMixinBootstrap.init(type, FabricLoaderImpl.INSTANCE);

            // Fabric Loader normally discovers MixinExtras as a nested mod and
            // initializes it from its mixin config plugin. J2H registers the
            // nested libraries on the JVM classpath but does not expose each
            // nested mod to Fabric's mod metadata, so install its injection
            // points explicitly before Fabric mixin configs are prepared.
            try {
                Class<?> mixinExtras = Class.forName(
                        "com.llamalad7.mixinextras.MixinExtrasBootstrap",
                        true,
                        FabricLoaderBootstrap.class.getClassLoader());
                mixinExtras.getMethod("init").invoke(null);
                System.err.println("J2H: MixinExtras bootstrap ready");
            } catch (ClassNotFoundException ignored) {
                // MixinExtras is optional for mods that do not declare it.
            }

            if (type == EnvType.CLIENT) {
                Mixins.addConfiguration("j2h.mixins.json");
            }

            Method finish = FabricLauncherBase.class.getDeclaredMethod("finishMixinBootstrapping");
            finish.setAccessible(true);
            finish.invoke(null);

            AccessWidenerAgent.installMixinTransformer();
            mixinsInitialized = true;
            System.err.println("J2H: Fabric Mixin bootstrap ready for " + type.name().toLowerCase());
        } catch (InvocationTargetException e) {
            throw new IllegalStateException("Fabric Mixin bootstrap failed: " + e.getCause(), e.getCause());
        } catch (Exception e) {
            throw new IllegalStateException("Fabric Mixin bootstrap failed: " + e, e);
        }
    }

    private static EntrypointStorage entrypointStorage()
            throws InvocationTargetException, IllegalAccessException {
        return getField("entrypointStorage");
    }

    private static Map<String, Object> adapterMap()
            throws InvocationTargetException, IllegalAccessException {
        return getField("adapterMap");
    }

    private static <T> T getField(String name) throws IllegalAccessException {
        try {
            Field f = FabricLoaderImpl.class.getDeclaredField(name);
            f.setAccessible(true);
            @SuppressWarnings("unchecked")
            T value = (T) f.get(FabricLoaderImpl.INSTANCE);
            return value;
        } catch (NoSuchFieldException e) {
            throw new IllegalStateException("loader layout changed: FabricLoaderImpl." + name, e);
        }
    }

    /** Verify the loader singleton answered from J2H's discovery. */
    public static boolean modLoaded(String id) {
        return FabricLoaderImpl.INSTANCE.isModLoaded(id);
    }

    public static int modCount() {
        return (int) FabricLoaderImpl.INSTANCE.getModsInternal().stream()
                .filter(mod -> !"minecraft".equals(mod.getMetadata().getId()))
                .count();
    }

    public static String minecraftMetadataVersion() {
        return FabricLoaderImpl.INSTANCE.getModContainer("minecraft")
                .orElseThrow(() -> new IllegalStateException("Fabric minecraft ModContainer is missing"))
                .getMetadata().getVersion().getFriendlyString();
    }

    public static boolean minecraftJavaDependencyAccepts(String javaVersion) {
        ModContainer minecraft = FabricLoaderImpl.INSTANCE.getModContainer("minecraft")
                .orElseThrow(() -> new IllegalStateException("Fabric minecraft ModContainer is missing"));
        Version parsedVersion;
        try {
            parsedVersion = Version.parse(javaVersion);
        } catch (VersionParsingException e) {
            throw new IllegalArgumentException("invalid Java version " + javaVersion, e);
        }
        return minecraft.getMetadata().getDepends().stream()
                .filter(dependency -> "java".equals(dependency.getModId()))
                .anyMatch(dependency -> dependency.matches(parsedVersion));
    }

    public static void validateClientJarVersion(String clientJarPath, String expectedVersion) {
        J2HGameProvider.validateClientJarVersion(Path.of(clientJarPath), expectedVersion);
    }

    public static String rawGameVersion() {
        return FabricLoaderImpl.INSTANCE.getRawGameVersion();
    }

    public static int entrypointContainerCount(String key, Class<?> type) {
        try {
            return ((List<?>) GET_ENTRYPOINT_CONTAINERS.invoke(entrypointStorage(), key, type)).size();
        } catch (Exception e) {
            return -1;
        }
    }
}
