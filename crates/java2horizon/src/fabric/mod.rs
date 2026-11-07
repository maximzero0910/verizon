//! Fabric metadata and entrypoint discovery.
//!
//! This is a loader bootstrap, not the Fabric Loader: it reads
//! `fabric.mod.json` (from a jar, a folder or a sidecar), resolves the
//! dependency graph and invokes entrypoints. The embedded Fabric Loader and
//! Mixin service provide metadata-backed mixin configs; nested jars are
//! unpacked so library jars inside a mod are visible.

use log::{info, warn};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::jvm::{JvmError, JvmRuntime};

pub mod libraries;

#[derive(Error, Debug)]
pub enum FabricError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing fabric.mod.json")]
    MissingMeta,
    #[error("JVM: {0}")]
    Jvm(#[from] JvmError),
    #[error("{0}")]
    Other(String),
}

/// Which Fabric entrypoint set a run is on. A client runs `main` and
/// `client`; a dedicated server runs `main` and `server`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrypointSide {
    Client,
    Server,
}

impl EntrypointSide {
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Client => &["main", "client"],
            Self::Server => &["main", "server"],
        }
    }

    /// Fabric's `environment`: `*` (or absent) runs on both, `client` and
    /// `server` only on that side.
    pub fn accepts(self, environment: Option<&str>) -> bool {
        match environment {
            Some("client") => self == Self::Client,
            Some("server") => self == Self::Server,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FabricModMeta {
    #[serde(default, rename = "schemaVersion")]
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub entrypoints: HashMap<String, Vec<EntrypointEntry>>,
    #[serde(default)]
    pub depends: HashMap<String, VersionRequirement>,
    #[serde(default)]
    pub recommends: HashMap<String, VersionRequirement>,
    #[serde(default)]
    pub suggests: HashMap<String, VersionRequirement>,
    #[serde(default)]
    pub breaks: HashMap<String, VersionRequirement>,
    #[serde(default)]
    pub conflicts: HashMap<String, VersionRequirement>,
    /// Ids this mod makes available to other mods (Fabric `provides`).
    #[serde(default)]
    pub provides: Vec<String>,
    /// Mixin configurations: strings or objects. Recognised, not applied.
    #[serde(default)]
    pub mixins: Value,
    #[serde(default, rename = "accessWidener")]
    pub access_widener: Option<String>,
    /// Nested jars under `META-INF/jars/`.
    #[serde(default)]
    pub jars: Vec<NestedJar>,
}

/// A Fabric dependency accepts one version predicate or any of several.
/// Fabric metadata uses an array to express alternatives, as in
/// `"sodium": ["0.9.x", "0.10.x"]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum VersionRequirement {
    Single(String),
    Any(Vec<String>),
}

impl VersionRequirement {
    fn accepts(&self, version: &str) -> bool {
        match self {
            Self::Single(requirement) => version_satisfies(version, requirement),
            Self::Any(requirements) => requirements
                .iter()
                .any(|requirement| version_satisfies(version, requirement)),
        }
    }
}

impl From<&str> for VersionRequirement {
    fn from(value: &str) -> Self {
        Self::Single(value.to_owned())
    }
}

impl From<String> for VersionRequirement {
    fn from(value: String) -> Self {
        Self::Single(value)
    }
}

impl fmt::Display for VersionRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(requirement) => f.write_str(requirement),
            Self::Any(requirements) => write!(f, "any of [{}]", requirements.join(", ")),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct NestedJar {
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EntrypointEntry {
    Simple(String),
    Object {
        value: String,
        #[serde(default)]
        adapter: Option<String>,
        #[serde(default)]
        language: Option<String>,
    },
}

impl EntrypointEntry {
    pub fn value(&self) -> &str {
        match self {
            EntrypointEntry::Simple(s) => s,
            EntrypointEntry::Object { value, .. } => value,
        }
    }

    pub fn adapter(&self) -> Option<&str> {
        match self {
            EntrypointEntry::Simple(_) => None,
            EntrypointEntry::Object { adapter, .. } => adapter.as_deref(),
        }
    }
}

impl FabricModMeta {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub fn has_mixins(&self) -> bool {
        match &self.mixins {
            Value::Null => false,
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
            _ => true,
        }
    }

    pub fn mixin_count(&self) -> usize {
        match &self.mixins {
            Value::Null => 0,
            Value::Array(a) => a.len(),
            Value::Object(o) => o.len(),
            _ => 1,
        }
    }
}

/// Read metadata from a jar, a folder holding `fabric.mod.json`, a JSON file,
/// or a `<name>.mod.json` sidecar (the milestone test layout).
pub fn load_mod_meta(path: &Path) -> Result<FabricModMeta, FabricError> {
    if path.is_dir() {
        let meta = path.join("fabric.mod.json");
        if meta.is_file() {
            return read_meta_file(&meta);
        }
        return Err(FabricError::MissingMeta);
    }
    if path.extension().is_some_and(|e| e == "json") {
        return read_meta_file(path);
    }
    if path.is_file() {
        if let Some(meta) = read_meta_from_jar(path)? {
            return Ok(meta);
        }
        let side = path.with_extension("mod.json");
        if side.is_file() {
            return read_meta_file(&side);
        }
        return Err(FabricError::MissingMeta);
    }
    Err(FabricError::MissingMeta)
}

fn read_meta_file(path: &Path) -> Result<FabricModMeta, FabricError> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn read_meta_from_jar(path: &Path) -> Result<Option<FabricModMeta>, FabricError> {
    let file = File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| FabricError::Other(e.to_string()))?;
    for name in ["fabric.mod.json", "quilt.mod.json"] {
        let mut entry = match zip.by_name(name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => continue,
            Err(e) => return Err(FabricError::Other(format!("{name}: {e}"))),
        };
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        if name == "quilt.mod.json" {
            // Quilt's top-level metadata is under `quilt_loader`.
            let value: Value = serde_json::from_str(&text)?;
            if let Some(loader) = value.get("quilt_loader") {
                return Ok(Some(serde_json::from_value(loader.clone())?));
            }
            continue;
        }
        return Ok(Some(serde_json::from_str(&text)?));
    }
    Ok(None)
}

/// Extract a mod's `META-INF/jars/*.jar` into `cache/<mod id>/`, returning
/// every extracted jar. The caller puts plain libraries on the classpath and
/// only treats jars with metadata as mods.
pub fn extract_nested_jars(jar: &Path, cache: &Path) -> Result<Vec<PathBuf>, FabricError> {
    let file = File::open(jar)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| FabricError::Other(e.to_string()))?;
    let stem = jar
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mod".into());
    let out_dir = cache.join(&stem);
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| FabricError::Other(e.to_string()))?;
        let name = entry
            .name()
            .map_err(|e| FabricError::Other(e.to_string()))?
            .into_owned();
        if !name.starts_with("META-INF/jars/") || !name.ends_with(".jar") || entry.is_dir() {
            continue;
        }
        let Some(file_name) = name.rsplit('/').next() else {
            continue;
        };
        fs::create_dir_all(&out_dir)?;
        let target = out_dir.join(file_name);
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let mut file = File::create(&target)?;
        file.write_all(&bytes)?;
        out.push(target);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct FabricEntrypoint {
    pub mod_id: String,
    pub key: String,
    pub class_binary_name: String,
    pub method: String,
}

impl FabricEntrypoint {
    /// The method an entrypoint invokes. A plain class value implements the
    /// Fabric interface for its key; `main` -> `onInitialize`
    /// (`ModInitializer`), `client` -> `onInitializeClient`
    /// (`ClientModInitializer`), `server` -> `onInitializeServer`
    /// (`ServerModInitializer`), `preLaunch` -> `onPreLaunch`. Unknown or
    /// custom keys (e.g. Mod Menu's `modmenu`) keep the generic
    /// `onInitialize`. A `Class::method` value always wins.
    pub fn from_value(mod_id: &str, key: &str, value: &str) -> Self {
        let default_method = match key {
            "client" => "onInitializeClient",
            "server" => "onInitializeServer",
            "preLaunch" => "onPreLaunch",
            _ => "onInitialize",
        };
        let (class, method) = value.split_once("::").unwrap_or((value, default_method));
        Self {
            mod_id: mod_id.to_owned(),
            key: key.to_owned(),
            class_binary_name: class.replace('.', "/"),
            method: method.to_owned(),
        }
    }

    pub fn from_meta(meta: &FabricModMeta, keys: &[&str]) -> Vec<Self> {
        let mut out = Vec::new();
        for key in keys {
            let Some(entries) = meta.entrypoints.get(*key) else {
                continue;
            };
            for entry in entries {
                out.push(Self::from_value(&meta.id, key, entry.value()));
            }
        }
        out
    }

    pub fn run(&self, jvm: &JvmRuntime) -> Result<(), FabricError> {
        info!(
            "J2H: entrypoint {}.{}() mod={} key={}",
            self.class_binary_name, self.method, self.mod_id, self.key
        );
        jvm.call_entrypoint(&self.class_binary_name, &self.method, "()V")?;
        Ok(())
    }
}

pub struct LoadedMod {
    pub meta: FabricModMeta,
    pub jar: PathBuf,
    pub entrypoints: Vec<FabricEntrypoint>,
}

impl LoadedMod {
    pub fn entrypoints(&self, keys: &[&str]) -> Vec<&FabricEntrypoint> {
        self.entrypoints
            .iter()
            .filter(|e| keys.contains(&e.key.as_str()))
            .collect()
    }
}

pub struct FabricLoader {
    mods: Vec<LoadedMod>,
    /// Jars that are on the classpath but are not mods (nested libraries).
    classpath: Vec<PathBuf>,
    side: EntrypointSide,
}

impl FabricLoader {
    pub fn new() -> Self {
        Self {
            mods: Vec::new(),
            classpath: Vec::new(),
            side: EntrypointSide::Client,
        }
    }

    /// The side whose `environment` and entrypoints this loader applies.
    pub fn set_side(&mut self, side: EntrypointSide) {
        self.side = side;
    }

    /// Put a plain library jar on the classpath without treating it as a mod.
    pub fn add_classpath(&mut self, jar: PathBuf) {
        if !self.classpath.contains(&jar) {
            self.classpath.push(jar);
        }
    }

    /// Put a resolved runtime library on the classpath, skipping any jar with
    /// the same file name (the versioned name identifies the artifact).
    pub fn add_library_classpath(&mut self, jar: PathBuf) {
        let name = jar.file_name();
        let duplicate = self
            .mods
            .iter()
            .map(|m| m.jar.as_path())
            .chain(self.classpath.iter().map(PathBuf::as_path))
            .any(|seen| seen == jar || (name.is_some() && seen.file_name() == name));
        if !duplicate {
            self.classpath.push(jar);
        }
    }

    /// Add a mod from a jar and an explicit metadata file (the milestone
    /// layout).
    pub fn add_mod(&mut self, jar: PathBuf, meta_path: &Path) -> Result<(), FabricError> {
        let meta = load_mod_meta(meta_path)?;
        self.add_with_meta(jar, meta)
    }

    /// Add a jar whose metadata is inside it.
    pub fn add_jar(&mut self, jar: PathBuf) -> Result<(), FabricError> {
        let meta = load_mod_meta(&jar)?;
        self.add_with_meta(jar, meta)
    }

    pub fn add_with_meta(&mut self, jar: PathBuf, meta: FabricModMeta) -> Result<(), FabricError> {
        if !self.side.accepts(meta.environment.as_deref()) {
            info!(
                "J2H: {} is {}-only; skipped on {:?}",
                meta.id,
                meta.environment.as_deref().unwrap_or("both"),
                self.side
            );
            return Ok(());
        }
        if self.mods.iter().any(|m| m.meta.id == meta.id) {
            return Err(FabricError::Other(format!(
                "two mods claim the id '{}'",
                meta.id
            )));
        }
        if meta.has_mixins() {
            info!(
                "J2H: mod '{}' declares {} mixin configuration(s)",
                meta.id,
                meta.mixin_count()
            );
        }
        if meta.access_widener.is_some() {
            info!("J2H: mod '{}' declares an access widener", meta.id);
        }
        let entrypoints = FabricEntrypoint::from_meta(&meta, &["main", "client", "server"]);
        info!(
            "J2H: discovered {} v{} ({})",
            meta.id,
            meta.version,
            meta.display_name()
        );
        self.mods.push(LoadedMod {
            meta,
            jar,
            entrypoints,
        });
        Ok(())
    }

    /// Discover jars and mod folders in `dir`. Nested jars are unpacked into
    /// `nested_cache` when given.
    pub fn discover_dir(&mut self, dir: &Path) -> usize {
        self.discover_dir_with_nested(dir, None)
    }

    pub fn discover_dir_with_nested(&mut self, dir: &Path, nested_cache: Option<&Path>) -> usize {
        let Ok(read) = fs::read_dir(dir) else {
            return 0;
        };
        let mut paths: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir() && p.join("fabric.mod.json").is_file()
                    || p.extension().is_some_and(|e| e == "jar")
            })
            .collect();
        paths.sort();
        let mut added = 0;
        let mut nested = Vec::new();
        for path in paths {
            let before = self.mods.len();
            match self.add_jar(path.clone()) {
                Ok(()) if self.mods.len() > before => added += 1,
                Ok(()) => {}
                Err(FabricError::MissingMeta) => self.add_classpath(path.clone()),
                Err(e) => warn!("J2H: {}: {e}", path.display()),
            }
            if let Some(cache) = nested_cache {
                match extract_nested_jars(&path, cache) {
                    Ok(jars) => nested.extend(jars),
                    Err(e) => warn!("J2H: {} nested jars: {e}", path.display()),
                }
            }
        }
        for path in nested {
            let before = self.mods.len();
            match self.add_jar(path.clone()) {
                Ok(()) if self.mods.len() > before => added += 1,
                Ok(()) => {}
                Err(FabricError::MissingMeta) => self.add_classpath(path),
                Err(e) => warn!("J2H: nested {}: {e}", path.display()),
            }
        }
        added
    }

    pub fn check_dependencies(&self) -> Result<(), FabricError> {
        let available: HashSet<&str> = self
            .mods
            .iter()
            .flat_map(|m| {
                std::iter::once(m.meta.id.as_str())
                    .chain(m.meta.provides.iter().map(String::as_str))
            })
            .collect();
        let by_id: HashMap<&str, &FabricModMeta> = self
            .mods
            .iter()
            .map(|m| (m.meta.id.as_str(), &m.meta))
            .collect();
        let mut missing = Vec::new();
        for m in &self.mods {
            for (dep, requirement) in &m.meta.depends {
                if builtin(dep) {
                    if let Some(version) = builtin_version(dep)
                        && !requirement.accepts(version)
                    {
                        missing.push(format!(
                            "mod '{}': needs {dep} {requirement}, bundled version is {version}",
                            m.meta.id
                        ));
                    }
                    continue;
                }
                if !available.contains(dep.as_str()) {
                    missing.push(format!("mod '{}': missing dependency '{dep}'", m.meta.id));
                    continue;
                }
                if let Some(found) = by_id.get(dep.as_str()) {
                    if !requirement.accepts(&found.version) {
                        missing.push(format!(
                            "mod '{}': needs {dep} {requirement}, found {}",
                            m.meta.id, found.version
                        ));
                    }
                }
            }
        }
        for m in &self.mods {
            for (other, requirement) in m.meta.breaks.iter().chain(m.meta.conflicts.iter()) {
                if builtin(other) {
                    continue;
                }
                let Some(found) = by_id.get(other.as_str()) else {
                    continue;
                };
                if requirement.accepts(&found.version) {
                    missing.push(format!(
                        "mod '{}': incompatible with '{other}' {}",
                        m.meta.id, found.version
                    ));
                }
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(FabricError::Other(missing.join("; ")))
        }
    }

    /// Kahn's order over `depends`; a cycle is appended in discovery order
    /// after its acyclic part.
    pub fn ordered(&self) -> Vec<&LoadedMod> {
        let mut remaining: Vec<&LoadedMod> = self.mods.iter().collect();
        let mut ordered = Vec::new();
        let mut done: HashSet<String> = HashSet::new();
        while !remaining.is_empty() {
            let (ready, rest): (Vec<_>, Vec<_>) = remaining.into_iter().partition(|m| {
                m.meta
                    .depends
                    .keys()
                    .all(|d| builtin(d) || done.contains(d))
            });
            if ready.is_empty() {
                ordered.extend(rest);
                break;
            }
            for m in ready {
                done.insert(m.meta.id.clone());
                done.extend(m.meta.provides.iter().cloned());
                ordered.push(m);
            }
            remaining = rest;
        }
        ordered
    }

    pub fn run_entrypoints(&self, jvm: &JvmRuntime) -> Result<(), FabricError> {
        self.run_entrypoints_for(jvm, &["main"])
    }

    pub fn run_entrypoints_for(&self, jvm: &JvmRuntime, keys: &[&str]) -> Result<(), FabricError> {
        for m in self.ordered() {
            for entrypoint in m.entrypoints(keys) {
                entrypoint.run(jvm)?;
            }
        }
        Ok(())
    }

    pub fn mod_ids(&self) -> Vec<String> {
        self.mods.iter().map(|m| m.meta.id.clone()).collect()
    }

    pub fn jars(&self) -> Vec<PathBuf> {
        let mut jars: Vec<PathBuf> = self.mods.iter().map(|m| m.jar.clone()).collect();
        for jar in &self.classpath {
            if !jars.contains(jar) {
                jars.push(jar.clone());
            }
        }
        jars
    }

    /// Whether any discovered mod expects the Fabric Loader API at runtime.
    pub fn needs_fabric_runtime(&self) -> bool {
        self.mods.iter().any(|m| {
            m.meta
                .depends
                .keys()
                .any(|dep| dep == "fabricloader" || dep == "fabric" || dep.starts_with("fabric-"))
        })
    }

    /// Whether any discovered mod expects Minecraft's runtime libraries.
    pub fn needs_minecraft_runtime(&self) -> bool {
        self.mods.iter().any(|m| {
            m.meta.depends.keys().any(|dep| {
                dep == "minecraft" || dep == "fabricloader" || dep.starts_with("fabric-")
            })
        })
    }

    pub fn mods(&self) -> &[LoadedMod] {
        &self.mods
    }

    pub fn len(&self) -> usize {
        self.mods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }
}

impl Default for FabricLoader {
    fn default() -> Self {
        Self::new()
    }
}

pub fn load_single(jar: PathBuf, meta: &Path) -> Result<FabricLoader, FabricError> {
    let mut l = FabricLoader::new();
    l.add_mod(jar, meta)?;
    if let Err(e) = l.check_dependencies() {
        warn!("J2H: {e}");
    }
    Ok(l)
}

fn builtin(id: &str) -> bool {
    builtin_version(id).is_some()
        || matches!(
            id,
            "java"
                | "minecraft"
                | "fabricloader"
                | "fabric"
                | "fabric-api"
                | "quilt_loader"
                | "quilt_base"
                | "horizon"
        )
}

/// Fabric Loader 0.20.1 bundles MixinExtras as a nested Fabric mod.
/// Keep the metadata dependency resolver in sync with that pinned runtime.
fn builtin_version(id: &str) -> Option<&'static str> {
    match id {
        "mixinextras" => Some("0.5.5"),
        _ => None,
    }
}

/// A small Fabric-style version predicate: `*`, exact, `>=`, `>`, `<=`, `<`,
/// `~`, `^`, and a space-separated conjunction. A trailing `.x` is a prefix.
pub fn version_satisfies(version: &str, requirement: &str) -> bool {
    let requirement = requirement.trim();
    if requirement.is_empty() || requirement == "*" {
        return true;
    }
    requirement.split_whitespace().all(|part| {
        let (op, wanted) = if let Some(rest) = part.strip_prefix(">=") {
            (">=", rest)
        } else if let Some(rest) = part.strip_prefix("<=") {
            ("<=", rest)
        } else if let Some(rest) = part.strip_prefix("~=") {
            ("~", rest)
        } else if let Some(rest) = part.strip_prefix(">") {
            (">", rest)
        } else if let Some(rest) = part.strip_prefix("<") {
            ("<", rest)
        } else if let Some(rest) = part.strip_prefix("~") {
            ("~", rest)
        } else if let Some(rest) = part.strip_prefix("^") {
            ("^", rest)
        } else if let Some(rest) = part.strip_prefix('=') {
            ("=", rest)
        } else {
            ("=", part)
        };
        compare(version, wanted.trim(), op)
    })
}

fn parts(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+'])
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

fn compare(version: &str, wanted: &str, op: &str) -> bool {
    if let Some(prefix) = wanted.strip_suffix(".x") {
        let v = parts(version);
        let w = parts(prefix);
        return w.iter().enumerate().all(|(i, n)| v.get(i) == Some(n));
    }
    let v = parts(version);
    let w = parts(wanted);
    let cmp = v.cmp(&w);
    match op {
        "=" => cmp.is_eq(),
        ">" => cmp.is_gt(),
        ">=" => cmp.is_ge(),
        "<" => cmp.is_lt(),
        "<=" => cmp.is_le(),
        "~" => cmp.is_ge() && v.first() == w.first() && v.get(1) == w.get(1),
        "^" => {
            cmp.is_ge()
                && match w.first().copied().unwrap_or(0) {
                    0 => v.get(1) == w.get(1),
                    _ => v.first() == w.first(),
                }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_cover_the_common_fabric_operators() {
        assert!(version_satisfies("1.2.3", ">=1.0"));
        assert!(version_satisfies("1.2.3", ">=1.2.3 <2"));
        assert!(version_satisfies("1.2.3", "1.2.x"));
        assert!(!version_satisfies("1.2.3", "2.x"));
        assert!(version_satisfies("1.9.0", "~1.9"));
        assert!(!version_satisfies("1.10.0", "~1.9"));
    }

    #[test]
    fn entrypoint_values_split_class_and_method() {
        let e = FabricEntrypoint::from_value("m", "main", "com.example.Mod::init");
        assert_eq!(e.class_binary_name, "com/example/Mod");
        assert_eq!(e.method, "init");
        let e = FabricEntrypoint::from_value("m", "main", "com.example.Mod");
        assert_eq!(e.class_binary_name, "com/example/Mod");
        assert_eq!(e.method, "onInitialize");
    }

    #[test]
    fn entrypoint_key_selects_the_fabric_interface_method() {
        let cases = [
            ("main", "onInitialize"),
            ("client", "onInitializeClient"),
            ("server", "onInitializeServer"),
            ("preLaunch", "onPreLaunch"),
        ];
        for (key, method) in cases {
            let e = FabricEntrypoint::from_value("m", key, "com.example.Mod");
            assert_eq!(e.method, method, "key={key}");
        }
        let e = FabricEntrypoint::from_value("m", "client", "com.example.Mod::custom");
        assert_eq!(e.method, "custom", "an explicit method must win");
        let e = FabricEntrypoint::from_value("m", "modmenu", "com.example.CustomEndpoint");
        assert_eq!(
            e.method, "onInitialize",
            "unknown keys keep the generic default"
        );
    }

    #[test]
    fn mod_menu_client_entrypoint_is_on_initialize_client() {
        // The real Mod Menu 21.0.0 descriptor: its `client` entrypoint is a
        // `ClientModInitializer` (method `onInitializeClient`), never
        // `onInitialize`.
        let jar = Path::new("/tmp/j2h-e2e/game3/mods/modmenu-21.0.0.jar");
        if !jar.is_file() {
            return;
        }
        let meta = load_mod_meta(jar).unwrap();
        assert_eq!(meta.id, "modmenu");
        assert_eq!(meta.environment.as_deref(), Some("client"));
        let client = meta
            .entrypoints
            .get("client")
            .and_then(|v| v.first())
            .expect("modmenu has a client entrypoint");
        let e = FabricEntrypoint::from_value(&meta.id, "client", client.value());
        assert_eq!(e.class_binary_name, "com/terraformersmc/modmenu/ModMenu");
        assert_eq!(e.method, "onInitializeClient");
    }

    fn meta(id: &str, environment: &str) -> FabricModMeta {
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "environment": environment,
        }))
        .unwrap()
    }

    #[test]
    fn environment_selects_the_side_that_loads_a_mod() {
        let mut client = FabricLoader::new();
        client
            .add_with_meta(PathBuf::from("/tmp/client.jar"), meta("client", "client"))
            .unwrap();
        client
            .add_with_meta(PathBuf::from("/tmp/server.jar"), meta("server", "server"))
            .unwrap();
        client
            .add_with_meta(PathBuf::from("/tmp/both.jar"), meta("both", "*"))
            .unwrap();
        assert_eq!(client.mod_ids(), ["client", "both"]);

        let mut server = FabricLoader::new();
        server.set_side(EntrypointSide::Server);
        server
            .add_with_meta(PathBuf::from("/tmp/client.jar"), meta("client", "client"))
            .unwrap();
        server
            .add_with_meta(PathBuf::from("/tmp/server.jar"), meta("server", "server"))
            .unwrap();
        server
            .add_with_meta(PathBuf::from("/tmp/both.jar"), meta("both", "*"))
            .unwrap();
        assert_eq!(server.mod_ids(), ["server", "both"]);
    }

    #[test]
    fn a_breaks_or_conflicts_range_refuses_the_pair() {
        let mut loader = FabricLoader::new();
        loader
            .add_with_meta(PathBuf::from("/tmp/a.jar"), meta("a", "*"))
            .unwrap();
        let mut broken = meta("b", "*");
        broken.breaks.insert("a".into(), ">=1.0".into());
        loader
            .add_with_meta(PathBuf::from("/tmp/b.jar"), broken)
            .unwrap();
        assert!(
            loader
                .check_dependencies()
                .unwrap_err()
                .to_string()
                .contains("incompatible")
        );
    }

    #[test]
    fn a_plain_library_is_on_the_classpath_but_is_not_a_mod() {
        let mut loader = FabricLoader::new();
        loader.add_classpath(PathBuf::from("/tmp/library.jar"));
        assert!(loader.is_empty());
        assert_eq!(loader.jars(), [PathBuf::from("/tmp/library.jar")]);
    }

    #[test]
    fn fabric_loader_bundled_mixinextras_satisfies_only_its_pinned_version() {
        let mut loader = FabricLoader::new();
        let mut mod_meta = meta("mixin_consumer", "*");
        mod_meta
            .depends
            .insert("mixinextras".into(), ">=0.5.5".into());
        loader
            .add_with_meta(PathBuf::from("/tmp/mixin-consumer.jar"), mod_meta.clone())
            .unwrap();
        assert!(loader.check_dependencies().is_ok());

        mod_meta
            .depends
            .insert("mixinextras".into(), ">=0.5.6".into());
        let mut incompatible = FabricLoader::new();
        incompatible
            .add_with_meta(PathBuf::from("/tmp/mixin-consumer.jar"), mod_meta)
            .unwrap();
        let error = incompatible.check_dependencies().unwrap_err().to_string();
        assert!(error.contains("bundled version is 0.5.5"), "{error}");
    }

    #[test]
    fn dependency_version_arrays_accept_any_matching_version() {
        let mut loader = FabricLoader::new();
        loader
            .add_with_meta(PathBuf::from("/tmp/sodium.jar"), {
                let mut sodium = meta("sodium", "*");
                sodium.version = "0.9.2".into();
                sodium
            })
            .unwrap();
        let consumer: FabricModMeta = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "id": "consumer",
            "version": "1.0.0",
            "depends": { "sodium": ["0.8.x", "0.9.x"] }
        }))
        .unwrap();
        loader
            .add_with_meta(PathBuf::from("/tmp/consumer.jar"), consumer.clone())
            .unwrap();
        assert!(loader.check_dependencies().is_ok());

        let incompatible: FabricModMeta = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "id": "incompatible_consumer",
            "version": "1.0.0",
            "depends": { "sodium": ["0.8.x", "0.10.x"] }
        }))
        .unwrap();
        let mut loader = FabricLoader::new();
        let mut sodium = meta("sodium", "*");
        sodium.version = "0.9.2".into();
        loader
            .add_with_meta(PathBuf::from("/tmp/sodium.jar"), sodium)
            .unwrap();
        loader
            .add_with_meta(PathBuf::from("/tmp/consumer.jar"), incompatible)
            .unwrap();
        assert!(loader.check_dependencies().is_err());
    }

    #[test]
    fn metadata_reads_from_the_test_jar() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let meta = load_mod_meta(&root.join("test_resources/testmod.jar")).unwrap();
        assert_eq!(meta.id, "testmod");
        assert_eq!(
            meta.depends.get("libmod"),
            Some(&VersionRequirement::from(">=1.0.0"))
        );
    }
}
