# J2H compatibility laboratory

The lab uses a committed, version-pinned catalog and a machine-readable scorecard. Artifact bytes and live-run data stay under ignored `.local/j2h-compat/`; each run gets fresh game, launcher, XDG, cache, mods, control-socket, logs, and evidence directories. Python GUI assertions use Pillow to measure rendered list/button regions and compare the details pane before and after scrolling.

## Commands

```sh
python3 tools/j2h_compat.py catalog
python3 tools/j2h_compat.py scorecard
python3 tools/j2h_compat.py validate
python3 tools/j2h_compat.py fetch modmenu fabric-api sodium lithium ferritecore iris cloth-config jade appleskin cardinal-components architectury roughlyenoughitems
python3 tools/j2h_compat.py run --mods modmenu --run-id manual-modmenu
python3 tools/j2h_compat.py run --fixtures brokenmod goodmod optionalmod requiredmod ruby --run-id transaction-fixture
python3 tools/j2h_compat.py run --mods sodium --fixtures sodium_config_probe --run-id sodium-config-roundtrip
python3 tools/j2h_compat.py run --mods jade --fixtures jade_rule_probe --run-id jade-game-rule-probe
python3 tools/j2h_compat.py run --mods architectury --fixtures architectury_payload_accessor_probe --run-id attachment-accessor
bash tools/j2h_compat/manual_play.sh
```

`manual_play.sh` opens a visible Horizon window with the catalog-pinned Mod Menu + REI combination and its required Fabric API, Architectury, and Cloth Config dependencies. Before launch it verifies each artifact's SHA-512 and Fabric metadata. Its persistent, isolated game profile is `.local/j2h-compat/manual-rei-26.3/` (override with `J2H_MANUAL_PROFILE`). It does not use or modify the user's regular Minecraft/Horizon profile. On sessions without a Wayland compositor it selects X11 by removing `WAYLAND_DISPLAY`.

In the world, press `E`, hover a creative inventory item, and press `R` to open REI's recipe view or `U` for usages. Use Escape to close. Press `M`, search for Roughly Enough Items, and choose Configure to inspect the REI settings screen. Game logs and configuration are retained under the isolated profile for diagnosis. This launches the real game for manual inspection; it does not mark REI compatible or claim that its screens currently render correctly.

`fetch` accepts HTTPS only from `cdn.modrinth.com`, checks the artifact against the published SHA-512, then checks its embedded Fabric ID, version, and `depends` metadata against the lock. Required catalog dependencies are expanded in dependency-first order; optional dependencies stay optional. `stage` accepts an already-acquired artifact only when its checksum and metadata match. `run` refuses unpinned files, stages only the selected mods and their required dependencies in a fresh isolated game directory, launches the optimized client through `horizon-ctl`, captures game and JVM logs, screenshots, startup state, GUI state, and generated Java content roots, and shuts down through the control socket. A startup-only run records `IN_PROGRESS`; the Mod Menu run adds real mouse/keyboard search, scroll, Escape, reopen, and screenshot assertions before it can be marked `VERIFIED_COMPATIBLE`.

Every `result.json` is immutable by run ID and linked from the scorecard. Fixture combinations assert rollback, required/optional dependency behavior, Ruby block/item content, and the committed content roots against live logs. Tests must distinguish runtime startup from tested mod behavior. For external combinations, pass multiple catalog IDs to one run; successful individual runs do not imply `VERIFIED_IN_COMBINATION`.

## Baseline already recorded

`modmenu` 21.0.0 (Modrinth version `kyy7dbrZ`, MIT) has repeatable live GUI assertions: its entrypoint initializes, the screen opens and renders, search hides and restores the mod card, scrolling changes the details pane, Escape closes the screen, reopening works, and both link-button label regions contain rendered text. Run artifacts are retained under `.local/j2h-compat/runs/`.

Fabric API 0.160.6+26.3 is pinned as the first broad API baseline (Modrinth version `BQfN2OGk`, Apache-2.0). Its live runs discover 43 nested modules, transform the Fabric Transfer API `ChiseledBookShelfBlockEntity` Mixin, and reach the playing screen. A new live dependent probe calls the transformed `ClientboundCustomPayloadPacketAccessor.getMaxPayloadSize()` and gets the pinned 26.3 vanilla value, 1048576. Fabric API is `PARTIALLY_COMPATIBLE` for that accessor behavior; broader events, registries, resource, transport, and renderer APIs remain untested. A previous Architectury/Fabric attachment initialization failure is cleared in the post-Mixin run: it transforms the accessor and registers `architectury:spawn_entity_packet`. Architectury remains `IN_PROGRESS` because no payload send/receive behavior was tested. Lithium and FerriteCore also reach a world together and J2H observes Lithium's access-widener transformations; their mixin-driven gameplay and memory changes remain unverified. The Sodium config probe preseeds `quality.hidden_fluid_culling=false` in the isolated game directory and checks Sodium's initialized options object, then toggles `animateOnlyVisibleTextures`, reloads the changed value, restores it, and reloads the original value. Sodium is `PARTIALLY_COMPATIBLE` for this startup config and file persistence behavior only; its renderer output and performance effects remain unverified because Horizon uses its own Rust renderer. Jade's pinned registry-sync Bootstrap Mixin now runs, and a live dependent probe confirms `jade:max_position_deviation` is in the Java `GAME_RULE` registry with default 21. Jade is `PARTIALLY_COMPATIBLE`; its HUD/world behavior and use of that dynamic rule in Horizon gameplay remain unverified. AppleSkin initializes without an entrypoint failure, with its HUD behavior unverified. Evidence is in the scorecard and isolated run folders.

Cardinal Components API 8.1.0 is also pinned. Its outer JAR has no declared dependencies, but its eight nested modules require Fabric API modules. The catalog therefore expands the pinned Fabric API dependency automatically. The live combination reaches gameplay; component attach/read/save behavior still needs a consumer fixture and is not verified.

Roughly Enough Items 26.3.824+fabric now passes its common and client entrypoints with Cloth Config, Architectury, and Fabric API in the post-provider live run. The probe confirms Fabric Loader exposes the pinned Minecraft `26.3` built-in metadata and records REI config plus default common/client plugin-provider registration. REI is `PARTIALLY_COMPATIBLE` for startup and plugin registration only; its configuration screen, recipe viewing, inventory overlay, and interaction behavior remain unverified. The current run is `regression-rei-minecraft-builtin-provider-20261107-final`.

The Mixin regression fixture covers an anonymous class in both a top-level and nested target, repeated nested invocation, and actual generated-class loading. Run it explicitly with `cargo test -p java2horizon --test mixin_runtime -- --ignored`; it needs the locally prepared Minecraft 26.3 and pinned Fabric libraries. The real Mod Menu GUI regression was rerun after the generated-class change and all seven assertions passed. The latest run IDs are `regression-fapi-sodium-nested-mixin-20261106` and `regression-modmenu-nested-mixin-20261106`.

The initial 12-mod catalog uses Modrinth's official project/version metadata and exact files/checksums for Mod Menu, Fabric API, Sodium, Lithium, FerriteCore, Iris, Cloth Config, Jade, AppleSkin, Cardinal Components API, Architectury API, and Roughly Enough Items. Candidate versions and dependencies are pinned under `catalog.json`; `scorecard.json` lists current evidence-backed statuses. Sodium's pinned option/config bytecode was inspected to ground the configuration probe; renderer behavior has not been established. Iris remains run-only and unverified. Jade is licensed CC-BY-NC-SA and stays in the local ignored artifact cache; it is not redistributed.

## Safety and scope

The runtime is not an OS sandbox. Only test jars from the pinned catalog are staged by default. The game's files and prepared resources remain in the user's existing `.local` tree and are read-only inputs to the run; no Mojang artifacts are copied to Git. Do not add arbitrary JARs to the catalog or run them unless their distribution source, version, license, and checksum are known and the isolated-run implications are acceptable.
