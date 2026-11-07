package dev.horizon.j2h;

import java.lang.reflect.Field;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;

import com.mojang.renderpearl.api.device.GpuDevice;
import com.mojang.renderpearl.api.device.DeviceInfo;
import com.mojang.renderpearl.api.device.DeviceLimits;

/**
 * No-op RenderPearl GPU objects for a JVM that has no rendering device.
 *
 * Vanilla resource paths that back screen content with real textures, such
 * as fabric mod icons ({@code DynamicTexture}), ask {@code RenderSystem}
 * for the ambient {@link GpuDevice} to allocate textures and views. The
 * embedded JVM cannot run Horizon's wgpu backend, so this installs proxies
 * whose every allocation is an inert stub. Pixels never reach the screen
 * through them, but the resource path completes and the extraction can
 * continue with the sprite/atlas content Horizon renders itself.
 */
public final class GpuStub {
    private GpuStub() {
    }

    private static final DeviceInfo DEVICE_INFO = new DeviceInfo(
            "J2H GUI extraction stub",
            "Horizon",
            "No GPU access from embedded GUI extraction",
            false,
            "none",
            0.0f,
            new DeviceLimits(1, 1, 16384, Long.MAX_VALUE, 1, 1, 1),
            null,
            java.util.Set.of(),
            null,
            null);

    private static final InvocationHandler HANDLER = new InvocationHandler() {
        @Override
        public Object invoke(Object proxy, Method method, Object[] args) {
            String name = method.getName();
            switch (name) {
                case "createTexture":
                    return stub("com.mojang.renderpearl.api.textures.GpuTexture");
                case "createTextureView":
                    return stub("com.mojang.renderpearl.api.textures.GpuTextureView");
                case "texture":
                    return stub("com.mojang.renderpearl.api.textures.GpuTexture");
                case "createCommandEncoder":
                    return stub("com.mojang.renderpearl.api.commands.CommandEncoder");
                case "createRenderPass":
                    return stub("com.mojang.renderpearl.api.commands.RenderPass");
                case "transientMemory":
                    return stub("com.mojang.renderpearl.api.buffers.TransientMemory");
                case "multiUploadStaging":
                case "multiUploadGpu":
                    return bufferSlices(args);
                case "slice":
                    if (proxy instanceof com.mojang.renderpearl.api.buffers.GpuBuffer) {
                        return new com.mojang.renderpearl.api.buffers.GpuBufferSlice(
                                (com.mojang.renderpearl.api.buffers.GpuBuffer) proxy,
                                ((Number) args[0]).longValue(),
                                ((Number) args[1]).longValue());
                    }
                    return defaultValue(method);
                case "createSampler":
                case "getSampler":
                case "getClampToEdge":
                case "getRepeat":
                    return stub("com.mojang.renderpearl.api.textures.GpuSampler");
                case "createBuffer":
                    return stub("com.mojang.renderpearl.api.buffers.GpuBuffer");
                case "createSurface":
                    return stub("com.mojang.renderpearl.api.device.GpuSurface");
                case "createTimestampQueryPool":
                    return stub("com.mojang.renderpearl.api.commands.GpuQueryPool");
                case "compilePipeline":
                    return java.util.concurrent.CompletableFuture.completedFuture(null);
                case "getLastDebugMessages":
                    return java.util.List.of();
                case "getDeviceInfo":
                    return DEVICE_INFO;
                case "isDebuggingEnabled":
                case "isClosed":
                    return Boolean.FALSE;
                case "getMaxLod":
                    return java.util.OptionalDouble.of(1.0);
                default:
                    return defaultValue(method);
            }
        }

        private Object bufferSlices(Object[] args) {
            int count = args != null && args.length > 0 && args[0] instanceof java.util.List
                    ? ((java.util.List<?>) args[0]).size()
                    : 0;
            java.util.List<com.mojang.renderpearl.api.buffers.GpuBufferSlice> out =
                    new java.util.ArrayList<>(count);
            com.mojang.renderpearl.api.buffers.GpuBuffer buffer =
                    (com.mojang.renderpearl.api.buffers.GpuBuffer) stub(
                            "com.mojang.renderpearl.api.buffers.GpuBuffer");
            for (int i = 0; i < count; i++) {
                out.add(new com.mojang.renderpearl.api.buffers.GpuBufferSlice(buffer, 0L, 0L));
            }
            return out;
        }

        private Object defaultValue(Method method) {
            Class<?> rt = method.getReturnType();
            if (rt == boolean.class) {
                return Boolean.FALSE;
            }
            if (rt == int.class) {
                return Integer.valueOf(0);
            }
            if (rt == long.class) {
                return Long.valueOf(0);
            }
            if (rt == float.class) {
                return Float.valueOf(0f);
            }
            if (rt == double.class) {
                return Double.valueOf(0d);
            }
            return null;
        }
    };

    /** Install the stub device and sampler cache on {@code RenderSystem}. */
    public static void install() {
        try {
            Class<?> rs = Class.forName("com.mojang.blaze3d.systems.RenderSystem");
            Field device = rs.getDeclaredField("DEVICE");
            device.setAccessible(true);
            if (device.get(null) == null) {
                device.set(null, stub("com.mojang.renderpearl.api.device.GpuDevice"));
                GuiBridge.mark("S7 gpu stub installed");
            }
            Field samplerCache = rs.getDeclaredField("samplerCache");
            samplerCache.setAccessible(true);
            if (samplerCache.get(null) == null) {
                samplerCache.set(null, new StubSamplerCache());
            }
            Field fallback = rs.getDeclaredField("fallbackPipelineCache");
            fallback.setAccessible(true);
            if (fallback.get(null) == null) {
                fallback.set(null, new StubPipelineCache());
            }
        } catch (Throwable t) {
            GuiBridge.log("gpu stub: " + t);
        }
    }

    private static Object stub(String interfaceName) {
        try {
            ClassLoader loader = GpuStub.class.getClassLoader();
            Class<?> itf = Class.forName(interfaceName, false, loader);
            return Proxy.newProxyInstance(loader, new Class<?>[] { itf }, HANDLER);
        } catch (Throwable t) {
            return null;
        }
    }

    /** Pipeline cache that hands out inert compiled pipelines. */
    public static final class StubPipelineCache extends com.mojang.blaze3d.pipeline.PipelineCache {
        public StubPipelineCache() {
            super(
                    (com.mojang.renderpearl.api.device.GpuDevice)
                            stub("com.mojang.renderpearl.api.device.GpuDevice"),
                    (com.mojang.renderpearl.api.pipeline.ShaderSource)
                            stub("com.mojang.renderpearl.api.pipeline.ShaderSource"));
        }

        @Override
        public com.mojang.renderpearl.api.pipeline.CompiledRenderPipeline get(
                com.mojang.renderpearl.api.pipeline.RenderPipeline pipeline) {
            return (com.mojang.renderpearl.api.pipeline.CompiledRenderPipeline)
                    stub("com.mojang.renderpearl.api.pipeline.CompiledRenderPipeline");
        }
    }

    /** Sampler cache that hands out inert samplers instead of touching the device. */
    public static final class StubSamplerCache extends com.mojang.blaze3d.systems.SamplerCache {
        private volatile Object sampler;

        private Object sampler() {
            if (sampler == null) {
                sampler = GpuStub.stub("com.mojang.renderpearl.api.textures.GpuSampler");
            }
            return sampler;
        }

        @Override
        public com.mojang.renderpearl.api.textures.GpuSampler getSampler(
                com.mojang.renderpearl.api.textures.AddressMode u,
                com.mojang.renderpearl.api.textures.AddressMode v,
                com.mojang.renderpearl.api.textures.FilterMode min,
                com.mojang.renderpearl.api.textures.FilterMode mag,
                boolean anisotropy) {
            return (com.mojang.renderpearl.api.textures.GpuSampler) sampler();
        }

        @Override
        public com.mojang.renderpearl.api.textures.GpuSampler getClampToEdge(
                com.mojang.renderpearl.api.textures.FilterMode filter) {
            return (com.mojang.renderpearl.api.textures.GpuSampler) sampler();
        }

        @Override
        public com.mojang.renderpearl.api.textures.GpuSampler getClampToEdge(
                com.mojang.renderpearl.api.textures.FilterMode filter, boolean anisotropy) {
            return (com.mojang.renderpearl.api.textures.GpuSampler) sampler();
        }

        @Override
        public com.mojang.renderpearl.api.textures.GpuSampler getRepeat(
                com.mojang.renderpearl.api.textures.FilterMode filter) {
            return (com.mojang.renderpearl.api.textures.GpuSampler) sampler();
        }

        @Override
        public com.mojang.renderpearl.api.textures.GpuSampler getRepeat(
                com.mojang.renderpearl.api.textures.FilterMode filter, boolean anisotropy) {
            return (com.mojang.renderpearl.api.textures.GpuSampler) sampler();
        }
    }
}
