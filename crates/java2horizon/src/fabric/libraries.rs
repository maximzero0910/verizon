//! Official Fabric Loader runtime libraries.
//!
//! Fabric mods are compiled against the loader API and the ASM/Mixin runtime
//! the launcher normally puts on the classpath. J2H resolves the same official
//! Maven artifacts instead of asking mods to bundle stubs. A mod that depends
//! on `fabricloader` triggers this; the loader itself is not initialized.

use super::{FabricError, extract_nested_jars};
use log::{info, warn};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Fabric Loader selected by `https://meta.fabricmc.net/v2/versions/loader/26.3`.
pub const FABRIC_LOADER_VERSION: &str = "0.20.1";

struct Artifact {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const ARTIFACTS: &[Artifact] = &[
    Artifact {
        file: "fabric-loader-0.20.1.jar",
        url: "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.20.1/fabric-loader-0.20.1.jar",
        sha256: "d0c5a082e07410aaff9de7ab196aa6b4236002870f9ba3ad5c1260fcce5110ad",
    },
    Artifact {
        file: "asm-9.10.1.jar",
        url: "https://repo1.maven.org/maven2/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar",
        sha256: "ed825d10ab1399c8c0cb669e688cf0c8c82629b4c8399b58352b68e92ca10fcb",
    },
    Artifact {
        file: "asm-analysis-9.10.1.jar",
        url: "https://repo1.maven.org/maven2/org/ow2/asm/asm-analysis/9.10.1/asm-analysis-9.10.1.jar",
        sha256: "dede75a21306b65974ecd8f87114ff6970f09fb794157a4ca09ab25c888c2bfc",
    },
    Artifact {
        file: "asm-commons-9.10.1.jar",
        url: "https://repo1.maven.org/maven2/org/ow2/asm/asm-commons/9.10.1/asm-commons-9.10.1.jar",
        sha256: "6d0abefb7cbf972ea16edb37ec14835372505063a45f976ab7ea889ed9497895",
    },
    Artifact {
        file: "asm-tree-9.10.1.jar",
        url: "https://repo1.maven.org/maven2/org/ow2/asm/asm-tree/9.10.1/asm-tree-9.10.1.jar",
        sha256: "3dfb0d5b6a106cd40b5b250e39935fbf2f927f4477546a5369a3ac609cf0506b",
    },
    Artifact {
        file: "asm-util-9.10.1.jar",
        url: "https://repo1.maven.org/maven2/org/ow2/asm/asm-util/9.10.1/asm-util-9.10.1.jar",
        sha256: "1bb99d091fba2597dc6d51193e9bbcf0d8447e7ed96bd8f0198b18152f09655c",
    },
    Artifact {
        file: "sponge-mixin-0.17.5+mixin.0.8.7.jar",
        url: "https://repo1.maven.org/maven2/net/fabricmc/sponge-mixin/0.17.5+mixin.0.8.7/sponge-mixin-0.17.5+mixin.0.8.7.jar",
        sha256: "03f0065023bfc3045d71bfb99872451d9ad5a0800a5a0dac34ab8c7d5c134f5d",
    },
];

/// Return the official loader libraries for `cache`, downloading missing jars.
///
/// `HORIZON_FABRIC_LIBS_DIR` replaces the download with a folder of jars, for
/// offline runs and tests. Classpath entries already supplied by the caller
/// (for example a loader jar in `mods/`) are not added twice.
pub fn ensure_libraries(
    cache: &Path,
    already_on_classpath: &[String],
) -> Result<Vec<PathBuf>, FabricError> {
    if let Some(dir) = std::env::var_os("HORIZON_FABRIC_LIBS_DIR") {
        let mut jars = libraries_from_dir(Path::new(&dir), already_on_classpath)?;
        let nested = cache.join("fabric").join("nested");
        let mut nested_jars = Vec::new();
        for jar in &jars {
            match extract_nested_jars(jar, &nested) {
                Ok(found) => nested_jars.extend(found),
                Err(e) => warn!("J2H: {} nested jars: {e}", jar.display()),
            }
        }
        jars.extend(nested_jars);
        return Ok(jars);
    }

    let dir = cache.join("fabric").join(FABRIC_LOADER_VERSION);
    fs::create_dir_all(&dir)?;
    let mut out = Vec::new();
    for artifact in ARTIFACTS {
        if already_on_classpath
            .iter()
            .any(|name| name == artifact.file)
        {
            continue;
        }
        let target = dir.join(artifact.file);
        if !matches_sha256(&target, artifact.sha256) {
            download(artifact, &target)?;
        }
        out.push(target);
    }

    let nested = cache.join("fabric").join("nested");
    let mut nested_jars = Vec::new();
    for jar in &out {
        match extract_nested_jars(jar, &nested) {
            Ok(jars) => nested_jars.extend(jars),
            Err(e) => warn!("J2H: {} nested jars: {e}", jar.display()),
        }
    }
    out.extend(nested_jars);
    Ok(out)
}

fn libraries_from_dir(
    dir: &Path,
    already_on_classpath: &[String],
) -> Result<Vec<PathBuf>, FabricError> {
    let mut jars = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "jar") {
            jars.push(path);
        }
    }
    jars.sort();
    jars.retain(|jar| {
        jar.file_name()
            .map(|name| {
                !already_on_classpath
                    .iter()
                    .any(|seen| seen == &name.to_string_lossy())
            })
            .unwrap_or(true)
    });
    if jars.is_empty() {
        return Err(FabricError::Other(format!(
            "HORIZON_FABRIC_LIBS_DIR={} has no jars",
            dir.display()
        )));
    }
    Ok(jars)
}

fn download(artifact: &Artifact, target: &Path) -> Result<(), FabricError> {
    info!("J2H: downloading Fabric Loader library {}", artifact.file);
    let response = ureq::get(artifact.url)
        .call()
        .map_err(|e| FabricError::Other(format!("{}: {e}", artifact.url)))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| FabricError::Other(format!("{}: {e}", artifact.url)))?;
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != artifact.sha256 {
        return Err(FabricError::Other(format!(
            "{}: sha256 {actual}, expected {}",
            artifact.url, artifact.sha256
        )));
    }
    let temp = target.with_extension("jar.tmp");
    fs::write(&temp, &bytes)?;
    fs::rename(&temp, target)?;
    Ok(())
}

fn matches_sha256(path: &Path, expected: &str) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    hex::encode(hasher.finalize()) == expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_folder_is_used_and_already_seen_jars_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("fabric-loader-0.20.1.jar"), b"loader").unwrap();
        fs::write(dir.path().join("asm-9.10.1.jar"), b"asm").unwrap();
        fs::write(dir.path().join("notes.txt"), b"ignored").unwrap();

        let jars = libraries_from_dir(dir.path(), &["asm-9.10.1.jar".into()]).unwrap();
        assert_eq!(jars.len(), 1);
        assert!(jars[0].ends_with("fabric-loader-0.20.1.jar"));
    }

    #[test]
    fn empty_offline_folder_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = libraries_from_dir(dir.path(), &[]).unwrap_err().to_string();
        assert!(err.contains("has no jars"), "{err}");
    }
}
