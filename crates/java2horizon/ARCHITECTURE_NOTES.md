# Java2Horizon — architectural notes

## Where J2H sits

`horizon-client` runs J2H from `game/java_mods.rs` before it builds the
resource root. J2H discovers jars in `<game_dir>/mods`, resolves their
dependencies, starts one JVM, runs their `main` entrypoints, and writes one
Horizon content-mod folder per namespace. `game/mods.rs` then reads those
folders through the normal `horizon-mods` + `mod_content` path.

```
Fabric jar -> J2H fabric loader -> JVM entrypoint
           -> JNI NativeBridge -> HorizonEngine
           -> content-mod folder -> horizon-mods Content::read
           -> mod_content::resource_root -> game
```

## Inspected Horizon surfaces

- `crates/horizon-world/src/registry.rs`: `BlockRegistry`, `BlockStateId`,
  append-only states, `default_state(name)`.
- `crates/horizon-mods/src/content.rs`: content-mod `Block { id, template,
  properties, item, ... }`.
- `crates/horizon-client/src/mod_content.rs`: overlays content-mod blocks onto
  the prepared resource root; vanilla state ids never move.
- `crates/horizon-client/src/game/native_hooks.rs`: behaviour hooks for native
  mods (not dynamic block registration).
- `crates/cargo-horizon-mod/template/content/block.json`: template copy schema.

## How blocks enter Horizon

Horizon has **no** runtime `register_block(id, name)` for native or Java code.
Blocks are declared as content-mod JSON:

```
data/<ns>/horizon/block/<path>.json
{ "template": "minecraft:…", "properties": { … }, "item": true }
```

The client builds a resource-root overlay (`mod_content::build`); loaders read
tables indexed by state id / name. J2H therefore records Java registrations,
then emits that data before the resource root is built.

## Incompatibilities

1. **No dynamic registry API**: registration is data-plane only. Native mods
   use compile-time hooks, not dynamic type registration.
2. **Prepared assets**: a live `Game` needs Minecraft 26.3 prepared files
   (`horizon-prepare`); they are not redistributed.
3. **Immutable vanilla range**: mod blocks append after vanilla; ids are not
   free-form handles.
4. **Mixin / mappings / Fabric API**: absent, so most ecosystem mods cannot
   reach their own initialization code.

## What the lifecycle implements

- `HorizonEngine` mirrors the content-mod schema and append-after-vanilla
  identity model.
- Java `registerBlock` -> schema-valid block + sequential state id >= vanilla
  base + content-mod overlay files at Horizon paths.
- Automated unit tests for schema, ids, duplicates, overlay I/O and engine
  apply.
- `FabricLoader` multi-mod discovery, dependency presence and ordered
  entrypoints.
