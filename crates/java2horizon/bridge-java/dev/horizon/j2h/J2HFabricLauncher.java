package dev.horizon.j2h;

import java.io.InputStream;
import java.nio.file.Path;
import java.util.Collection;
import java.util.List;

import net.fabricmc.api.EnvType;
import net.fabricmc.loader.impl.launch.FabricLauncherBase;

/**
 * A no-op {@code FabricLauncher} for J2H's embedded JVM.
 *
 * Horizon is already the Minecraft runtime; J2H only asks Fabric Loader's
 * internals for environment facts, chiefly {@code isDevelopment()}, which
 * the base class reports from the unset {@code fabric.development} property.
 * The classpath hooks the real Knot loaders use are no-ops because J2H
 * drives its own {@code -Djava.class.path}.
 */
public final class J2HFabricLauncher extends FabricLauncherBase {
    private static volatile EnvType environmentType = EnvType.CLIENT;

    public static void setEnvironmentType(EnvType environmentType) {
        J2HFabricLauncher.environmentType = environmentType;
    }

    @Override
    public void addToClassPath(Path path, String... allowedPrefixes) {
    }

    @Override
    public void setAllowedPrefixes(Path path, String... prefixes) {
    }

    @Override
    public void setValidParentClassPath(Collection<Path> paths) {
    }

    @Override
    public EnvType getEnvironmentType() {
        return environmentType;
    }

    @Override
    public boolean isClassLoaded(String name) {
        return false;
    }

    @Override
    public Class<?> loadIntoTarget(String name) throws ClassNotFoundException {
        return Class.forName(name, false, ClassLoader.getSystemClassLoader());
    }

    @Override
    public InputStream getResourceAsStream(String name) {
        return ClassLoader.getSystemResourceAsStream(name);
    }

    @Override
    public ClassLoader getTargetClassLoader() {
        return ClassLoader.getSystemClassLoader();
    }

    @Override
    public byte[] getClassByteArray(String name, boolean resolve) throws java.io.IOException {
        String path = name.replace('.', '/') + ".class";
        try (InputStream in = ClassLoader.getSystemResourceAsStream(path)) {
            if (in == null) {
                throw new java.io.FileNotFoundException(path);
            }
            return in.readAllBytes();
        }
    }

    @Override
    public java.util.jar.Manifest getManifest(Path path) {
        try (InputStream in = java.nio.file.Files.newInputStream(path)) {
            return new java.util.jar.Manifest(in);
        } catch (java.io.IOException e) {
            return null;
        }
    }

    @Override
    public String getEntrypoint() {
        return null;
    }

    @Override
    public List<Path> getClassPath() {
        return List.of();
    }
}
