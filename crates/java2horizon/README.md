# Java2Horizon (J2H)

**Experimental** Java/Fabric compatibility runtime for
[Horizon](https://playhorizon.dev/), Minecraft Java 26.3 rewritten in Rust.

It embeds the JVM, initializes Fabric Loader's singleton and API modules,
places Minecraft 26.3 classes on the classpath, and bridges supported
registrations and GUI extraction to Horizon. Each Fabric API, Minecraft, and
rendering surface still requires its own runtime verification; this is not a
complete Fabric environment.

The game integration and user-facing notes are in
[`docs/java-mods.md`](../../docs/java-mods.md).

## What works

| Capability | Status |
|------------|--------|
| Embed OpenJDK via JNI | Yes |
| Load classes from JARs | Yes |
| `fabric.mod.json` + `main`/`client`/`server` entrypoints | Yes (including Fabric Loader singleton bootstrap) |
| Static and no-arg instance entrypoints | Yes |
| Multi-mod discovery, `depends`/`breaks`/`conflicts`, `environment`, order | Yes |
| Nested jars under `META-INF/jars/` | Extracted and put on the classpath |
| Official Fabric Loader API + ASM/Mixin runtime on the classpath | Yes, fetched and SHA-256 checked |
| Fabric Minecraft built-in metadata and provider | Yes, seeded from Horizon's pinned version and resolved client JAR |
| Fabric Mixin service and config registration | Yes, initialized before Minecraft client classes |
| Mixin bytecode application | Yes, through JVM instrumentation; synthetic injection regression passes |
| Access widener v1/v2, official namespace | Yes, transformed before Minecraft classes load |
| Class tweaker v1, official namespace | Access/finality changes and interface injection |
| Java -> Rust natives (log, blocks, items, entity types, effects, tabs, menus) | Yes |
| Emit Horizon content-mod trees and validate with `horizon-mods` | Yes |
| Remapping named/intermediary Mixin targets or refmaps | **No** |
| Class-tweaker enum extension, named/intermediary remapping, generic interface signatures | **No** |
| Runtime block registration or behaviour hooks | **No** |

## In the game

Jars in the game folder's `mods/` are discovered at startup. J2H runs once,
writes its content roots to `.horizon-cache/java2horizon/`, and
`crates/horizon-client/src/game/mods.rs` appends those roots to the normal
content pipeline. A new or changed jar needs a restart.

- `HORIZON_NO_JAVA_MODS=1` disables the runtime.
- `HORIZON_JAVA_MODS_DIR=/path` reads another folder (tests).
- `HORIZON_FABRIC_LIBS_DIR=/path` supplies the Fabric Loader runtime jars
  offline instead of downloading them.
- A jar renamed `*.jar.disabled` is skipped.

Java mods run arbitrary code in the game process; install only trusted jars.

## Initialization transactions

Each mod's synchronous entrypoints stage their `NativeBridge` callbacks in an
owned transaction. J2H validates the complete batch against a copy of committed
Horizon state, then commits it atomically. A failed entrypoint or invalid batch
is rolled back; required dependents are skipped and independent or optional
dependents continue. `Lifecycle::mod_initializations` records each result and
its diagnostics.

This rollback covers J2H-owned callbacks and generated Horizon content. It
cannot undo arbitrary Java static state, Fabric Loader's already-populated mod
list, Fabric event listeners, key mappings, or other Java-side effects. Those
limits are reported with failed initialization records. A NativeBridge call
made outside the synchronous owning transaction throws in Java instead of
being assigned to whichever mod happens to initialize next.

## Requirements

- Rust 1.85+ (the workspace toolchain).
- A Java 25+ runtime through `JAVA_HOME` or `java` on `PATH` for Minecraft 26.3.

Mixin configs are initialized through Fabric Loader's Mixin bootstrap. J2H
attaches the pinned Mixin transformer to JVM class definition, after access
widening and before the client singleton loads. This is not Knot: mappings,
custom launch plugins, and broad real-mod coverage are not established merely
by the synthetic transformer regression.

## Test

```sh
cargo test -p java2horizon
cargo run -p java2horizon        # runs libmod + testmod, validates the output
cargo test -p java2horizon --test ui_debug_harness -- --nocapture
J2H_REBUILD_RUNTIME=1 cargo test -p java2horizon --test ui_debug_harness -- --nocapture  # rebuild embedded GUI bridge classes
cargo test -p java2horizon --test access_widener_agent -- --nocapture
crates/java2horizon/build_runtime.sh  # rebuild embedded Fabric/Mixin bootstrap classes
build_access_widener_agent.sh  # regenerate the embedded JVM access-widener agent
```

The runner prints `J2H INTEGRATION PASSED` and writes
`test_resources/horizon_content/` (ignored by git).

`build_test_mods.sh` rebuilds the two test jars from `test_mod/`.

## Layout

```
src/
  jvm/           one process-wide JVM, classpath, entrypoint calls
  bridge/        JNI natives (Java -> Rust)
  fabric/        metadata, dependencies, entrypoints, nested jars,
                 official loader/ASM/Mixin libraries
  horizon_shim/  registration collection, content emit, horizon-mods validation
  lifecycle/     staged Fabric-style -> Horizon content pipeline
  main.rs        integration runner
bridge-java/     canonical dev.horizon.j2h.NativeBridge source for mod jars
                 and the access widener instrumentation agent
test_mod/        Java sources for libmod + testmod
test_resources/  JARs + *.mod.json metadata
ARCHITECTURE_NOTES.md
COMPATIBILITY_GAP_REPORT.md
```

## Limits and honesty

- Not affiliated with Mojang, Microsoft, Fabric or the Horizon project.
- You still need a legal Minecraft Java Edition install to use Horizon as a game.
- See `COMPATIBILITY_GAP_REPORT.md` for the remaining blockers.
