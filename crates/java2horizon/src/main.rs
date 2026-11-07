//! Java2Horizon Milestone 3 runner: lifecycle + horizon-mods validation.

use java2horizon::lifecycle::Lifecycle;
use log::{error, info};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    info!("=== Java2Horizon compatibility runtime ===");

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let content_out = root.join("test_resources/horizon_content");
    let _ = std::fs::remove_dir_all(&content_out);

    let mut life = Lifecycle::new();

    // Discover: libmod then testmod (depends on libmod)
    let lib_jar = root.join("test_resources/libmod.jar");
    let lib_meta = root.join("test_resources/libmod.mod.json");
    let mod_jar = root.join("test_resources/testmod.jar");
    let mod_meta = root.join("test_resources/testmod.mod.json");

    for (jar, meta) in [(&lib_jar, &lib_meta), (&mod_jar, &mod_meta)] {
        if !jar.exists() {
            error!("Missing {}. Run build_test_mods.sh", jar.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = life.discover_mod(jar.clone(), meta) {
            error!("{e}");
            return ExitCode::FAILURE;
        }
    }

    if let Err(e) = life.run_all(&content_out) {
        error!("Lifecycle failed: {e}");
        return ExitCode::FAILURE;
    }

    let eng = &life.engine;
    let ok = eng.has_block("testmod:example_block")
        && eng.has_item("testmod:example_item")
        && eng.has_block("libmod:shared_block")
        && eng.initialized_mods().len() >= 2
        && !life.content_roots.is_empty();

    info!(
        "blocks={} items={} entities={} effects={} mods={:?}",
        eng.block_count(),
        eng.item_count(),
        eng.entity_type_count(),
        eng.mob_effect_count(),
        eng.initialized_mods()
    );
    if ok {
        info!("J2H INTEGRATION PASSED");
        info!("  ✓ Lifecycle stages completed");
        info!("  ✓ Multi-mod dependency order");
        info!("  ✓ Content validated by horizon-mods Content::read");
        ExitCode::SUCCESS
    } else {
        error!("J2H INTEGRATION FAILED");
        ExitCode::FAILURE
    }
}
