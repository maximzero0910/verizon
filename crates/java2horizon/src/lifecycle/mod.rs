//! Staged lifecycle reconciling Fabric initialization with Horizon content
//! preparation.
//!
//! Fabric expects registration during `ModInitializer.onInitialize()`.
//! Horizon expects content definitions present when its resource root is
//! built, before a world loads. J2H runs the Java entrypoints while
//! recording bridge registrations, writes a Horizon content-mod tree, then
//! validates it with `horizon-mods`.
//!
//! Discover -> ResolveDeps -> ResolveLibraries -> StartJvm -> LoadClasses ->
//! CollectContent -> EmitHorizonContent -> PrepareResources -> PostInit -> Ready

use crate::bridge::{Bridge, RegistrationTransaction};
use crate::fabric::{EntrypointSide, FabricEntrypoint, FabricLoader};
use crate::horizon_shim::HorizonEngine;
use crate::jvm::{JvmConfig, JvmRuntime};
use log::{info, warn};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Discover,
    ResolveDeps,
    ResolveLibraries,
    StartJvm,
    LoadClasses,
    CollectContent,
    EmitHorizonContent,
    PrepareResources,
    PostInit,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModInitializationStatus {
    Initialized,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModInitializationRecord {
    pub mod_id: String,
    pub status: ModInitializationStatus,
    pub diagnostics: Vec<String>,
}

#[derive(Error, Debug)]
pub enum LifecycleError {
    #[error("stage {0:?}: {1}")]
    Stage(Stage, String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub struct Lifecycle {
    pub stage: Stage,
    pub loader: FabricLoader,
    pub jvm: Option<JvmRuntime>,
    pub bridge: Bridge,
    pub engine: HorizonEngine,
    pub content_roots: Vec<PathBuf>,
    /// Per-mod result records from the latest entrypoint initialization pass.
    pub mod_initializations: Vec<ModInitializationRecord>,
    pub side: EntrypointSide,
    library_cache: Option<PathBuf>,
    game_dir: Option<PathBuf>,
    resource_root: Option<PathBuf>,
    mods_dir: Option<PathBuf>,
    preflight_failures: HashMap<String, String>,
    access_widener_agent: Option<(PathBuf, String)>,
    access_widener_directory: Option<PathBuf>,
    minecraft_client_jar: Option<PathBuf>,
    client_identity: Option<(String, String)>,
}

impl Lifecycle {
    pub fn new() -> Self {
        Self {
            stage: Stage::Discover,
            loader: FabricLoader::new(),
            jvm: None,
            bridge: Bridge::new(),
            engine: HorizonEngine::new(),
            content_roots: Vec::new(),
            mod_initializations: Vec::new(),
            side: EntrypointSide::Client,
            library_cache: None,
            game_dir: None,
            resource_root: None,
            mods_dir: None,
            preflight_failures: HashMap::new(),
            access_widener_agent: None,
            access_widener_directory: None,
            minecraft_client_jar: None,
            client_identity: None,
        }
    }

    /// The game folder the loader reports as its game dir; its `config/`
    /// subfolder becomes the loader's config dir.
    pub fn set_game_dir(&mut self, game_dir: PathBuf) {
        self.game_dir = Some(game_dir);
    }

    /// The prepared resource root the game opened, for mods that resolve
    /// vanilla sprites, atlases or other pack resources themselves.
    pub fn set_resource_root(&mut self, root: PathBuf) {
        self.resource_root = Some(root);
    }

    /// The folder the discovered jars live in; defaults to `game_dir/mods`.
    pub fn set_mods_dir(&mut self, dir: PathBuf) {
        self.mods_dir = Some(dir);
    }

    /// Where official Fabric Loader libraries are cached (or read from
    /// `HORIZON_FABRIC_LIBS_DIR`).
    pub fn set_library_cache(&mut self, cache: PathBuf) {
        self.library_cache = Some(cache);
    }

    /// Run `main` + `client` (the default) or `main` + `server` entrypoints.
    pub fn set_side(&mut self, side: EntrypointSide) {
        self.side = side;
        self.loader.set_side(side);
    }

    /// Set the local player's display name and UUID for Minecraft client mods.
    /// Authentication credentials are deliberately not part of this bridge.
    pub fn set_client_identity(&mut self, name: String, uuid: String) {
        self.client_identity = Some((name, uuid));
    }

    pub fn discover_mod(&mut self, jar: PathBuf, meta: &Path) -> Result<(), LifecycleError> {
        self.stage = Stage::Discover;
        self.loader
            .add_mod(jar, meta)
            .map_err(|e| LifecycleError::Stage(Stage::Discover, e.to_string()))?;
        Ok(())
    }

    /// Discover every Fabric mod in `dir`; nested jars are unpacked under
    /// `nested_cache`.
    pub fn discover_dir(
        &mut self,
        dir: &Path,
        nested_cache: Option<&Path>,
    ) -> Result<usize, LifecycleError> {
        self.stage = Stage::Discover;
        let added = self.loader.discover_dir_with_nested(dir, nested_cache);
        info!("J2H: discovered {added} mod(s) in {}", dir.display());
        Ok(added)
    }

    pub fn resolve_deps(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::ResolveDeps;
        self.loader
            .check_dependencies()
            .map_err(|e| LifecycleError::Stage(Stage::ResolveDeps, e.to_string()))?;
        info!("J2H: dependencies OK for {:?}", self.loader.mod_ids());
        Ok(())
    }

    pub fn start_jvm(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::StartJvm;
        let mut jars = self.loader.jars();
        if jars.is_empty() {
            return Err(LifecycleError::Stage(
                Stage::StartJvm,
                "no Java mods to start a JVM for".into(),
            ));
        }
        let cache = self
            .library_cache
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("horizon-java2horizon"));
        let runtime_jar = cache.join("j2h-runtime.jar");
        if let Err(e) = std::fs::create_dir_all(&cache)
            .and_then(|_| std::fs::write(&runtime_jar, crate::J2H_RUNTIME_JAR))
        {
            return Err(LifecycleError::Stage(
                Stage::StartJvm,
                format!("writing j2h-runtime.jar: {e}"),
            ));
        }
        if !jars.contains(&runtime_jar) {
            jars.insert(0, runtime_jar);
        }
        let mut config = JvmConfig::with_classpath(jars);
        if let Some(root) = &self.resource_root {
            config
                .options
                .push(format!("-Dj2h.resources_dir={}", root.to_string_lossy()));
        }
        if let Some(dir) = self
            .mods_dir
            .clone()
            .or_else(|| self.game_dir.as_ref().map(|game| game.join("mods")))
        {
            config
                .options
                .push(format!("-Dj2h.mods_dir={}", dir.to_string_lossy()));
        }
        if let Some((agent, arguments)) = &self.access_widener_agent {
            config
                .options
                .push(format!("-javaagent:{}={arguments}", agent.display()));
        }
        let jvm = JvmRuntime::start(config)
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        crate::gui::attach_runtime(&jvm);
        self.jvm = Some(jvm);
        Ok(())
    }

    /// Bootstrap the real Fabric Loader singleton with J2H's discovered mods:
    /// a launcher is installed and each mod jar is registered through the
    /// loader's own metadata parser, so `FabricLoader.getInstance()` answers
    /// `isModLoaded`, `getAllMods` and `getEntrypointContainers`.
    pub fn bootstrap_loader(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::StartJvm;
        if !self.loader.needs_fabric_runtime() {
            // No mod expects the Fabric Loader API, so the loader jar is not
            // on the classpath and there is no singleton to populate.
            info!("J2H: no Fabric runtime requested; skipping loader bootstrap");
            return Ok(());
        }
        let jvm = self
            .jvm
            .as_ref()
            .ok_or_else(|| LifecycleError::Stage(Stage::StartJvm, "JVM not started".into()))?;
        jvm.call_static_void(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "installLauncher",
            "()V",
        )
        .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        if let Some(game_dir) = self.game_dir.as_ref() {
            let dir = game_dir.to_string_lossy().into_owned();
            jvm.call_static_void_with_string(
                "dev/horizon/j2h/FabricLoaderBootstrap",
                "setGameDir",
                "(Ljava/lang/String;)V",
                &dir,
            )
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        }
        let minecraft_client_jar = self.minecraft_client_jar.as_ref().ok_or_else(|| {
            LifecycleError::Stage(
                Stage::StartJvm,
                "Fabric runtime requires the resolved Minecraft client jar for its built-in mod provider".into(),
            )
        })?;
        jvm.call_static_void_with_two_strings(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "initializeGameProvider",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            horizon_core::version::VERSION_ID,
            &minecraft_client_jar.to_string_lossy(),
        )
        .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        let mods = self.loader.mods();
        info!(
            "J2H: bootstrapping Fabric Loader with {} discovered mod(s)",
            mods.len()
        );
        for m in mods {
            let path = m.jar.to_string_lossy().into_owned();
            jvm.call_static_void_with_string(
                "dev/horizon/j2h/FabricLoaderBootstrap",
                "registerMod",
                "(Ljava/lang/String;)V",
                &path,
            )
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, format!("{}: {e}", m.meta.id)))?;
        }
        jvm.call_static_void(
            "dev/horizon/j2h/FabricLoaderBootstrap",
            "finalizeLoader",
            "()V",
        )
        .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        let registered = jvm
            .call_static_i32("dev/horizon/j2h/FabricLoaderBootstrap", "modCount", "()I")
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        if registered as usize != mods.len() {
            return Err(LifecycleError::Stage(
                Stage::StartJvm,
                format!(
                    "loader registered {registered}/{} discovered mod(s)",
                    mods.len()
                ),
            ));
        }
        for (method, expected) in [
            (
                "minecraftMetadataVersion",
                horizon_core::version::VERSION_ID,
            ),
            ("rawGameVersion", horizon_core::version::VERSION_ID),
        ] {
            let reported = jvm
                .call_static_string(
                    "dev/horizon/j2h/FabricLoaderBootstrap",
                    method,
                    "()Ljava/lang/String;",
                )
                .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
            if reported != expected {
                return Err(LifecycleError::Stage(
                    Stage::StartJvm,
                    format!("Fabric {method} returned {reported:?}, expected {expected:?}"),
                ));
            }
        }
        for m in mods {
            let loaded = jvm
                .call_static_bool_with_string(
                    "dev/horizon/j2h/FabricLoaderBootstrap",
                    "modLoaded",
                    "(Ljava/lang/String;)Z",
                    &m.meta.id,
                )
                .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
            if !loaded {
                return Err(LifecycleError::Stage(
                    Stage::StartJvm,
                    format!("loader did not answer isModLoaded({})", m.meta.id),
                ));
            }
        }
        if self.access_widener_agent.is_some() {
            let environment = match self.side {
                EntrypointSide::Client => "client",
                EntrypointSide::Server => "server",
            };
            jvm.call_static_void_with_string(
                "dev/horizon/j2h/FabricLoaderBootstrap",
                "initializeMixins",
                "(Ljava/lang/String;)V",
                environment,
            )
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        }
        if self.side == EntrypointSide::Client {
            Self::set_up_client_runtime(jvm, self.client_identity.as_ref())?;
        }
        if self.access_widener_agent.is_some() {
            let mixin_failure = jvm
                .call_static_string(
                    "dev/horizon/j2h/AccessWidenerAgent",
                    "mixinTransformFailure",
                    "()Ljava/lang/String;",
                )
                .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
            if !mixin_failure.is_empty() {
                return Err(LifecycleError::Stage(Stage::StartJvm, mixin_failure));
            }
        }
        info!("J2H: Fabric Loader singleton verified: {registered} mod(s) registered and visible",);
        Ok(())
    }

    /// Establish the client-only pre-options state: a genuine `Minecraft`
    /// singleton, real game version, and Horizon's local profile before any
    /// entrypoint runs. Repeated installs with the same identity are safe.
    fn set_up_client_runtime(
        jvm: &JvmRuntime,
        identity: Option<&(String, String)>,
    ) -> Result<(), LifecycleError> {
        let install = if let Some((name, uuid)) = identity {
            jvm.call_static_void_with_two_strings(
                "dev/horizon/j2h/MinecraftBootstrap",
                "installWithIdentity",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                name,
                uuid,
            )
        } else {
            jvm.call_static_void("dev/horizon/j2h/MinecraftBootstrap", "install", "()V")
        };
        install.map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        let ready = jvm
            .call_static_bool(
                "dev/horizon/j2h/MinecraftBootstrap",
                "minecraftReady",
                "()Z",
            )
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        if !ready {
            return Err(LifecycleError::Stage(
                Stage::StartJvm,
                "Minecraft singleton not available in pre-options state".into(),
            ));
        }
        if let Some((name, uuid)) = identity {
            let matches = jvm
                .call_static_bool_with_two_strings(
                    "dev/horizon/j2h/MinecraftBootstrap",
                    "identityMatches",
                    "(Ljava/lang/String;Ljava/lang/String;)Z",
                    name,
                    uuid,
                )
                .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
            if !matches {
                return Err(LifecycleError::Stage(
                    Stage::StartJvm,
                    "Minecraft singleton does not match Horizon's local profile".into(),
                ));
            }
        }
        let version = jvm
            .call_static_string(
                "dev/horizon/j2h/MinecraftBootstrap",
                "currentVersionId",
                "()Ljava/lang/String;",
            )
            .map_err(|e| LifecycleError::Stage(Stage::StartJvm, e.to_string()))?;
        if version.is_empty() {
            return Err(LifecycleError::Stage(
                Stage::StartJvm,
                "game version not detected by SharedConstants".into(),
            ));
        }
        info!("J2H: Minecraft client state ready (version {version}, pre-options)");
        Ok(())
    }

    pub fn load_classes_and_bridge(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::LoadClasses;
        let jvm = self
            .jvm
            .as_ref()
            .ok_or_else(|| LifecycleError::Stage(Stage::LoadClasses, "JVM not started".into()))?;
        let registered = self
            .bridge
            .register_natives(jvm)
            .map_err(|e| LifecycleError::Stage(Stage::LoadClasses, e.to_string()))?;
        if registered == 0 {
            warn!("J2H: no NativeBridge class; mods cannot register content");
        }
        // The GUI bridge's draw-call natives require the Minecraft client jar
        // (GuiGraphicsExtractor, Font, Gui etc.) on the classpath.  A mod-only
        // environment (simple block/item jars) has none.
        if self.loader.needs_minecraft_runtime()
            && jvm
                .has_class("net/minecraft/client/Minecraft")
                .unwrap_or(false)
        {
            crate::gui::register_natives(jvm)
                .map_err(|e| LifecycleError::Stage(Stage::LoadClasses, e.to_string()))?;
        }
        Ok(())
    }

    /// Run each mod's main entrypoints, in dependency order, with bridge
    /// events attributed to the running mod.
    pub fn collect_content(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::CollectContent;
        let jvm = self.jvm.as_ref().ok_or_else(|| {
            LifecycleError::Stage(Stage::CollectContent, "JVM not started".into())
        })?;
        let mut provider_owner = HashMap::new();
        for m in self.loader.mods() {
            provider_owner.insert(m.meta.id.clone(), m.meta.id.clone());
            for alias in &m.meta.provides {
                provider_owner.insert(alias.clone(), m.meta.id.clone());
            }
        }
        let order: Vec<(String, Vec<String>, Vec<FabricEntrypoint>)> = self
            .loader
            .ordered()
            .into_iter()
            .map(|m| {
                let failed_dependencies = m
                    .meta
                    .depends
                    .keys()
                    .filter_map(|dep| provider_owner.get(dep).cloned())
                    .collect();
                (
                    m.meta.id.clone(),
                    failed_dependencies,
                    m.entrypoints(self.side.keys())
                        .into_iter()
                        .cloned()
                        .collect(),
                )
            })
            .collect();
        let provided_by_mod: HashMap<String, Vec<String>> = self
            .loader
            .mods()
            .iter()
            .map(|m| (m.meta.id.clone(), m.meta.provides.clone()))
            .collect();
        let mut failed_mods = HashSet::new();
        self.mod_initializations.clear();

        for (mod_id, dependencies, entrypoints) in order {
            if let Some(diagnostic) = self.preflight_failures.get(&mod_id) {
                log::error!("J2H: mod '{mod_id}' initialization failed: {diagnostic}");
                failed_mods.insert(mod_id.clone());
                self.mod_initializations.push(ModInitializationRecord {
                    mod_id,
                    status: ModInitializationStatus::Failed,
                    diagnostics: vec![diagnostic.clone()],
                });
                continue;
            }
            if let Some(failed_dependency) =
                dependencies.iter().find(|id| failed_mods.contains(*id))
            {
                let diagnostic =
                    format!("required dependency '{failed_dependency}' failed initialization");
                warn!("J2H: skipping mod '{mod_id}': {diagnostic}");
                failed_mods.insert(mod_id.clone());
                self.mod_initializations.push(ModInitializationRecord {
                    mod_id,
                    status: ModInitializationStatus::Skipped,
                    diagnostics: vec![diagnostic],
                });
                continue;
            }

            if self.access_widener_agent.is_some() {
                let rejected = jvm
                    .call_static_bool_with_string(
                        "dev/horizon/j2h/AccessWidenerAgent",
                        "modRejected",
                        "(Ljava/lang/String;)Z",
                        &mod_id,
                    )
                    .map_err(|e| LifecycleError::Stage(Stage::CollectContent, e.to_string()))?;
                if rejected {
                    let diagnostic = jvm
                        .call_static_string_with_string(
                            "dev/horizon/j2h/AccessWidenerAgent",
                            "modFailure",
                            "(Ljava/lang/String;)Ljava/lang/String;",
                            &mod_id,
                        )
                        .map_err(|e| LifecycleError::Stage(Stage::CollectContent, e.to_string()))?;
                    log::error!("J2H: mod '{mod_id}' initialization failed: {diagnostic}");
                    failed_mods.insert(mod_id.clone());
                    self.mod_initializations.push(ModInitializationRecord {
                        mod_id,
                        status: ModInitializationStatus::Failed,
                        diagnostics: vec![diagnostic],
                    });
                    continue;
                }
            }

            let transaction = RegistrationTransaction::new(mod_id.clone());
            let initialized = self.bridge.with_transaction(&transaction, || {
                for entrypoint in &entrypoints {
                    let unsupported_api = match entrypoint.class_binary_name.as_str() {
                        "net/fabricmc/fabric/impl/menu/Networking" => {
                            Some("minecraft:menu registry listeners")
                        }
                        "net/fabricmc/fabric/impl/menu/client/ClientNetworking" => {
                            Some("fabric-menu-api-v1 network payloads")
                        }
                        "net/fabricmc/fabric/impl/recipe/ingredient/CustomIngredientInit" => {
                            Some("custom recipe ingredients")
                        }
                        "net/fabricmc/fabric/impl/client/renderer/DebugOverlayClient" => {
                            Some("Fabric renderer debug overlay hook")
                        }
                        "net/fabricmc/fabric/impl/attachment/sync/AttachmentSync" => {
                            Some("synchronized data attachments")
                        }
                        "net/fabricmc/fabric/impl/attachment/client/AttachmentSyncClient" => {
                            Some("client synchronized data attachments")
                        }
                        _ => None,
                    };
                    if let Some(capability) = unsupported_api {
                        warn!(
                            "J2H: skipping unsupported Fabric API initializer {} ({capability})",
                            entrypoint.class_binary_name,
                        );
                        continue;
                    }
                    entrypoint.run(jvm).map_err(|e| {
                        format!(
                            "{}:{} {} failed: {e}",
                            entrypoint.mod_id, entrypoint.key, entrypoint.class_binary_name
                        )
                    })?;
                }
                Ok::<(), String>(())
            });

            if let Err(error) = initialized {
                transaction.rollback();
                let mut diagnostics = vec![error.clone()];
                diagnostics.extend(irreversible_java_side_effects(self.side));
                log::error!("J2H: mod '{mod_id}' initialization failed: {error}");
                failed_mods.insert(mod_id.clone());
                self.mod_initializations.push(ModInitializationRecord {
                    mod_id,
                    status: ModInitializationStatus::Failed,
                    diagnostics,
                });
                continue;
            }

            let callbacks = transaction.events();
            let provides = provided_by_mod
                .get(&mod_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if let Err(error) = self.engine.apply_transaction(&mod_id, provides, &callbacks) {
                transaction.rollback();
                let diagnostic = format!("registration validation failed: {error}");
                warn!("J2H: mod '{mod_id}' transaction rolled back: {diagnostic}");
                failed_mods.insert(mod_id.clone());
                let mut diagnostics = vec![diagnostic];
                diagnostics.extend(irreversible_java_side_effects(self.side));
                self.mod_initializations.push(ModInitializationRecord {
                    mod_id,
                    status: ModInitializationStatus::Failed,
                    diagnostics,
                });
                continue;
            }

            transaction.commit();

            self.mod_initializations.push(ModInitializationRecord {
                mod_id,
                status: ModInitializationStatus::Initialized,
                diagnostics: Vec::new(),
            });
        }
        Ok(())
    }

    /// Drain bridge events into the Horizon engine and write content-mod trees.
    pub fn emit_horizon_content(&mut self, out_root: &Path) -> Result<(), LifecycleError> {
        self.stage = Stage::EmitHorizonContent;
        let written = self
            .engine
            .emit_content_mods(out_root)
            .map_err(|e| LifecycleError::Stage(Stage::EmitHorizonContent, e.to_string()))?;
        self.content_roots = written;
        info!(
            "J2H: emitted {} content-mod root(s) under {}",
            self.content_roots.len(),
            out_root.display()
        );
        Ok(())
    }

    /// Validate emitted trees with real `horizon_mods::content::Content::read`.
    pub fn prepare_resources(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::PrepareResources;
        self.engine
            .validate_with_horizon_mods(&self.content_roots)
            .map_err(|e| LifecycleError::Stage(Stage::PrepareResources, e.to_string()))?;
        Ok(())
    }

    pub fn post_init(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::PostInit;
        // Client and server post-init entrypoints run after content is fixed.
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::Ready;
        info!("J2H: Ready");
        Ok(())
    }

    /// Run the full staged pipeline.
    pub fn run_all(&mut self, content_out: &Path) -> Result<(), LifecycleError> {
        self.resolve_deps()?;
        self.resolve_libraries()?;
        self.start_jvm()?;
        self.bootstrap_loader()?;
        self.load_classes_and_bridge()?;
        self.collect_content()?;
        self.emit_horizon_content(content_out)?;
        self.prepare_resources()?;
        self.post_init()?;
        self.finish()?;
        Ok(())
    }

    fn resolve_libraries(&mut self) -> Result<(), LifecycleError> {
        self.stage = Stage::ResolveLibraries;
        let cache = self
            .library_cache
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("horizon-java2horizon"));

        if self.loader.needs_fabric_runtime() {
            let already = self.classpath_names();
            let jars = crate::fabric::libraries::ensure_libraries(&cache, &already)
                .map_err(|e| LifecycleError::Stage(Stage::ResolveLibraries, e.to_string()))?;
            for jar in jars {
                self.loader.add_library_classpath(jar);
            }
        }

        if self.loader.needs_minecraft_runtime() {
            let already = self.classpath_names();
            let resolver = crate::minecraft::Resolver::from_environment(&cache);
            let jars = resolver
                .ensure(&already)
                .map_err(|e| LifecycleError::Stage(Stage::ResolveLibraries, e.to_string()))?;
            // Resolver::ensure documents and preserves the client jar as its
            // first entry. Mojang's published filename can change per version
            // (26.3 currently uses `client-26.3.jar`).
            let client_jar = jars.first().cloned();
            self.minecraft_client_jar = client_jar.clone();
            for jar in jars {
                self.loader.add_library_classpath(jar);
            }
            let access_wideners = self.stage_access_wideners(&cache);
            match client_jar {
                Some(client) => {
                    let agent = cache.join("fabric").join("j2h-access-widener-agent.jar");
                    fs::write(&agent, crate::J2H_ACCESS_WIDENER_AGENT_JAR).map_err(|e| {
                        LifecycleError::Stage(
                            Stage::ResolveLibraries,
                            format!("writing JVM instrumentation agent {}: {e}", agent.display()),
                        )
                    })?;
                    self.loader.add_library_classpath(agent.clone());
                    let directory = self
                        .access_widener_directory
                        .as_ref()
                        .expect("access widener staging directory is established");
                    self.access_widener_agent = Some((
                        agent,
                        format!("{}|{}", directory.display(), client.display()),
                    ));
                }
                None => {
                    for (mod_id, _) in access_wideners {
                        self.preflight_failures.insert(
                            mod_id,
                            "access widener requires the resolved Minecraft client jar".into(),
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn stage_access_wideners(&mut self, cache: &Path) -> Vec<(String, PathBuf)> {
        let mods: Vec<_> = self
            .loader
            .mods()
            .iter()
            .filter_map(|loaded| {
                loaded
                    .meta
                    .access_widener
                    .as_ref()
                    .map(|name| (loaded.meta.id.clone(), loaded.jar.clone(), name.clone()))
            })
            .collect();
        let mut staged = Vec::new();
        let mut hasher = Sha256::new();
        for (mod_id, jar, entry) in mods {
            match read_access_widener(&jar, &entry) {
                Ok(bytes) => {
                    hasher.update(mod_id.as_bytes());
                    hasher.update((bytes.len() as u64).to_le_bytes());
                    hasher.update(&bytes);
                    staged.push((mod_id, bytes));
                }
                Err(error) => {
                    let diagnostic = format!("could not read access widener '{entry}': {error}");
                    log::error!("J2H: mod '{}' initialization failed: {diagnostic}", mod_id);
                    self.preflight_failures.insert(mod_id, diagnostic);
                }
            }
        }
        let digest = if staged.is_empty() {
            "empty".to_owned()
        } else {
            hex::encode(hasher.finalize())
        };
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = cache
            .join("fabric")
            .join("access-wideners")
            .join(format!("{digest}-{}-{unique}", std::process::id()));
        if let Err(error) = fs::create_dir_all(&directory) {
            for (mod_id, _) in &staged {
                self.preflight_failures.insert(
                    mod_id.clone(),
                    format!("could not stage access widener data: {error}"),
                );
            }
            return Vec::new();
        }
        self.access_widener_directory = Some(directory.clone());
        let mut files = Vec::new();
        for (mod_id, bytes) in staged {
            let safe_id: String = mod_id
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || "_.-".contains(c) {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = directory.join(format!("{safe_id}.aw"));
            match fs::write(&path, bytes) {
                Ok(()) => files.push((mod_id, path)),
                Err(error) => {
                    self.preflight_failures.insert(
                        mod_id,
                        format!("could not stage access widener data: {error}"),
                    );
                }
            }
        }
        info!("J2H: staged {} access widener(s) ({digest})", files.len());
        files
    }

    fn classpath_names(&self) -> Vec<String> {
        self.loader
            .jars()
            .iter()
            .filter_map(|jar| {
                jar.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    }

    /// Discover `mods_dir`, run the pipeline and return the content roots.
    pub fn run_dir(
        &mut self,
        mods_dir: &Path,
        content_out: &Path,
    ) -> Result<Vec<PathBuf>, LifecycleError> {
        self.set_library_cache(content_out.with_file_name("java2horizon-libraries"));
        self.discover_dir(mods_dir, Some(&content_out.join("nested")))?;
        self.run_all(content_out)?;
        Ok(self.content_roots.clone())
    }
}

fn read_access_widener(mod_path: &Path, entry: &str) -> Result<Vec<u8>, String> {
    let relative = Path::new(entry);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("path must be relative and cannot contain parent components".into());
    }
    if mod_path.is_dir() {
        return fs::read(mod_path.join(relative)).map_err(|error| error.to_string());
    }
    let file = File::open(mod_path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let mut source = archive.by_name(entry).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    source
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn irreversible_java_side_effects(side: EntrypointSide) -> Vec<String> {
    let mut diagnostics = vec![
        "J2H-owned registrations were rolled back; arbitrary Java static state, Fabric event listeners, and key mappings cannot be rolled back".into(),
    ];
    if side == EntrypointSide::Client && crate::gui::is_open() {
        crate::gui::close();
        crate::gui::drain();
        diagnostics.push("closed the Java screen opened during initialization".into());
    }
    diagnostics
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_loader_is_not_a_reason_to_start_a_jvm() {
        let mut life = Lifecycle::new();
        let err = life.start_jvm().unwrap_err().to_string();
        assert!(err.contains("no Java mods"), "{err}");
    }

    /// Without a Fabric runtime there is nothing to bootstrap, and the client
    /// setup never runs for it. `bootstrap_loader` must return cleanly rather
    /// than attempt to load `MinecraftBootstrap` (client classes that are not
    /// on a server's classpath).
    #[test]
    fn bootstrap_without_fabric_runtime_skips_client_setup() {
        let mut life = Lifecycle::new();
        life.set_side(EntrypointSide::Server);
        let result = life.bootstrap_loader();
        assert!(result.is_ok(), "{}", result.unwrap_err());
    }

    /// The embedded runtime jar must carry every bootstrap class the Rust
    /// lifecycle invokes, so a stale jar fails here instead of at a confusing
    /// runtime `ClassNotFound`. Class-file method names sit inside DEFLATE
    /// streams, so this checks the archive's (uncompressed) entry names.
    #[test]
    fn embedded_runtime_jar_carries_the_bootstrap_classes() {
        let bytes = crate::J2H_RUNTIME_JAR;
        for entry in [
            "dev/horizon/j2h/MinecraftBootstrap.class",
            "dev/horizon/j2h/FabricLoaderBootstrap.class",
            "dev/horizon/j2h/J2HFabricLauncher.class",
        ] {
            assert!(jar_contains(bytes, entry), "runtime jar lacks {entry}");
        }
    }

    fn jar_contains(bytes: &[u8], needle: &str) -> bool {
        let needle = needle.as_bytes();
        bytes.windows(needle.len()).any(|window| window == needle)
    }
}
