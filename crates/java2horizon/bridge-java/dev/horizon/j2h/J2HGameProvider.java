package dev.horizon.j2h;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Collection;
import java.util.List;
import java.util.Set;
import java.util.zip.ZipFile;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import net.fabricmc.loader.api.metadata.ModMetadata;
import net.fabricmc.loader.api.metadata.ModDependency;
import net.fabricmc.loader.impl.game.GameProvider;
import net.fabricmc.loader.impl.game.patch.GameTransformer;
import net.fabricmc.loader.impl.launch.FabricLauncher;
import net.fabricmc.loader.impl.metadata.BuiltinModMetadata;
import net.fabricmc.loader.impl.metadata.ModDependencyImpl;
import net.fabricmc.loader.impl.util.Arguments;

/** Fabric Loader provider facts for Horizon's pinned Minecraft runtime. */
public final class J2HGameProvider implements GameProvider {
    private final String version;
    private final Path clientJar;
    private final Path gameDir;
    private final Collection<BuiltinMod> builtinMods;
    private final Arguments arguments;
    private final int minimumJavaVersion;

    public J2HGameProvider(String version, Path clientJar, Path gameDir) {
        if (version == null || version.isBlank()) {
            throw new IllegalArgumentException("Minecraft version must not be empty");
        }
        if (!Files.isRegularFile(clientJar)) {
            throw new IllegalArgumentException("Minecraft client jar is missing: " + clientJar);
        }
        this.version = version;
        this.clientJar = clientJar.toAbsolutePath().normalize();
        this.minimumJavaVersion = readAndValidateGameVersion(this.clientJar, version);
        this.gameDir = (gameDir == null ? Path.of(".") : gameDir).toAbsolutePath().normalize();
        ModMetadata minecraft;
        try {
            minecraft = new BuiltinModMetadata.Builder("minecraft", version)
                .setName("Minecraft")
                .setDescription("Horizon runtime using Minecraft Java " + version)
                .addDependency(new ModDependencyImpl(
                        ModDependency.Kind.DEPENDS,
                        "java",
                        List.of(">=" + minimumJavaVersion)))
                .build();
        } catch (Exception e) {
            throw new IllegalArgumentException("could not build Minecraft metadata for " + version, e);
        }
        this.builtinMods = List.of(new BuiltinMod(List.of(this.clientJar), minecraft));
        this.arguments = new Arguments();
        this.arguments.put(Arguments.GAME_VERSION, version);
    }

    private static int readAndValidateGameVersion(Path clientJar, String expectedVersion) {
        try (ZipFile jar = new ZipFile(clientJar.toFile())) {
            var entry = jar.getEntry("version.json");
            if (entry == null) {
                throw new IllegalArgumentException("Minecraft client jar has no version.json: " + clientJar);
            }
            try (var reader = new java.io.InputStreamReader(jar.getInputStream(entry), java.nio.charset.StandardCharsets.UTF_8)) {
                JsonObject metadata = JsonParser.parseReader(reader).getAsJsonObject();
                String jarVersion = metadata.get("id").getAsString();
                if (!expectedVersion.equals(jarVersion)) {
                    throw new IllegalArgumentException("Minecraft client jar version mismatch: expected "
                            + expectedVersion + ", got " + jarVersion + " from " + clientJar);
                }
                int javaVersion = metadata.get("java_version").getAsInt();
                if (javaVersion < 1) {
                    throw new IllegalArgumentException("invalid Minecraft java_version " + javaVersion
                            + " in " + clientJar);
                }
                return javaVersion;
            }
        } catch (java.io.IOException | RuntimeException e) {
            if (e instanceof IllegalArgumentException) {
                throw (IllegalArgumentException) e;
            }
            throw new IllegalArgumentException("could not validate Minecraft version metadata in " + clientJar, e);
        }
    }

    static void validateClientJarVersion(Path clientJar, String expectedVersion) {
        readAndValidateGameVersion(clientJar, expectedVersion);
    }

    @Override public String getGameId() { return "minecraft"; }
    @Override public String getGameName() { return "Minecraft"; }
    @Override public String getRawGameVersion() { return version; }
    @Override public String getNormalizedGameVersion() { return version; }
    @Override public Collection<BuiltinMod> getBuiltinMods() { return builtinMods; }
    @Override public String getEntrypoint() { return null; }
    @Override public Path getLaunchDirectory() { return gameDir; }
    @Override public boolean requiresUrlClassLoader() { return false; }
    @Override public Set<BuiltinTransform> getBuiltinTransforms(String namespace) { return Set.of(); }
    @Override public boolean isEnabled() { return true; }
    @Override public boolean locateGame(FabricLauncher launcher, String[] args) { return false; }
    @Override public void initialize(FabricLauncher launcher) { }
    @Override public GameTransformer getEntrypointTransformer() { return new GameTransformer(); }
    @Override public void unlockClassPath(FabricLauncher launcher) { }
    @Override public void launch(ClassLoader loader) {
        throw new UnsupportedOperationException("Horizon owns Minecraft launch; Fabric must not launch it");
    }
    @Override public Arguments getArguments() { return arguments; }
    @Override public String[] getLaunchArguments(boolean sanitize) { return new String[0]; }
}
