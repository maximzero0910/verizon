//! Java/Fabric mods in the game folder's `mods/` directory.
//!
//! Java jars are not content mods: their code has to run before Horizon
//! builds its resource root. This module runs the Java2Horizon lifecycle
//! once, in `mods/`, and hands the generated content-mod roots to
//! `mods::content_root`, which overlays them like any other content mod.
//!
//! The JVM is process-wide and expensive, so a session starts it once. Mods
//! added after the world is open need a restart, as content mods do.

use java2horizon::{EntrypointSide, Lifecycle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder a Java instance keeps its mods in.
pub(super) const JAVA_MODS_DIR: &str = "mods";

struct JavaContent {
    game_dir: PathBuf,
    roots: Vec<PathBuf>,
}

static CONTENT: OnceLock<JavaContent> = OnceLock::new();

/// The content-mod roots generated from the Java mods in `game_dir/mods`.
/// Empty when there are no Java mods or when the runtime is disabled.
pub(super) fn content_roots(
    game_dir: &Path,
    resources: &Path,
    cache: &Path,
    dedicated: bool,
    benchmark: bool,
    player_name: &str,
) -> Vec<PathBuf> {
    if !runtime_enabled(benchmark) {
        return Vec::new();
    }
    let dir = java_mods_dir(game_dir);
    if !has_java_mods(&dir) {
        return Vec::new();
    }
    let content = CONTENT.get_or_init(|| JavaContent {
        game_dir: game_dir.to_owned(),
        roots: run(&dir, resources, cache, dedicated, player_name),
    });
    if content.game_dir != game_dir {
        log::warn!(
            "Horizon: Java mods were loaded for {}; {} needs a restart",
            content.game_dir.display(),
            game_dir.display()
        );
    }
    content.roots.clone()
}

pub(super) fn runtime_enabled(benchmark: bool) -> bool {
    !benchmark && std::env::var_os("HORIZON_NO_JAVA_MODS").is_none()
}

pub(super) fn java_mods_dir(game_dir: &Path) -> PathBuf {
    std::env::var_os("HORIZON_JAVA_MODS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| game_dir.join(JAVA_MODS_DIR))
}

fn has_java_mods(dir: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    read.flatten().any(|entry| {
        let path = entry.path();
        path.extension().is_some_and(|e| e == "jar") || path.join("fabric.mod.json").is_file()
    })
}

fn run(
    dir: &Path,
    resources: &Path,
    cache: &Path,
    dedicated: bool,
    player_name: &str,
) -> Vec<PathBuf> {
    let out = cache.join("java2horizon");
    // A restart regenerates the tree; a stale namespace must not survive.
    match std::fs::remove_dir_all(&out) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log::warn!("Horizon: Java mod cache {}: {e}", out.display());
            return Vec::new();
        }
    }
    let nested = out.join("nested");
    let mut life = Lifecycle::new();
    life.set_library_cache(cache.join("java2horizon-libraries"));
    life.set_resource_root(resources.to_owned());
    life.set_mods_dir(dir.to_owned());
    if let Some(parent) = cache.parent() {
        life.set_game_dir(parent.to_owned());
    }
    life.set_side(if dedicated {
        EntrypointSide::Server
    } else {
        EntrypointSide::Client
    });
    let uuid = horizon_client::auth::current()
        .filter(|account| account.name == player_name)
        .map_or_else(
            || horizon_protocol::crypto::offline_uuid(player_name),
            |account| account.uuid,
        );
    life.set_client_identity(
        player_name.to_owned(),
        horizon_protocol::crypto::uuid_string(uuid),
    );
    match life.discover_dir(dir, Some(&nested)) {
        Ok(0) => return Vec::new(),
        Ok(n) => log::info!("Horizon: {n} Java mod(s) in {}", dir.display()),
        Err(e) => {
            log::warn!("Horizon: Java mod discovery failed: {e}");
            return Vec::new();
        }
    }
    if let Err(e) = life.run_all(&out) {
        log::warn!("Horizon: Java mods not loaded: {e}");
        return Vec::new();
    }
    let roots = life.content_roots.clone();
    if !roots.is_empty() {
        log::info!(
            "Horizon: Java mods registered {} block(s), {} item(s), {} entity type(s), {} effect(s)",
            life.engine.block_count(),
            life.engine.item_count(),
            life.engine.entity_type_count(),
            life.engine.mob_effect_count()
        );
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_mods_folder_has_no_java_mods() {
        assert!(!has_java_mods(Path::new("/definitely/not/a/mods/folder")));
    }

    #[test]
    fn a_jar_is_a_java_mod() {
        let dir = std::env::temp_dir().join(format!("j2h-java-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("example.jar"), b"not a jar").unwrap();
        assert!(has_java_mods(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
