# Compatibility gap report (historical Milestone 3 snapshot)

> This document records an early prototype state and is no longer an authoritative description of J2H. The current pinned-mod evidence and statuses are in [`tools/j2h_compat/scorecard.json`](../../tools/j2h_compat/scorecard.json); live evidence and reproducible commands are documented in [`tools/j2h_compat/README.md`](../../tools/j2h_compat/README.md) and [`docs/java-mods.md`](../../docs/java-mods.md).

## Toolchain

- Installed **rustc 1.99.1** (stable) via rustup under `/tmp/rustup`.
- Horizon workspace `rust-version = "1.85"`; `horizon-mods` pins **1.95** — satisfied by 1.99.1.
- `horizon-core` and **`horizon-mods` path-linked and compiling**.
- `horizon-world` OOM'd (SIGKILL) during compile in this environment — BlockRegistry NBT path still local.

## What validates against real Horizon code

| Step | Status |
|------|--------|
| Emit `horizon-mod.toml` + `data/<ns>/horizon/block|item/*.json` | Working |
| `Manifest::parse` + `Content::read` from **horizon-mods** | Working |
| Multi-mod dep order (libmod → testmod) | Working |
| Expanded bridge: block props, item, light, hardness | Working |
| Staged lifecycle Discover→…→Ready | Working |

## Fabric Loader reality

Current layer is **not** Fabric Loader. Missing for real mods:

1. **Mixin / MixinExtras** — majority of non-trivial Fabric mods
2. **Yarn/official mappings + intermediary** — Minecraft class access
3. **Fabric API** modules (registry, object builder, lifecycle events, networking, renderer…)
4. **Access wideners**
5. **Full dependency download** from Modrinth/Maven (Fabric Loader 0.20.1,
   ASM 9.10.1 and sponge-mixin 0.17.5 are now fetched and hash-checked)
6. **Classpath isolation** (nested jars are extracted; isolation is missing)

## Common patterns (from ecosystem knowledge + Modrinth categories)

| Class of mod | Typical needs | J2H today |
|--------------|---------------|-----------|
| Pure content (blocks/items via API) | Registry hooks | Partial (bridge → content overlay) |
| Library (e.g. Cloth Config, Architectury) | Mixin + API | Missing |
| Utility (Inventory HUD, MiniHUD) | Client mixin, rendering | Missing |
| Worldgen | Fabric biome/API or mixin | Missing |
| Tech (Create, AE2) | Heavy mixin, networking, BE | Missing |

## Next priority interfaces

1. Fabric Registry API shim → Horizon content overlay (expand what M3 started)
2. Mixin subsystem against Horizon hooks or a Java Minecraft facade
3. Link `horizon-world` BlockRegistry when memory allows
4. Real Fabric Loader embed experiment (ClassLoader + Knot)

## Verification levels

- **L1:** horizon-mods `Content::read` accepts generated trees — **PASS**
- **L2:** `mod_content::build` on a prepared 26.3 root — not run here (no prepared assets)
- **L3:** in-game block place — not run here (no playable Horizon session here)

The crate is now part of the Horizon workspace; `cargo test -p java2horizon`
and `cargo run -p java2horizon` pass on the two milestone jars. The game hook
is `crates/horizon-client/src/game/java_mods.rs`.

## Real-mod probe: Mod Menu 21.0.0

With official Fabric Loader 0.20.1 and its ASM/Mixin runtime on the classpath,
J2H now constructs `LifecycleEventsImpl` and calls its instance `onInitialize`.
The next blocker is `com.google.common.collect.MapMaker`: Guava is a Minecraft
runtime library that Fabric API expects Minecraft to provide, not a Fabric
Loader dependency. Mod Menu's own client entrypoint has not run yet.
