# Java mods (Java2Horizon, experimental)

Java2Horizon (J2H) is an experimental compatibility runtime for Minecraft
26.3 Fabric mods. It starts an embedded JVM, initializes Fabric Loader's
singleton and API modules, exposes real Minecraft client classes, and bridges
supported registrations and GUI extraction into Horizon. It is not a complete
Fabric implementation; each API surface needs independent runtime verification.

The game starts J2H before it builds its resource root. J2H reads
`fabric.mod.json`, checks dependencies, starts one embedded JVM, runs each
mod's `main` entrypoint, and turns the registrations its code makes through
J2H's native bridge into ordinary Horizon content-mod folders. Horizon then
loads those folders like any other content mod.

When a mod depends on `fabricloader` (or a `fabric-*` module), J2H provisions
Fabric Loader 0.20.1, its ASM 9.10.1 modules and sponge-mixin 0.17.5. It
initializes Fabric Loader's singleton and exposes Fabric API modules carried
by mod jars. Minecraft 26.3's client jar and runtime libraries are also placed
on the JVM classpath. This does not mean every Fabric API or Minecraft behavior
is implemented by Horizon.

J2H supplies the pinned Minecraft runtime to Fabric Loader as a built-in mod
through a small `GameProvider` and Loader's normal built-in candidate path. Its
metadata version is read from Horizon's pinned Minecraft version, and its
origin is the resolved client jar. The provider exposes runtime identity and
game-directory facts; it does not launch Minecraft, because Horizon owns the
game loop. Other Fabric built-ins and the full Knot launch lifecycle are not
implemented.

## What works

- `fabric.mod.json` (and a sidecar `<jar>.mod.json` for test jars), including
  `depends`, `breaks`, `conflicts`, `provides`, `environment`, `entrypoints`,
  and nested jars under `META-INF/jars/`. A client runs `main` + `client` and
  skips `environment = "server"` mods; a dedicated server does the reverse.
- Dependency presence and a stable load order (a mod's dependencies first).
- Static entrypoints and no-arg instance entrypoints (Fabric's
  `ModInitializer`/`ClientModInitializer` shape).
- A Java entrypoint calling J2H's `NativeBridge` to register:
  blocks, items, entity types, mob effects, creative tabs, and menu types.
  The jar must carry the `dev.horizon.j2h.NativeBridge` class; a canonical
  source copy is in `crates/java2horizon/bridge-java/`. J2H registers the
  natives on it at startup.
- Emission of `horizon-mod.toml` plus
  `data/<namespace>/horizon/{block,item,entity_type,mob_effect,creative_tab,menu}/`
  and `assets/<namespace>/lang/en_us.json`.
- Validation with the real `horizon-mods` `Manifest::parse` and
  `Content::read`, before the game reads the content.
- Access widener v1/v2 in the `official` namespace, and class tweaker v1 in
  the `official` namespace, applied before Minecraft classes are loaded. J2H
  validates each mod's targets against the pinned client jar and isolates an
  unsupported or malformed widener to that mod.
- Fabric's Mixin bootstrap and declared Mixin configurations, applied by the
  pinned Mixin transformer through JVM instrumentation before Minecraft client
  classes load. The transformer has a synthetic injection test; real-mod
  compatibility is tracked separately in the J2H scorecard.
- Before client entrypoints, the real `Minecraft` singleton receives Horizon's
  current player name and UUID. Offline UUIDs use Minecraft's
  `OfflinePlayer:<name>` rule; signed-in profiles use their actual UUID. This
  supports identity-only code such as Sodium's fingerprint setup, but does not
  establish Sodium renderer compatibility.

## Installing

Put Fabric jars in the game folder's `mods/` directory (the folder holding
`options.txt`), then restart the game. J2H discovers `.jar` files and folders
that contain `fabric.mod.json`. A jar renamed to `*.jar.disabled` is skipped.
It also skips hidden files and `fabric.mod.json`-less jars it cannot read.
Options > Mods lists the discovered jars with a `(Java)` mark; because they
run before the world opens, that row reports the startup state rather than
toggling live. Rename the jar to turn one off.

Requirements:

- A Java 25+ runtime reachable through `JAVA_HOME`, or a `java` executable on
  `PATH`. The JVM is loaded in-process.
- The Fabric `minecraft` built-in mod metadata is read from the selected client
  JAR's `version.json`. J2H checks that the JAR matches the configured game
  version and advertises the file's `java_version` as a required Java
  dependency, matching Fabric Loader's version-derived requirement.
- The generated content is cached under `.horizon-cache/java2horizon/` in the
  game folder. It is rebuilt when the game starts, so a new jar needs a
  restart.

Environment switches:

- `HORIZON_NO_JAVA_MODS=1` disables the runtime for this run.
- `HORIZON_JAVA_MODS_DIR=/path` reads jars from another folder, for tests.
- `HORIZON_FABRIC_LIBS_DIR=/path` reads the official Fabric Loader runtime
  jars from a folder instead of downloading them, for offline runs.

Java mods execute arbitrary code. Install only jars you trust, and note that
the JVM shares the game process.

## Initialization transactions

J2H collects each mod's synchronous `NativeBridge` registrations in a separate
transaction. It validates the full batch against the already committed Horizon
registries and commits it atomically. If an entrypoint throws or a registration
is invalid, J2H discards that mod's staged registrations, records the failure,
skips mods that require it, and continues with independent mods. Optional
dependents continue. This includes blocks, items, registry entries, and the
event/command callbacks represented by `NativeBridge`.

J2H cannot roll back arbitrary Java static state, the Fabric Loader mod list,
Fabric API event listeners, key mappings, or other Java-side registrations.
An open Java screen is closed after a failed client initialization, but its
listeners and key mappings may remain active. These are reported as
irreversible side effects in the failed mod's diagnostics. NativeBridge calls
made outside an active synchronous transaction throw an
`IllegalStateException`; asynchronous Java registrations are not staged.

## What does not work

J2H applies Fabric Mixin configurations, but does not remap named/intermediary
targets or refmaps to official Minecraft names; platform-specific Mixin launch
plugins and compatibility with arbitrary configs remain unverified. Class
tweaker support is limited to class/member access and
finality changes plus interface injection; enum extension is not implemented,
and generic interface signatures are not retained. Many Fabric API modules and
Minecraft behaviors remain incomplete. J2H does not implement Forge or
NeoForge. Java block and item registrations become static content-mod
declarations before the resource root is built; arbitrary in-world registry
mutation and Java-defined block behavior are not supported. Client rendering,
screens, networking, world generation, block entities and behaviour hooks are
not generally bridged.

Options > Mods lists each discovered jar as a `(Java)` row. The row reports
the startup state and its toggle is disabled in-world; rename the jar to
`*.jar.disabled` and restart to turn it off. A failure is logged and the game
continues without that mod's content.

## Testing the bridge

The crate carries two test jars and their sources:

```sh
cargo test -p java2horizon
cargo run -p java2horizon        # runs the two jars and validates the output
```

The runner prints `J2H INTEGRATION PASSED` and writes
`crates/java2horizon/test_resources/horizon_content/` (ignored by git).

## Layout

- `crates/java2horizon/src/jvm/`: one process-wide JVM, classpath, JNI calls.
- `crates/java2horizon/src/bridge/`: Java-to-Rust native callbacks.
- `crates/java2horizon/src/fabric/`: metadata, dependencies, entrypoints,
  nested jars.
- `crates/java2horizon/src/horizon_shim/`: registration collection, content
  emission, `horizon-mods` validation.
- `crates/java2horizon/src/lifecycle/`: the staged pipeline.
- `crates/horizon-client/src/game/java_mods.rs`: the game hook that runs J2H
  once and appends the generated roots to the content pipeline.

## Global testing preview

J2H is compiled into the Horizon client binary. The profile tool at `tools/j2h_patcher.py` verifies a client with `--j2h-info`, copies it into a new profile, and stages catalog-pinned JARs under that profile's `game/mods/`. It does not modify a normal Horizon install or alter executable bytes. The profile has separate saves, options, logs, caches, and launcher/XDG data. Existing profile paths are never overwritten.

Build Horizon and prepare Minecraft 26.3 resources first. Then create and verify a test profile:

```sh
cargo build --profile fast -p horizon-client
python3 tools/j2h_patcher.py install \
  --client target/fast/horizon-client \
  --resources .local/horizon-client/26.3 \
  --profile "$HOME/Horizon-J2H-26.3" \
  --mods modmenu roughlyenoughitems
python3 tools/j2h_patcher.py status --profile "$HOME/Horizon-J2H-26.3"
python3 tools/j2h_patcher.py launch --profile "$HOME/Horizon-J2H-26.3" --creative
```

`--mods` accepts IDs from `tools/j2h_compat/catalog.json`; required pinned dependencies are installed automatically and verified against SHA-512 plus Fabric metadata. Omitting `--mods` installs an empty J2H profile. `launch` uses the profile for game data and logs. Back up any test saves you want to keep before removing a profile.

This is an early compatibility build, not a general-purpose Fabric replacement. Java mods run with the user's process permissions and are not sandboxed by J2H. The catalog and scorecard at `tools/j2h_compat/` record exact artifacts and evidence-backed status. An earlier Mod Menu regression passed seven GUI assertions, but the latest live rerun passed six and failed its scrolling assertion; Mod Menu is not currently marked verified. REI initializes, but its config screen hits a missing `Minecraft.gameRenderer` panorama path and its inventory overlay is not hosted by Horizon's Rust inventory. Do not treat REI as fully compatible. Most Fabric events, game APIs, mod rendering, and gameplay behavior remain incomplete or untested.

The existing GitHub Actions `checks` workflow runs the offline Python test suite, validates the pinned J2H catalog, and performs the repository's Rust formatting, lint, and tests. The established game-build and signed nightly/release workflows produce the cross-platform client; a profile installer never replaces or modifies those release pipelines.
