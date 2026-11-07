//! Mojang's version manifest, version JSON and library rules.
//!
//! The version JSON's `libraries` array is the launch classpath Mojang
//! publishes for a version; it already contains transitive runtime libraries.
//! J2H evaluates its rules for this OS/architecture and keeps the metadata
//! order, which is also the launcher's classpath order.

use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::process::Command;

pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    #[serde(default)]
    pub versions: Vec<VersionRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionRef {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

impl VersionManifest {
    pub fn find(&self, id: &str) -> Option<&VersionRef> {
        self.versions.iter().find(|version| version.id == id)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionMeta {
    pub id: String,
    #[serde(default)]
    pub downloads: VersionDownloads,
    #[serde(default)]
    pub libraries: Vec<Library>,
}

/// The version JSON's top-level `downloads`: the launchable client and
/// server jars Mojang publishes. These hold the `net.minecraft` classes a
/// Fabric mod runs against; the `libraries` array only supplies the
/// dependencies.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct VersionDownloads {
    #[serde(default)]
    pub client: Option<Download>,
    #[serde(default)]
    pub server: Option<Download>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Downloads,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Downloads {
    #[serde(default)]
    pub artifact: Option<Download>,
}

/// A single published artifact. The version JSON's `downloads.client` and
/// `downloads.server` have no `path` (Mojang names them `client.jar` and
/// `server.jar`); a `libraries` artifact always has one.
#[derive(Debug, Clone, Deserialize)]
pub struct Download {
    #[serde(default)]
    pub path: Option<String>,
    pub sha1: String,
    #[serde(default)]
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsRule>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// The platform and launcher features a rule is evaluated against.
#[derive(Debug, Clone)]
pub struct RuleContext {
    /// Mojang's name: `linux`, `osx` or `windows`.
    pub os: String,
    /// Mojang's name: `x86`, `x86_64`, `arm` or `arm64`.
    pub arch: String,
    pub os_version: Option<String>,
    pub features: HashMap<String, bool>,
}

impl RuleContext {
    pub fn current() -> Self {
        Self {
            os: current_os().into(),
            arch: current_arch().into(),
            os_version: current_os_version(),
            features: HashMap::new(),
        }
    }
}

impl Library {
    /// Mojang's rule semantics: no rules means allow; otherwise the last
    /// matching rule decides.
    pub fn applies(&self, context: &RuleContext) -> bool {
        let Some(rules) = &self.rules else {
            return true;
        };
        let mut allowed = false;
        for rule in rules {
            if rule.matches(context) {
                allowed = rule.action == RuleAction::Allow;
            }
        }
        allowed
    }
}

impl Rule {
    fn matches(&self, context: &RuleContext) -> bool {
        if let Some(os) = &self.os {
            if let Some(name) = &os.name {
                if name != &context.os {
                    return false;
                }
            }
            if let Some(arch) = &os.arch {
                if arch != &context.arch {
                    return false;
                }
            }
            if let Some(version) = &os.version {
                let Some(actual) = &context.os_version else {
                    return false;
                };
                let Ok(pattern) = Regex::new(version) else {
                    return false;
                };
                if !pattern.is_match(actual) {
                    return false;
                }
            }
        }
        if let Some(features) = &self.features {
            for (name, expected) in features {
                if context.features.get(name).copied().unwrap_or(false) != *expected {
                    return false;
                }
            }
        }
        true
    }
}

/// A library selected for this platform, in Mojang's metadata order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLibrary {
    pub name: String,
    /// Maven-style path below the library cache, e.g. `com/google/.../guava.jar`.
    pub path: String,
    pub url: String,
    pub sha1: String,
    pub size: Option<u64>,
}

impl ResolvedLibrary {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

impl VersionMeta {
    pub fn resolve_libraries(&self, context: &RuleContext) -> Vec<ResolvedLibrary> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for library in &self.libraries {
            if !library.applies(context) {
                continue;
            }
            let Some(download) = &library.downloads.artifact else {
                continue;
            };
            let Some(path) = &download.path else {
                continue;
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            out.push(ResolvedLibrary {
                name: library.name.clone(),
                path: path.clone(),
                url: download.url.clone(),
                sha1: download.sha1.clone(),
                size: download.size,
            });
        }
        out
    }

    /// The launchable client jar, as a library-shaped artifact below the
    /// cache root. Mojang publishes it without a `path`; on disk it is
    /// `client-<version>.jar`.
    pub fn client_download(&self, version_id: &str) -> Option<ResolvedLibrary> {
        self.downloads.client.as_ref().map(|d| ResolvedLibrary {
            name: format!("minecraft:client:{version_id}"),
            path: format!("client-{version_id}.jar"),
            url: d.url.clone(),
            sha1: d.sha1.clone(),
            size: d.size,
        })
    }
}

fn current_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "osx",
        other => other,
    }
}

fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

fn current_os_version() -> Option<String> {
    if let Ok(version) = std::env::var("HORIZON_OS_VERSION") {
        if !version.trim().is_empty() {
            return Some(version);
        }
    }
    #[cfg(target_os = "linux")]
    let (program, args): (&str, &[&str]) = ("uname", &["-r"]);
    #[cfg(target_os = "macos")]
    let (program, args): (&str, &[&str]) = ("sw_vers", &["-productVersion"]);
    #[cfg(target_os = "windows")]
    let (program, args): (&str, &[&str]) = ("cmd", &["/C", "ver"]);
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let (program, args): (&str, &[&str]) = ("uname", &["-r"]);

    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> RuleContext {
        RuleContext {
            os: "linux".into(),
            arch: "x86_64".into(),
            os_version: Some("6.9.0".into()),
            features: HashMap::new(),
        }
    }

    #[test]
    fn manifest_finds_the_pinned_version() {
        let manifest: VersionManifest = serde_json::from_str(
            r#"{"versions":[{"id":"26.2","url":"https://example/26.2.json","sha1":"a"},{"id":"26.3","url":"https://example/26.3.json","sha1":"b"}]}"#,
        )
        .unwrap();
        let found = manifest.find("26.3").unwrap();
        assert_eq!(found.url, "https://example/26.3.json");
        assert_eq!(found.sha1.as_deref(), Some("b"));
    }

    #[test]
    fn rules_filter_os_arch_version_and_features() {
        let meta: VersionMeta = serde_json::from_str(
            r#"{
                "id":"26.3",
                "libraries":[
                    {"name":"always","downloads":{"artifact":{"path":"always.jar","sha1":"a","url":"https://example/always.jar"}}},
                    {"name":"linux","downloads":{"artifact":{"path":"linux.jar","sha1":"b","url":"https://example/linux.jar"}},"rules":[{"action":"allow","os":{"name":"linux"}}]},
                    {"name":"windows","downloads":{"artifact":{"path":"windows.jar","sha1":"c","url":"https://example/windows.jar"}},"rules":[{"action":"allow","os":{"name":"windows"}}]},
                    {"name":"x86","downloads":{"artifact":{"path":"x86.jar","sha1":"d","url":"https://example/x86.jar"}},"rules":[{"action":"allow","os":{"arch":"x86_64"}}]},
                    {"name":"arm","downloads":{"artifact":{"path":"arm.jar","sha1":"e","url":"https://example/arm.jar"}},"rules":[{"action":"allow","os":{"arch":"arm64"}}]},
                    {"name":"old-linux","downloads":{"artifact":{"path":"old.jar","sha1":"f","url":"https://example/old.jar"}},"rules":[{"action":"allow","os":{"version":"^5\\."}}]},
                    {"name":"new-linux","downloads":{"artifact":{"path":"new.jar","sha1":"0","url":"https://example/new.jar"}},"rules":[{"action":"allow","os":{"version":"^6\\."}}]},
                    {"name":"demo","downloads":{"artifact":{"path":"demo.jar","sha1":"1","url":"https://example/demo.jar"}},"rules":[{"action":"allow","features":{"is_demo_user":true}}]},
                    {"name":"blocked","downloads":{"artifact":{"path":"blocked.jar","sha1":"2","url":"https://example/blocked.jar"}},"rules":[{"action":"allow"},{"action":"disallow","os":{"name":"linux"}}]}
                ]
            }"#,
        )
        .unwrap();
        let selected: Vec<_> = meta
            .resolve_libraries(&context())
            .into_iter()
            .map(|library| library.path)
            .collect();
        assert_eq!(
            selected,
            vec!["always.jar", "linux.jar", "x86.jar", "new.jar",]
        );
    }

    #[test]
    fn a_later_matching_rule_overrides_an_earlier_one() {
        let meta: VersionMeta = serde_json::from_str(
            r#"{
                "id":"26.3",
                "libraries":[
                    {"name":"blocked","downloads":{"artifact":{"path":"blocked.jar","sha1":"a","url":"https://example/blocked.jar"}},"rules":[{"action":"allow"},{"action":"disallow","os":{"name":"linux"}},{"action":"allow","os":{"arch":"x86_64"}}]},
                    {"name":"allowed","downloads":{"artifact":{"path":"allowed.jar","sha1":"b","url":"https://example/allowed.jar"}},"rules":[{"action":"disallow"},{"action":"allow","os":{"name":"linux"}}]}
                ]
            }"#,
        )
        .unwrap();
        let selected: Vec<_> = meta
            .resolve_libraries(&context())
            .into_iter()
            .map(|library| library.path)
            .collect();
        assert_eq!(selected, vec!["blocked.jar", "allowed.jar"]);
    }

    #[test]
    fn duplicate_paths_keep_the_first_metadata_entry() {
        let meta: VersionMeta = serde_json::from_str(
            r#"{
                "id":"26.3",
                "libraries":[
                    {"name":"first","downloads":{"artifact":{"path":"same.jar","sha1":"a","url":"https://example/first.jar"}}},
                    {"name":"second","downloads":{"artifact":{"path":"same.jar","sha1":"b","url":"https://example/second.jar"}}}
                ]
            }"#,
        )
        .unwrap();
        let selected = meta.resolve_libraries(&context());
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].url, "https://example/first.jar");
    }

    #[test]
    fn transitive_entries_from_the_version_json_are_kept() {
        let meta: VersionMeta = serde_json::from_str(
            r#"{
                "id":"26.3",
                "libraries":[
                    {"name":"com.google.guava:failureaccess:1.0.3","downloads":{"artifact":{"path":"com/google/guava/failureaccess/1.0.3/failureaccess-1.0.3.jar","sha1":"a","url":"https://example/failureaccess.jar"}}},
                    {"name":"com.google.guava:guava:33.6.0-jre","downloads":{"artifact":{"path":"com/google/guava/guava/33.6.0-jre/guava-33.6.0-jre.jar","sha1":"b","url":"https://example/guava.jar"}}}
                ]
            }"#,
        )
        .unwrap();
        let selected = meta.resolve_libraries(&context());
        assert_eq!(selected.len(), 2);
        assert!(
            selected
                .iter()
                .any(|library| library.file_name() == "guava-33.6.0-jre.jar")
        );
        assert!(
            selected
                .iter()
                .any(|library| library.file_name() == "failureaccess-1.0.3.jar")
        );
    }
}
