package dev.horizon.j2h;

import java.io.IOException;
import java.io.InputStream;
import java.io.PrintWriter;
import java.io.StringWriter;
import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.IllegalClassFormatException;
import java.lang.instrument.Instrumentation;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.security.ProtectionDomain;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.Opcodes;

/** Applies Fabric access wideners to Minecraft classes before class definition. */
public final class AccessWidenerAgent {
    private static volatile Map<String, List<Rule>> rules = Map.of();
    private static volatile Instrumentation instrumentation;
    private static volatile boolean mixinTransformerInstalled;
    private static final Set<String> generatedMixinClasses = ConcurrentHashMap.newKeySet();
    private static final Map<String, String> generatedMixinClassOwners = new ConcurrentHashMap<>();
    private static volatile String mixinTransformFailureTarget;
    private static volatile Throwable mixinTransformFailureCause;
    private static volatile Method defineClass;
    private static final Set<String> rejectedMods = new HashSet<>();
    private static final Map<String, String> modErrors = new HashMap<>();

    private AccessWidenerAgent() {
    }

    /** Agent arguments are an access-widener directory and the pinned client jar, separated by '|'. */
    public static void premain(String arguments, Instrumentation instrumentation) throws Exception {
        String[] parts = arguments.split("\\|", 2);
        if (parts.length != 2) {
            throw new IllegalArgumentException("expected access-widener directory|Minecraft client jar");
        }
        Map<String, List<Rule>> loaded = loadRules(Paths.get(parts[0]));
        validateRules(loaded, Paths.get(parts[1]));
        rules = loaded;
        AccessWidenerAgent.instrumentation = instrumentation;
        openClassLoaderDefine(instrumentation);
        instrumentation.addTransformer(new Transformer(), false);
        System.err.println("J2H: JVM instrumentation agent ready for " + loaded.size() + " access widener(s)");
    }

    /** Attach Fabric's configured Mixin transformer after loader bootstrap. */
    public static synchronized void installMixinTransformer() throws Exception {
        if (mixinTransformerInstalled) return;
        Instrumentation inst = instrumentation;
        if (inst == null) throw new IllegalStateException("J2H instrumentation agent is not active");

        Class<?> knotService = Class.forName("net.fabricmc.loader.impl.launch.knot.MixinServiceKnot");
        var getter = knotService.getDeclaredMethod("getTransformer");
        getter.setAccessible(true);
        Object transformer = getter.invoke(null);
        var transform = transformer.getClass().getMethod(
                "transformClassBytes", String.class, String.class, byte[].class);
        transform.setAccessible(true);
        inst.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(
                    Module module,
                    ClassLoader loader,
                    String internalName,
                    Class<?> classBeingRedefined,
                    ProtectionDomain domain,
                    byte[] classfileBuffer) throws IllegalClassFormatException {
                if (internalName == null) return null;
                String name = internalName.replace('/', '.');
                if (generatedMixinClasses.contains(name)) return null;
                try {
                    synchronized (transformer) {
                        if (generatedMixinClasses.contains(name)) return null;
                        byte[] transformed = (byte[]) transform.invoke(
                                transformer, name, name, classfileBuffer);
                        if (transformed != classfileBuffer) {
                            defineGeneratedNestMembers(transformer, loader, domain, transformed);
                            System.err.println("J2H: Mixin transformed " + name);
                        }
                        return transformed == classfileBuffer ? null : transformed;
                    }
                } catch (Throwable failure) {
                    Throwable cause = unwrap(failure);
                    synchronized (AccessWidenerAgent.class) {
                        if (mixinTransformFailureCause == null) {
                            mixinTransformFailureTarget = name;
                            mixinTransformFailureCause = cause;
                        }
                    }
                    System.err.println("J2H: Mixin transformation failed for " + name + ": " + cause);
                    cause.printStackTrace(System.err);
                    // ClassFileTransformer exceptions are treated like a null
                    // return by the JVM, which would define the original host
                    // bytes. A non-empty invalid classfile forces definition to
                    // fail instead. Empty arrays are also treated as no change
                    // by the pinned JVM, so keep this deliberately non-empty.
                    return new byte[] { 0 };
                }
            }
        }, false);
        mixinTransformerInstalled = true;
        System.err.println("J2H: Fabric Mixin transformer installed");
    }

    /**
     * Define Mixin-generated nest members in the host's loader before the host is
     * defined. Reuse the host ProtectionDomain so generated classes satisfy the
     * signed client's package signer checks. This is a J2H-specific adaptation;
     * Knot's missing-resource path uses a null CodeSource and is not identical.
     */
    private static void defineGeneratedNestMembers(
            Object transformer, ClassLoader loader, ProtectionDomain domain, byte[] hostBytes) throws Exception {
        if (hostBytes == null || loader == null) return;
        String hostInternalName = new ClassReader(hostBytes).getClassName();
        Set<String> nestMembers = new HashSet<>();
        new ClassReader(hostBytes).accept(new ClassVisitor(Opcodes.ASM9) {
            @Override
            public void visitNestMember(String nestMember) {
                nestMembers.add(nestMember.replace('/', '.'));
            }
        }, ClassReader.SKIP_CODE | ClassReader.SKIP_DEBUG | ClassReader.SKIP_FRAMES);

        Object extensions = accessibleMethod(transformer.getClass(), "getExtensions").invoke(transformer);
        Object registry = accessibleMethod(extensions.getClass(), "getSyntheticClassRegistry").invoke(extensions);
        Method findSynthetic = accessibleMethod(registry.getClass(), "findSyntheticClass", String.class);
        Method transformMissing = accessibleMethod(
                transformer.getClass(), "transformClassBytes", String.class, String.class, byte[].class);

        // Mixin 0.17.5 registers generated inner classes before target transformation,
        // but a nested target's transformed NestMembers need not list that generated
        // class. Seed candidates from the pinned registry while restricting them to
        // this target so a different host's generated class never inherits this host's
        // ProtectionDomain.
        nestMembers.addAll(registryCandidates(registry, hostInternalName, findSynthetic));

        ArrayDeque<String> pending = new ArrayDeque<>(nestMembers);
        Set<String> visited = new HashSet<>();
        Map<String, byte[]> generated = new HashMap<>();
        while (!pending.isEmpty()) {
            String name = pending.removeFirst();
            if (!visited.add(name) || generatedMixinClasses.contains(name)) continue;
            Object synthetic = findSynthetic.invoke(registry, name);
            if (synthetic == null || !syntheticBelongsToTarget(synthetic, hostInternalName)) continue;

            byte[] bytes = (byte[]) transformMissing.invoke(transformer, name, name, null);
            if (bytes == null) {
                throw new IllegalStateException("Mixin registered synthetic class but generated no bytes: " + name);
            }
            String actualName = new ClassReader(bytes).getClassName().replace('/', '.');
            if (!name.equals(actualName)) {
                throw new IllegalStateException(
                        "Mixin generated class name mismatch: requested " + name + ", received " + actualName);
            }
            generated.put(name, bytes);

            Set<String> generatedNestMembers = new HashSet<>();
            new ClassReader(bytes).accept(new ClassVisitor(Opcodes.ASM9) {
                @Override
                public void visitNestMember(String nestMember) {
                    generatedNestMembers.add(nestMember.replace('/', '.'));
                }
            }, ClassReader.SKIP_CODE | ClassReader.SKIP_DEBUG | ClassReader.SKIP_FRAMES);
            pending.addAll(generatedNestMembers);
        }
        if (generated.isEmpty()) return;

        Method define = defineClass;
        if (define == null) throw new IllegalStateException("ClassLoader.defineClass access is unavailable");
        int defined = 0;
        for (Map.Entry<String, byte[]> entry : generated.entrySet()) {
            String name = entry.getKey();
            if (!generatedMixinClasses.add(name)) continue;
            byte[] bytes = entry.getValue();
            try {
                define.invoke(loader, name, bytes, 0, bytes.length, domain);
                generatedMixinClassOwners.put(name, hostInternalName);
                defined++;
            } catch (Throwable failure) {
                generatedMixinClasses.remove(name);
                throw failure;
            }
        }
        System.err.println("J2H: defined " + defined + " Mixin-generated class(es)");
    }

    private static Set<String> registryCandidates(Object registry, String hostInternalName, Method findSynthetic) {
        try {
            var classesField = registry.getClass().getDeclaredField("classes");
            classesField.setAccessible(true);
            Object registeredClasses = classesField.get(registry);
            if (!(registeredClasses instanceof Map<?, ?> classes)) {
                throw new IllegalStateException("Mixin synthetic registry classes field is not a Map");
            }
            String generatedPrefix = hostInternalName + "$";
            Set<String> candidates = new HashSet<>();
            for (Object key : classes.keySet()) {
                if (key instanceof String name && name.startsWith(generatedPrefix)) {
                    Object synthetic = findSynthetic.invoke(registry, name);
                    if (synthetic != null && syntheticBelongsToTarget(synthetic, hostInternalName)) {
                        candidates.add(name.replace('/', '.'));
                    }
                }
            }
            return candidates;
        } catch (ReflectiveOperationException | RuntimeException failure) {
            throw new IllegalStateException("Cannot enumerate Mixin synthetic classes for target "
                    + hostInternalName + "; refusing NestMembers-only fallback", failure);
        }
    }

    /** Diagnostic accessor used by runtime verification to confirm synthetic ownership. */
    public static String generatedMixinClassesForTarget(String targetInternalName) {
        return generatedMixinClassOwners.entrySet().stream()
                .filter(entry -> entry.getValue().equals(targetInternalName))
                .map(Map.Entry::getKey)
                .sorted()
                .collect(java.util.stream.Collectors.joining(","));
    }

    private static boolean syntheticBelongsToTarget(Object synthetic, String hostInternalName)
            throws ReflectiveOperationException {
        // InnerClassInfo exposes its exact target through a pinned package-private
        // method. Use it when available; the public mixin target list is a fallback
        // for other synthetic generators, where only a single-target match is safe.
        try {
            Method targetName = synthetic.getClass().getDeclaredMethod("getTargetName");
            targetName.setAccessible(true);
            Object value = targetName.invoke(synthetic);
            return hostInternalName.equals(value);
        } catch (NoSuchMethodException ignored) {
            Object mixin = accessibleMethod(synthetic.getClass(), "getMixin").invoke(synthetic);
            if (mixin == null) return false;
            Object targets = accessibleMethod(mixin.getClass(), "getTargetClasses").invoke(mixin);
            if (!(targets instanceof List<?> targetList) || targetList.size() != 1) return false;
            return hostInternalName.equals(String.valueOf(targetList.get(0)).replace('.', '/'));
        }
    }

    private static void openClassLoaderDefine(Instrumentation instrumentation) throws Exception {
        Module javaBase = ClassLoader.class.getModule();
        Module agentModule = AccessWidenerAgent.class.getModule();
        instrumentation.redefineModule(
                javaBase,
                Set.of(),
                Map.of(),
                Map.of("java.lang", Set.of(agentModule)),
                Set.of(),
                Map.of());
        Method method = ClassLoader.class.getDeclaredMethod(
                "defineClass", String.class, byte[].class, int.class, int.class, ProtectionDomain.class);
        method.setAccessible(true);
        defineClass = method;
    }

    private static Throwable unwrap(Throwable failure) {
        Throwable cause = failure;
        while ((cause instanceof InvocationTargetException || cause instanceof java.util.concurrent.ExecutionException)
                && cause.getCause() != null) {
            cause = cause.getCause();
        }
        return cause;
    }

    private static Method accessibleMethod(Class<?> owner, String name, Class<?>... parameterTypes)
            throws ReflectiveOperationException {
        Method method = owner.getMethod(name, parameterTypes);
        method.setAccessible(true);
        return method;
    }

    /** Return the first fatal transformer failure with its complete Java cause chain. */
    public static synchronized String mixinTransformFailure() {
        Throwable failure = mixinTransformFailureCause;
        if (failure == null) return "";
        StringWriter trace = new StringWriter();
        failure.printStackTrace(new PrintWriter(trace));
        return "Mixin transformation failed for " + mixinTransformFailureTarget + ":\n" + trace;
    }

    /** Return an empty string when the mod has no rejected access widener. */
    public static synchronized String modFailure(String modId) {
        return modErrors.getOrDefault(modId, "");
    }

    public static synchronized boolean modRejected(String modId) {
        return rejectedMods.contains(modId);
    }

    private static Map<String, List<Rule>> loadRules(Path directory) throws IOException {
        Map<String, List<Rule>> loaded = new HashMap<>();
        try (var paths = Files.list(directory)) {
            for (Path file : paths.filter(path -> path.getFileName().toString().endsWith(".aw")).sorted().toList()) {
                String filename = file.getFileName().toString();
                String modId = filename.substring(0, filename.length() - 3);
                try {
                    loaded.put(modId, parse(modId, Files.readString(file, StandardCharsets.UTF_8)));
                } catch (RuntimeException failure) {
                    reject(modId, "invalid access widener: " + failure.getMessage());
                }
            }
        }
        return loaded;
    }

    private static List<Rule> parse(String modId, String source) {
        List<Rule> parsed = new ArrayList<>();
        String[] lines = source.split("\\R");
        String[] header = null;
        for (int i = 0; i < lines.length; i++) {
            String line = lines[i].split("#", 2)[0].trim();
            if (line.isEmpty()) continue;
            String[] fields = line.split("\\s+");
            if (header == null) {
                header = fields;
                boolean legacy = fields.length == 3 && fields[0].equals("accessWidener")
                        && (fields[1].equals("v1") || fields[1].equals("v2"));
                boolean tweaker = fields.length == 3 && fields[0].equals("classTweaker")
                        && fields[1].equals("v1");
                if ((!legacy && !tweaker) || !fields[2].equals("official")) {
                    throw new IllegalArgumentException("line 1: only v1/v2 official access wideners and v1 official class tweakers are supported");
                }
                continue;
            }
            int offset = fields[0].startsWith("transitive-") ? "transitive-".length() : 0;
            String directive = fields[0].substring(offset);
            String kind = fields.length > 1 ? fields[1] : "";
            if (directive.equals("inject-interface") && fields.length == 3
                    && header[0].equals("classTweaker")) {
                parsed.add(new Rule(modId, directive, "interface", fields[1], fields[2], ""));
            } else
            if (kind.equals("class") && fields.length == 3
                    && (directive.equals("accessible") || directive.equals("extendable"))) {
                parsed.add(new Rule(modId, directive, kind, fields[2], "", ""));
            } else if (kind.equals("method") && fields.length == 5
                    && (directive.equals("accessible") || directive.equals("extendable"))) {
                parsed.add(new Rule(modId, directive, kind, fields[2], fields[3], fields[4]));
            } else if (kind.equals("field") && fields.length == 5
                    && (directive.equals("accessible") || directive.equals("mutable"))) {
                parsed.add(new Rule(modId, directive, kind, fields[2], fields[3], fields[4]));
            } else {
                throw new IllegalArgumentException("line " + (i + 1) + ": unsupported or malformed entry");
            }
        }
        if (header == null) throw new IllegalArgumentException("missing header");
        return parsed;
    }

    private static void validateRules(Map<String, List<Rule>> allRules, Path clientJar) throws IOException {
        Map<String, byte[]> classes = new HashMap<>();
        try (JarFile jar = new JarFile(clientJar.toFile())) {
            for (List<Rule> modRules : allRules.values()) {
                for (Rule rule : modRules) {
                    if (classes.containsKey(rule.owner)) continue;
                    JarEntry entry = jar.getJarEntry(rule.owner + ".class");
                    if (entry == null) continue;
                    try (InputStream in = jar.getInputStream(entry)) {
                        classes.put(rule.owner, in.readAllBytes());
                    }
                }
            }
        }
        for (Map.Entry<String, List<Rule>> mod : allRules.entrySet()) {
            String error = null;
            for (Rule rule : mod.getValue()) {
                byte[] bytes = classes.get(rule.owner);
                if (bytes == null) {
                    error = "target class not found in pinned client jar: " + rule.owner;
                    break;
                }
                try {
                    ClassFile model = new ClassFile(bytes);
                    if (!model.contains(rule)) {
                        error = "target " + rule.kind + " not found in pinned client jar: " + rule.owner + "." + rule.name + rule.descriptor;
                        break;
                    }
                } catch (IOException malformed) {
                    error = "cannot inspect target class " + rule.owner + ": " + malformed.getMessage();
                    break;
                }
            }
            if (error != null) reject(mod.getKey(), error);
        }
    }

    private static synchronized void reject(String modId, String reason) {
        rejectedMods.add(modId);
        modErrors.put(modId, reason);
        System.err.println("J2H: mod '" + modId + "' access widener rejected: " + reason);
    }

    private static final class Transformer implements ClassFileTransformer {
        @Override
        public byte[] transform(Module module, ClassLoader loader, String className,
                Class<?> classBeingRedefined, ProtectionDomain protectionDomain, byte[] classfileBuffer)
                throws IllegalClassFormatException {
            List<Rule> targetRules = rules.values().stream().flatMap(List::stream)
                    .filter(rule -> rule.owner.equals(className) && !rejectedMods.contains(rule.modId)).toList();
            if (targetRules.isEmpty()) return null;
            try {
                ClassFile model = new ClassFile(classfileBuffer);
                byte[] transformed = model.apply(targetRules);
                if (transformed != null) {
                    Set<String> sources = new HashSet<>();
                    for (Rule rule : targetRules) sources.add(rule.modId);
                    System.err.println("J2H: access widener transformed " + className + " from " + sources);
                }
                return transformed;
            } catch (IOException failure) {
                throw new IllegalClassFormatException("J2H access widener could not transform " + className + ": " + failure);
            }
        }
    }

    private static final class Rule {
        final String modId, directive, kind, owner, name, descriptor;
        Rule(String modId, String directive, String kind, String owner, String name, String descriptor) {
            this.modId = modId; this.directive = directive; this.kind = kind;
            this.owner = owner; this.name = name; this.descriptor = descriptor;
        }
    }

    /** Small class-file reader; access widener changes only two-byte access flags. */
    private static final class ClassFile {
        private final byte[] bytes;
        private final String[] utf8;
        private final int[] classNameIndexes;
        private final int constantPoolCount;
        private final int constantPoolEnd;
        private final int classFlagsOffset;
        private final int interfacesOffset;
        private final int interfaceCount;
        private final int fieldsOffset;
        private final int methodsOffset;
        private final int fieldCount;
        private final int methodCount;

        ClassFile(byte[] bytes) throws IOException {
            this.bytes = bytes.clone();
            Cursor cursor = new Cursor(this.bytes);
            if (cursor.u4() != 0xCAFEBABEL) throw new IOException("bad class magic");
            cursor.skip(4);
            int count = cursor.u2();
            constantPoolCount = count;
            utf8 = new String[count];
            classNameIndexes = new int[count];
            for (int i = 1; i < count; i++) {
                int tag = cursor.u1();
                switch (tag) {
                    case 1 -> utf8[i] = new String(cursor.bytes(cursor.u2()), StandardCharsets.UTF_8);
                    case 3, 4 -> cursor.skip(4);
                    case 5, 6 -> { cursor.skip(8); i++; }
                    case 7 -> classNameIndexes[i] = cursor.u2();
                    case 8, 16, 19, 20 -> cursor.skip(2);
                    case 9, 10, 11, 12, 17, 18 -> cursor.skip(4);
                    case 15 -> cursor.skip(3);
                    default -> throw new IOException("unknown constant pool tag " + tag);
                }
            }
            constantPoolEnd = cursor.position();
            classFlagsOffset = cursor.position();
            cursor.skip(6);
            interfaceCount = cursor.u2();
            interfacesOffset = cursor.position();
            cursor.skip(2 * interfaceCount);
            fieldCount = cursor.u2();
            fieldsOffset = cursor.position();
            for (int i = 0; i < fieldCount; i++) skipMember(cursor);
            methodCount = cursor.u2();
            methodsOffset = cursor.position();
        }

        boolean contains(Rule rule) throws IOException {
            if (rule.kind.equals("class") || rule.kind.equals("interface")) return true;
            Cursor cursor = new Cursor(bytes);
            cursor.position(rule.kind.equals("field") ? fieldsOffset : methodsOffset);
            int count = rule.kind.equals("field") ? fieldCount : methodCount;
            for (int i = 0; i < count; i++) {
                int access = cursor.u2();
                String name = cpUtf8(cursor.u2());
                String desc = cpUtf8(cursor.u2());
                if (name.equals(rule.name) && desc.equals(rule.descriptor)) return true;
                skipAttributes(cursor);
            }
            return false;
        }

        byte[] apply(List<Rule> targetRules) throws IOException {
            boolean changed = false;
            Set<String> implementedInterfaces = new HashSet<>();
            Cursor interfaceCursor = new Cursor(bytes);
            interfaceCursor.position(interfacesOffset);
            for (int i = 0; i < interfaceCount; i++) {
                int classIndex = interfaceCursor.u2();
                int nameIndex = classIndex > 0 && classIndex < classNameIndexes.length
                        ? classNameIndexes[classIndex] : 0;
                implementedInterfaces.add(cpUtf8(nameIndex));
            }
            Set<String> newInterfaces = new java.util.LinkedHashSet<>();
            for (Rule rule : targetRules) {
                if (rule.kind.equals("interface")) {
                    String name = rule.name;
                    int genericStart = name.indexOf('<');
                    if (genericStart >= 0) name = name.substring(0, genericStart);
                    if (!implementedInterfaces.contains(name)) newInterfaces.add(name);
                }
            }
            Cursor cursor = new Cursor(bytes);
            int classAccess = readU2(classFlagsOffset);
            int newClassAccess = classAccess;
            for (Rule rule : targetRules) {
                if (rule.kind.equals("class")) newClassAccess = widen(newClassAccess, rule.directive, "class");
                else if (rule.kind.equals("method") && rule.directive.equals("extendable"))
                    newClassAccess = widen(newClassAccess, "extendable", "class");
                else if ((rule.kind.equals("method") || rule.kind.equals("field"))
                        && rule.directive.equals("accessible"))
                    newClassAccess = widen(newClassAccess, "accessible", "class");
            }
            if (newClassAccess != classAccess) { writeU2(classFlagsOffset, newClassAccess); changed = true; }
            cursor.position(fieldsOffset);
            for (int i = 0; i < fieldCount; i++) {
                int flagsOffset = cursor.position();
                int flags = cursor.u2();
                String name = cpUtf8(cursor.u2());
                String desc = cpUtf8(cursor.u2());
                int next = skipAttributesAndGetPosition(cursor);
                int result = flags;
                for (Rule rule : targetRules) if (rule.kind.equals("field") && rule.name.equals(name) && rule.descriptor.equals(desc))
                    result = widen(result, rule.directive, "field");
                if (result != flags) { writeU2(flagsOffset, result); changed = true; }
                cursor.position(next);
            }
            cursor.position(methodsOffset);
            for (int i = 0; i < methodCount; i++) {
                int flagsOffset = cursor.position();
                int flags = cursor.u2();
                String name = cpUtf8(cursor.u2());
                String desc = cpUtf8(cursor.u2());
                int next = skipAttributesAndGetPosition(cursor);
                int result = flags;
                for (Rule rule : targetRules) if (rule.kind.equals("method") && rule.name.equals(name) && rule.descriptor.equals(desc)) {
                    result = widen(result, rule.directive, "method");
                    if (rule.directive.equals("accessible") && (name.equals("<init>") || name.equals("<clinit>")))
                        result &= ~0x0010;
                }
                if (result != flags) { writeU2(flagsOffset, result); changed = true; }
                cursor.position(next);
            }
            if (!newInterfaces.isEmpty()) return addInterfaces(newInterfaces);
            return changed ? bytes : null;
        }

        private byte[] addInterfaces(Set<String> names) throws IOException {
            Map<String, Integer> utf8Indexes = new HashMap<>();
            Map<String, Integer> classIndexes = new HashMap<>();
            for (int i = 1; i < utf8.length; i++) if (utf8[i] != null) utf8Indexes.putIfAbsent(utf8[i], i);
            for (int i = 1; i < classNameIndexes.length; i++) {
                if (classNameIndexes[i] > 0) classIndexes.putIfAbsent(cpUtf8(classNameIndexes[i]), i);
            }
            java.io.ByteArrayOutputStream additions = new java.io.ByteArrayOutputStream();
            java.io.DataOutputStream cp = new java.io.DataOutputStream(additions);
            List<Integer> newClassIndexes = new ArrayList<>();
            int nextIndex = constantPoolCount;
            for (String name : names) {
                Integer nameIndex = utf8Indexes.get(name);
                if (nameIndex == null) {
                    byte[] encoded = name.getBytes(StandardCharsets.UTF_8);
                    if (encoded.length > 65535) throw new IOException("interface name is too long");
                    nameIndex = nextIndex++;
                    cp.writeByte(1);
                    cp.writeShort(encoded.length);
                    cp.write(encoded);
                }
                Integer classIndex = classIndexes.get(name);
                if (classIndex == null) {
                    classIndex = nextIndex++;
                    cp.writeByte(7);
                    cp.writeShort(nameIndex);
                }
                newClassIndexes.add(classIndex);
            }
            if (nextIndex > 65535 || interfaceCount + newClassIndexes.size() > 65535) {
                throw new IOException("class file constant/interface table is full");
            }

            java.io.ByteArrayOutputStream result = new java.io.ByteArrayOutputStream(bytes.length + additions.size() + newClassIndexes.size() * 2);
            java.io.DataOutputStream out = new java.io.DataOutputStream(result);
            out.write(bytes, 0, 8);
            out.writeShort(nextIndex);
            out.write(bytes, 10, constantPoolEnd - 10);
            additions.writeTo(out);
            byte[] suffix = java.util.Arrays.copyOfRange(bytes, constantPoolEnd, bytes.length);
            int interfaceListEnd = 8 + interfaceCount * 2;
            out.write(suffix, 0, 6);
            out.writeShort(interfaceCount + newClassIndexes.size());
            out.write(suffix, 8, interfaceCount * 2);
            for (int index : newClassIndexes) out.writeShort(index);
            out.write(suffix, interfaceListEnd, suffix.length - interfaceListEnd);
            return result.toByteArray();
        }

        private int widen(int flags, String directive, String kind) {
            if (kind.equals("class")) {
                if (directive.equals("accessible") || directive.equals("extendable")) flags = (flags & ~0x0006) | 0x0001;
                if (directive.equals("extendable")) flags &= ~0x0010;
            } else if (kind.equals("method")) {
                if (directive.equals("accessible")) {
                    if ((flags & 0x0002) != 0 && (flags & 0x0008) == 0) flags |= 0x0010;
                    flags = (flags & ~0x0006) | 0x0001;
                }
                if (directive.equals("extendable")) {
                    flags &= ~0x0010;
                    if ((flags & 0x0002) != 0) flags = (flags & ~0x0002) | 0x0004;
                }
            } else if (kind.equals("field")) {
                if (directive.equals("accessible")) flags = (flags & ~0x0006) | 0x0001;
                if (directive.equals("mutable")) flags &= ~0x0010;
            }
            return flags;
        }

        private int readU2(int offset) { return ((bytes[offset] & 0xff) << 8) | (bytes[offset + 1] & 0xff); }
        private void writeU2(int offset, int value) { bytes[offset] = (byte) (value >>> 8); bytes[offset + 1] = (byte) value; }
        private String cpUtf8(int index) throws IOException {
            if (index <= 0 || index >= utf8.length || utf8[index] == null) throw new IOException("invalid UTF-8 constant index " + index);
            return utf8[index];
        }
        private static void skipMember(Cursor cursor) throws IOException { cursor.skip(6); skipAttributes(cursor); }
        private static void skipAttributes(Cursor cursor) throws IOException {
            int count = cursor.u2();
            for (int i = 0; i < count; i++) { cursor.skip(2); cursor.skipLength(cursor.u4()); }
        }
        private static int skipAttributesAndGetPosition(Cursor cursor) throws IOException { skipAttributes(cursor); return cursor.position(); }
    }

    private static final class Cursor {
        private final byte[] data;
        private int position;
        Cursor(byte[] data) { this.data = data; }
        int position() { return position; }
        void position(int value) throws IOException { if (value < 0 || value > data.length) throw new IOException("invalid class-file offset"); position = value; }
        int u1() throws IOException { require(1); return data[position++] & 0xff; }
        int u2() throws IOException { return (u1() << 8) | u1(); }
        long u4() throws IOException { return ((long) u2() << 16) | u2(); }
        byte[] bytes(int length) throws IOException { require(length); byte[] out = java.util.Arrays.copyOfRange(data, position, position + length); position += length; return out; }
        void skip(long length) throws IOException { if (length < 0 || length > Integer.MAX_VALUE) throw new IOException("invalid attribute length"); require((int) length); position += (int) length; }
        void skipLength(long length) throws IOException { skip(length); }
        private void require(int length) throws IOException { if (position + length > data.length) throw new IOException("truncated class file"); }
    }
}
