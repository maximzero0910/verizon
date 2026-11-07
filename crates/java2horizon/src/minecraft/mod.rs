//! Minecraft's own runtime libraries, resolved from Mojang's version manifest.
//!
//! This is deliberately separate from Fabric Loader/API provisioning: the
//! Fabric cache lives under `fabric/`, Minecraft's under `minecraft/`.

pub mod libraries;
pub mod metadata;

pub use libraries::Resolver;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MinecraftError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("HTTP: {0}")]
    Http(String),
    #[error("metadata: {0}")]
    Metadata(String),
    #[error("offline: {0}")]
    Offline(String),
    #[error("version: {0}")]
    MissingVersion(String),
    #[error("missing artifact: {0}")]
    MissingArtifact(String),
    #[error("invalid artifact {path}: expected sha1 {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("invalid artifact {path}: expected {expected} bytes, got {actual}")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },
}
