//! Java2Horizon (J2H): run Java/Fabric mods against Horizon's data-driven
//! content system.
//!
//! The loader reads `fabric.mod.json`, resolves dependencies, starts an
//! embedded JVM, invokes Fabric entrypoints and turns the registrations they
//! make through the native bridge into a Horizon content-mod tree. The tree
//! is validated by the real `horizon-mods` crate before the game loads it.
//!
//! It is not Fabric Knot: supported official-namespace access wideners and
//! Fabric Mixin configs are applied through JVM instrumentation, while
//! Minecraft mappings and many Fabric API behaviors remain incomplete.

pub mod bridge;
pub mod fabric;
pub mod gui;
pub mod horizon_shim;
pub mod jvm;
pub mod lifecycle;
pub mod minecraft;

pub use bridge::{Bridge, BridgeError, NativeCallback};
pub use fabric::{
    EntrypointSide, FabricEntrypoint, FabricError, FabricLoader, FabricModMeta, load_mod_meta,
    load_single,
};
pub use gui::GuiCommand;
pub use horizon_shim::{EngineEvent, HorizonEngine, HorizonError};
pub use jvm::{JvmConfig, JvmError, JvmRuntime};
pub use lifecycle::{Lifecycle, LifecycleError, Stage};
pub use minecraft::{MinecraftError, Resolver as MinecraftLibraries};

/// J2H's own Java runtime helpers (see `bridge-java/` and
/// `build_runtime.sh`): the FabricLauncher stub and the loader bootstrap that
/// feeds discovered mods into the real Fabric Loader singleton. Embedded here
/// and extracted next to the JVM classpath at startup, so the game needs no
/// Java preparation step.
pub const J2H_RUNTIME_JAR: &[u8] = include_bytes!("../test_resources/j2h-runtime.jar");

/// Minimal JVM instrumentation agent for supported official-namespace access wideners.
pub const J2H_ACCESS_WIDENER_AGENT_JAR: &[u8] =
    include_bytes!("../test_resources/j2h-access-widener-agent.jar");
