//! Resolve, verify and cache Minecraft's published runtime libraries.

use super::MinecraftError;
use super::metadata::{
    ResolvedLibrary, RuleContext, VERSION_MANIFEST_URL, VersionManifest, VersionMeta, VersionRef,
};
use horizon_core::version::VERSION_ID;
use log::{info, warn};
use sha1::{Digest, Sha1};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const USER_AGENT: &str = concat!("Horizon-J2H/", env!("CARGO_PKG_VERSION"));

/// A Minecraft runtime resolver with explicit offline inputs.
#[derive(Debug, Clone)]
pub struct Resolver {
    pub cache: PathBuf,
    /// Directory with `version_manifest_v2.json` and `<version>.json`.
    pub metadata_dir: Option<PathBuf>,
    /// Directory containing the jars named by the version metadata.
    pub libraries_dir: Option<PathBuf>,
    /// Do not use the network for metadata or artifacts.
    pub offline: bool,
}

impl Resolver {
    pub fn new(cache: impl Into<PathBuf>) -> Self {
        Self {
            cache: cache.into(),
            metadata_dir: None,
            libraries_dir: None,
            offline: false,
        }
    }

    pub fn from_environment(cache: impl Into<PathBuf>) -> Self {
        Self {
            cache: cache.into(),
            metadata_dir: std::env::var_os("HORIZON_MINECRAFT_META_DIR").map(PathBuf::from),
            libraries_dir: std::env::var_os("HORIZON_MINECRAFT_LIBS_DIR").map(PathBuf::from),
            offline: std::env::var_os("HORIZON_MINECRAFT_OFFLINE").is_some(),
        }
    }

    /// Resolve Minecraft 26.3's launchable client jar and its libraries, in
    /// the launcher's classpath order: the client jar first, then the
    /// libraries.
    ///
    /// Jars whose file name is already on the classpath are skipped, so
    /// Fabric/loader libraries and mod-provided jars are never duplicated.
    pub fn ensure(&self, already_on_classpath: &[String]) -> Result<Vec<PathBuf>, MinecraftError> {
        let manifest = self.load_manifest()?;
        let version = manifest.find(VERSION_ID).ok_or_else(|| {
            MinecraftError::MissingVersion(format!(
                "version manifest has no entry for Minecraft {VERSION_ID}"
            ))
        })?;
        let meta = self.load_version(version)?;

        let cache_root = self.cache.join("minecraft").join(VERSION_ID);
        let mut out = Vec::new();
        if let Some(client) = meta.client_download(VERSION_ID) {
            if !already_on_classpath
                .iter()
                .any(|seen| seen == client.file_name())
            {
                let path = self.ensure_artifact(&cache_root, &client, "client.jar")?;
                out.push(path);
            }
        }

        let libraries = skip_already(
            meta.resolve_libraries(&RuleContext::current()),
            already_on_classpath,
        );
        info!(
            "J2H: Minecraft {} metadata selects {} runtime libraries",
            VERSION_ID,
            libraries.len()
        );
        for library in libraries {
            let path = self.ensure_artifact(&cache_root, &library, "")?;
            out.push(path);
        }
        Ok(out)
    }

    /// One artifact: from `libraries_dir` (falling back to `alt_name`), or
    /// cached/downloaded under `cache_root`, verified for size and SHA-1.
    fn ensure_artifact(
        &self,
        cache_root: &Path,
        library: &ResolvedLibrary,
        alt_name: &str,
    ) -> Result<PathBuf, MinecraftError> {
        if let Some(dir) = &self.libraries_dir {
            let path = match prepopulated_path(dir, library.file_name()) {
                Ok(path) => path,
                Err(_) if !alt_name.is_empty() && alt_name != library.file_name() => {
                    prepopulated_path(dir, alt_name).map_err(|_| {
                        MinecraftError::MissingArtifact(format!(
                            "{}/{} (searched {})",
                            library.file_name(),
                            alt_name,
                            dir.display()
                        ))
                    })?
                }
                Err(e) => return Err(e),
            };
            verify_file(&path, library)?;
            Ok(path)
        } else {
            ensure_cached_or_download(cache_root, library, self.offline, &fetch_url)
        }
    }

    fn load_manifest(&self) -> Result<VersionManifest, MinecraftError> {
        if let Some(dir) = &self.metadata_dir {
            let path = dir.join("version_manifest_v2.json");
            let bytes = fs::read(&path)
                .map_err(|e| MinecraftError::Metadata(format!("{}: {e}", path.display())))?;
            return serde_json::from_slice(&bytes).map_err(MinecraftError::from);
        }

        let path = self
            .cache
            .join("minecraft")
            .join("version_manifest_v2.json");
        if path.exists() {
            match serde_json::from_slice::<VersionManifest>(&fs::read(&path)?) {
                Ok(manifest) => return Ok(manifest),
                Err(e) if self.offline => {
                    return Err(MinecraftError::Metadata(format!("{}: {e}", path.display())));
                }
                Err(e) => warn!("J2H: {}: {e}; refreshing", path.display()),
            }
        }
        if self.offline {
            return Err(MinecraftError::Offline(format!(
                "no cached version manifest at {}",
                path.display()
            )));
        }
        let bytes = fetch_url(VERSION_MANIFEST_URL)?;
        write_atomic(&path, &bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn load_version(&self, version: &VersionRef) -> Result<VersionMeta, MinecraftError> {
        if let Some(dir) = &self.metadata_dir {
            let path = dir.join(format!("{}.json", version.id));
            let bytes = fs::read(&path)
                .map_err(|e| MinecraftError::Metadata(format!("{}: {e}", path.display())))?;
            verify_version_json(&path, &bytes, version)?;
            return serde_json::from_slice(&bytes).map_err(MinecraftError::from);
        }

        let path = self
            .cache
            .join("minecraft")
            .join(format!("{}.json", version.id));
        if path.exists() {
            let bytes = fs::read(&path)?;
            match verify_version_json(&path, &bytes, version) {
                Ok(()) => return serde_json::from_slice(&bytes).map_err(MinecraftError::from),
                Err(e) if self.offline => return Err(e),
                Err(e) => warn!("J2H: {e}; refreshing"),
            }
        }
        if self.offline {
            return Err(MinecraftError::Offline(format!(
                "no cached version JSON at {}",
                path.display()
            )));
        }
        let bytes = fetch_url(&version.url)?;
        verify_version_json(&path, &bytes, version)?;
        write_atomic(&path, &bytes)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

fn verify_version_json(
    path: &Path,
    bytes: &[u8],
    version: &VersionRef,
) -> Result<(), MinecraftError> {
    let Some(expected) = &version.sha1 else {
        return Ok(());
    };
    let actual = sha1_hex(bytes);
    if &actual != expected {
        return Err(MinecraftError::HashMismatch {
            path: path.display().to_string(),
            expected: expected.clone(),
            actual,
        });
    }
    Ok(())
}

fn skip_already(
    libraries: Vec<ResolvedLibrary>,
    already_on_classpath: &[String],
) -> Vec<ResolvedLibrary> {
    let mut seen = Vec::new();
    libraries
        .into_iter()
        .filter(|library| {
            let name = library.file_name();
            if already_on_classpath.iter().any(|seen| seen == name) {
                return false;
            }
            if seen.iter().any(|seen: &String| seen == name) {
                return false;
            }
            seen.push(name.to_owned());
            true
        })
        .collect()
}

fn ensure_cached_or_download(
    cache_root: &Path,
    library: &ResolvedLibrary,
    offline: bool,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, MinecraftError>,
) -> Result<PathBuf, MinecraftError> {
    let target = cache_root.join(&library.path);
    if file_valid(&target, library)? {
        return Ok(target);
    }
    if offline {
        return Err(MinecraftError::Offline(format!(
            "missing or invalid cached artifact {}",
            target.display()
        )));
    }
    info!(
        "J2H: downloading Minecraft library {} ({})",
        library.name,
        library.file_name()
    );
    let bytes = fetch(&library.url)?;
    verify_bytes(&target.display().to_string(), &bytes, library)?;
    write_atomic(&target, &bytes)?;
    Ok(target)
}

fn prepopulated_path(dir: &Path, file_name: &str) -> Result<PathBuf, MinecraftError> {
    find_file(dir, file_name).ok_or_else(|| {
        MinecraftError::MissingArtifact(format!("{} (searched {})", file_name, dir.display()))
    })
}

fn find_file(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let mut queue = VecDeque::from([(dir.to_owned(), 0usize)]);
    while let Some((current, depth)) = queue.pop_front() {
        if depth > 16 {
            continue;
        }
        let Ok(read) = fs::read_dir(&current) else {
            continue;
        };
        let mut entries: Vec<_> = read.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for path in entries {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                queue.push_back((path, depth + 1));
            } else if metadata.is_file()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy() == file_name)
            {
                return Some(path);
            }
        }
    }
    None
}

fn file_valid(path: &Path, library: &ResolvedLibrary) -> Result<bool, MinecraftError> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = fs::metadata(path)?;
    if let Some(expected) = library.size {
        if metadata.len() != expected {
            return Ok(false);
        }
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) => return Err(MinecraftError::Io(e)),
        }
    }
    Ok(hex::encode(hasher.finalize()) == library.sha1)
}

fn verify_file(path: &Path, library: &ResolvedLibrary) -> Result<(), MinecraftError> {
    if !path.exists() {
        return Err(MinecraftError::MissingArtifact(path.display().to_string()));
    }
    let metadata = fs::metadata(path)?;
    if let Some(expected) = library.size {
        if metadata.len() != expected {
            return Err(MinecraftError::SizeMismatch {
                path: path.display().to_string(),
                expected,
                actual: metadata.len(),
            });
        }
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buffer[..n]),
            Err(e) => return Err(MinecraftError::Io(e)),
        }
    }
    let actual = hex::encode(hasher.finalize());
    if actual != library.sha1 {
        return Err(MinecraftError::HashMismatch {
            path: path.display().to_string(),
            expected: library.sha1.clone(),
            actual,
        });
    }
    Ok(())
}

fn verify_bytes(path: &str, bytes: &[u8], library: &ResolvedLibrary) -> Result<(), MinecraftError> {
    if let Some(expected) = library.size {
        if bytes.len() as u64 != expected {
            return Err(MinecraftError::SizeMismatch {
                path: path.into(),
                expected,
                actual: bytes.len() as u64,
            });
        }
    }
    let actual = sha1_hex(bytes);
    if actual != library.sha1 {
        return Err(MinecraftError::HashMismatch {
            path: path.into(),
            expected: library.sha1.clone(),
            actual,
        });
    }
    Ok(())
}

fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

fn fetch_url(url: &str) -> Result<Vec<u8>, MinecraftError> {
    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| MinecraftError::Http(format!("{url}: {e}")))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| MinecraftError::Http(format!("{url}: {e}")))?;
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MinecraftError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "artifact".into());
    let temp = path.with_file_name(format!("{file_name}.part"));
    {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn library(path: &str, bytes: &[u8], size: Option<u64>) -> ResolvedLibrary {
        ResolvedLibrary {
            name: format!("test:{path}"),
            path: path.into(),
            url: format!("https://example/{path}"),
            sha1: sha1_hex(bytes),
            size,
        }
    }

    #[test]
    fn sha1_verification_accepts_and_rejects() {
        let lib = library("a.jar", b"hello", Some(5));
        verify_bytes("a.jar", b"hello", &lib).unwrap();
        let err = verify_bytes("a.jar", b"HELLO", &lib).unwrap_err();
        assert!(err.to_string().contains("sha1"), "{err}");
    }

    #[test]
    fn cache_is_reused_then_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library("a/b.jar", b"good", Some(4));
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(b"good".to_vec())
        };

        let path = ensure_cached_or_download(dir.path(), &lib, false, &fetch).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"good");

        ensure_cached_or_download(dir.path(), &lib, false, &fetch).unwrap();
        assert_eq!(
            calls.get(),
            1,
            "a valid cache entry must not be fetched again"
        );

        fs::write(&path, b"bad!").unwrap();
        ensure_cached_or_download(dir.path(), &lib, false, &fetch).unwrap();
        assert_eq!(calls.get(), 2, "a corrupted cache entry must be replaced");
        assert_eq!(fs::read(&path).unwrap(), b"good");
    }

    #[test]
    fn invalid_download_is_rejected_and_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library("a/b.jar", b"good", Some(4));
        let fetch = |_: &str| Ok(b"evil".to_vec());
        let err = ensure_cached_or_download(dir.path(), &lib, false, &fetch).unwrap_err();
        assert!(err.to_string().contains("sha1"), "{err}");
        assert!(!dir.path().join("a/b.jar").exists());
    }

    #[test]
    fn stale_partial_download_does_not_count_as_valid() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library("a/b.jar", b"good", Some(4));
        fs::create_dir_all(dir.path().join("a")).unwrap();
        fs::write(dir.path().join("a/b.jar.part"), b"partial").unwrap();
        let fetch = |_: &str| Ok(b"good".to_vec());
        let path = ensure_cached_or_download(dir.path(), &lib, false, &fetch).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"good");
        assert!(!dir.path().join("a/b.jar.part").exists());
    }

    #[test]
    fn offline_missing_artifact_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library("a/b.jar", b"good", Some(4));
        let fetch = |_: &str| panic!("offline must not fetch");
        let err = ensure_cached_or_download(dir.path(), &lib, true, &fetch).unwrap_err();
        assert!(err.to_string().contains("offline"), "{err}");
    }

    #[test]
    fn prepopulated_tree_is_searched_and_verified() {
        let dir = tempfile::tempdir().unwrap();
        let lib = library("a/b.jar", b"good", Some(4));
        let path = dir.path().join("com/example/a/b.jar");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"good").unwrap();

        assert_eq!(find_file(dir.path(), "b.jar").unwrap(), path);
        verify_file(&path, &lib).unwrap();

        fs::write(&path, b"bad!").unwrap();
        let err = verify_file(&path, &lib).unwrap_err();
        assert!(err.to_string().contains("sha1"), "{err}");
    }

    #[test]
    fn already_on_classpath_is_skipped_by_file_name() {
        let libs = vec![library("a/b.jar", b"good", Some(4))];
        let filtered = skip_already(libs, &["b.jar".into()]);
        assert!(filtered.is_empty());
    }

    #[test]
    fn duplicate_file_names_are_collapsed() {
        let libs = vec![
            library("a/b.jar", b"good", Some(4)),
            library("c/b.jar", b"good", Some(4)),
        ];
        let filtered = skip_already(libs, &[]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "a/b.jar");
    }

    #[test]
    fn offline_metadata_and_libraries_resolve_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let meta_dir = dir.path().join("meta");
        let lib_dir = dir.path().join("libs");
        fs::create_dir_all(&meta_dir).unwrap();
        fs::create_dir_all(lib_dir.join("com/example")).unwrap();
        let jar = b"good";
        let jar_path = lib_dir.join("com/example/a-1.0.jar");
        fs::write(&jar_path, jar).unwrap();
        let jar_sha1 = sha1_hex(jar);
        let version_json = format!(
            r#"{{"id":"{VERSION_ID}","libraries":[{{"name":"com.example:a:1.0","downloads":{{"artifact":{{"path":"com/example/a-1.0.jar","sha1":"{jar_sha1}","size":4,"url":"https://example/a-1.0.jar"}}}}}}]}}"#
        );
        let version_sha1 = sha1_hex(version_json.as_bytes());
        fs::write(meta_dir.join(format!("{VERSION_ID}.json")), &version_json).unwrap();
        fs::write(
            meta_dir.join("version_manifest_v2.json"),
            format!(
                r#"{{"versions":[{{"id":"{VERSION_ID}","url":"https://example/{VERSION_ID}.json","sha1":"{version_sha1}"}}]}}"#
            ),
        )
        .unwrap();

        let resolver = Resolver {
            cache: dir.path().join("cache"),
            metadata_dir: Some(meta_dir),
            libraries_dir: Some(lib_dir),
            offline: true,
        };
        let resolved = resolver.ensure(&[]).unwrap();
        assert_eq!(resolved, vec![jar_path]);
    }

    #[test]
    fn offline_without_cached_metadata_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = Resolver {
            cache: dir.path().to_owned(),
            metadata_dir: None,
            libraries_dir: None,
            offline: true,
        };
        let err = resolver.ensure(&[]).unwrap_err();
        assert!(err.to_string().contains("offline"), "{err}");
    }

    fn version_meta_with_client(client_sha1: &str, client_size: u64, library_sha1: &str) -> String {
        format!(
            r#"{{"id":"{VERSION_ID}","downloads":{{"client":{{"sha1":"{client_sha1}","size":{client_size},"url":"https://example/client.jar"}}}},"libraries":[{{"name":"com.example:a:1.0","downloads":{{"artifact":{{"path":"com/example/a-1.0.jar","sha1":"{library_sha1}","size":4,"url":"https://example/a-1.0.jar"}}}}}}]}}"#
        )
    }

    fn offline_resolver(meta_dir: &Path, lib_dir: &Path, cache: &Path) -> Resolver {
        Resolver {
            cache: cache.to_owned(),
            metadata_dir: Some(meta_dir.to_owned()),
            libraries_dir: Some(lib_dir.to_owned()),
            offline: true,
        }
    }

    fn write_meta(meta_dir: &Path, version_json: &str) {
        fs::create_dir_all(meta_dir).unwrap();
        let version_sha1 = sha1_hex(version_json.as_bytes());
        fs::write(meta_dir.join(format!("{VERSION_ID}.json")), version_json).unwrap();
        fs::write(
            meta_dir.join("version_manifest_v2.json"),
            format!(
                r#"{{"versions":[{{"id":"{VERSION_ID}","url":"https://example/{VERSION_ID}.json","sha1":"{version_sha1}"}}]}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn offline_client_jar_resolves_first_then_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let meta_dir = dir.path().join("meta");
        let lib_dir = dir.path().join("libs");
        let client = b"minecraft-client";
        let lib = b"lib!";
        write_meta(
            &meta_dir,
            &version_meta_with_client(&sha1_hex(client), 16, &sha1_hex(lib)),
        );
        fs::create_dir_all(lib_dir.join("com/example")).unwrap();
        let client_path = lib_dir.join("client-26.3.jar");
        let lib_path = lib_dir.join("com/example/a-1.0.jar");
        fs::write(&client_path, client).unwrap();
        fs::write(&lib_path, lib).unwrap();

        let resolver = offline_resolver(&meta_dir, &lib_dir, &dir.path().join("cache"));
        let resolved = resolver.ensure(&[]).unwrap();
        assert_eq!(resolved, vec![client_path, lib_path]);
    }

    #[test]
    fn offline_client_jar_accepts_mojangs_client_jar_name() {
        let dir = tempfile::tempdir().unwrap();
        let meta_dir = dir.path().join("meta");
        let lib_dir = dir.path().join("libs");
        let client = b"minecraft-client";
        write_meta(
            &meta_dir,
            &version_meta_with_client(&sha1_hex(client), 16, &sha1_hex(b"lib!")),
        );
        fs::create_dir_all(lib_dir.join("com/example")).unwrap();
        fs::write(lib_dir.join("client.jar"), client).unwrap();
        fs::write(lib_dir.join("com/example/a-1.0.jar"), b"lib!").unwrap();

        let resolver = offline_resolver(&meta_dir, &lib_dir, &dir.path().join("cache"));
        let resolved = resolver.ensure(&[]).unwrap();
        assert_eq!(resolved[0], lib_dir.join("client.jar"));
    }

    #[test]
    fn offline_wrong_client_jar_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let meta_dir = dir.path().join("meta");
        let lib_dir = dir.path().join("libs");
        write_meta(
            &meta_dir,
            &version_meta_with_client(&sha1_hex(b"minecraft-client"), 16, &sha1_hex(b"lib")),
        );
        fs::create_dir_all(&lib_dir).unwrap();
        fs::write(lib_dir.join("client-26.3.jar"), b"XXXXXXXXXXXXXXXX").unwrap();

        let resolver = offline_resolver(&meta_dir, &lib_dir, &dir.path().join("cache"));
        let err = resolver.ensure(&[]).unwrap_err();
        assert!(err.to_string().contains("sha1"), "{err}");
    }

    #[test]
    fn offline_missing_client_jar_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let meta_dir = dir.path().join("meta");
        let lib_dir = dir.path().join("libs");
        write_meta(
            &meta_dir,
            &version_meta_with_client(&sha1_hex(b"minecraft-client"), 16, &sha1_hex(b"lib")),
        );
        fs::create_dir_all(lib_dir.join("com/example")).unwrap();
        fs::write(lib_dir.join("com/example/a-1.0.jar"), b"lib").unwrap();

        let resolver = offline_resolver(&meta_dir, &lib_dir, &dir.path().join("cache"));
        let err = resolver.ensure(&[]).unwrap_err();
        assert!(err.to_string().contains("client"), "{err}");
    }

    #[test]
    fn client_jar_download_caches_and_reuses_like_a_library() {
        let dir = tempfile::tempdir().unwrap();
        let meta: VersionMeta = serde_json::from_str(&version_meta_with_client(
            &sha1_hex(b"minecraft-client"),
            16,
            &sha1_hex(b"lib"),
        ))
        .unwrap();
        let client = meta.client_download(VERSION_ID).unwrap();
        assert_eq!(client.file_name(), "client-26.3.jar");

        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(b"minecraft-client".to_vec())
        };
        let path = ensure_cached_or_download(dir.path(), &client, false, &fetch).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"minecraft-client");

        ensure_cached_or_download(dir.path(), &client, false, &fetch).unwrap();
        assert_eq!(calls.get(), 1, "a valid cached client jar must not refetch");
    }

    #[test]
    fn real_26_3_metadata_exposes_the_client_jar() {
        // Guarded by the local Mojang metadata (fetched by the resolver on a
        // real run); the test is skipped outside this checkout's test cache.
        let meta_file = Path::new("/tmp/j2h-e2e/mcmeta/26.3.json");
        if !meta_file.is_file() {
            return;
        }
        let meta: VersionMeta = serde_json::from_slice(&fs::read(meta_file).unwrap()).unwrap();
        let client = meta.client_download("26.3").unwrap();
        assert_eq!(client.file_name(), "client-26.3.jar");
        assert_eq!(client.sha1.len(), 40);
        assert!(client.size.is_some_and(|s| s > 41_000_000));
        assert!(client.url.starts_with("https://piston-data.mojang.com/"));
    }
}
