//! Horizon content integration.
//!
//! Horizon has no runtime `register_block` API for code. A mod's content is a
//! content-mod tree (`horizon-mod.toml` + `data/<id>/horizon/...`), which the
//! client overlays onto its prepared resource root before the world loads.
//! J2H collects Java registrations and writes exactly that tree; every tree
//! is then parsed with the real `horizon-mods` loader (`Manifest::parse` and
//! `Content::read`), so a registration that Horizon would refuse fails here
//! instead of producing a silently broken mod.
//!
//! Blocks and items use Horizon's template model. Entity types, mob effects,
//! creative tabs and menu types are written in the same data-driven format.
//! Commands, networking and rendering have no content-mod representation yet;
//! they are recorded as pending callbacks and reported as unsupported.

use crate::bridge::NativeCallback;
use horizon_mods::content::{Content, Rarity};
use horizon_mods::manifest::{Capability, Manifest, Side};
use log::{info, warn};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HorizonError {
    #[error("invalid id: {0}")]
    InvalidId(String),
    #[error("duplicate: {0}")]
    Duplicate(String),
    #[error("horizon-mods validation: {0}")]
    HorizonMods(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Clone, Debug)]
pub struct PendingBlock {
    pub id: String,
    pub display_name: String,
    pub template: String,
    pub hardness: f32,
    pub light: Option<u8>,
    pub item: bool,
}

#[derive(Clone, Debug)]
pub struct PendingItem {
    pub id: String,
    pub display_name: String,
    pub template: String,
    pub max_stack_size: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct PendingEntityType {
    pub id: String,
    pub template: String,
    pub item: Option<String>,
    pub damage: f32,
}

#[derive(Clone, Debug)]
pub struct PendingMobEffect {
    pub id: String,
    pub display_name: String,
    pub color: u32,
    pub category: String,
}

#[derive(Clone, Debug)]
pub struct PendingCreativeTab {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub items: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PendingMenuType {
    pub id: String,
    pub slots: u16,
    pub data_slots: u16,
    pub results: Vec<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockStateId(pub u32);

#[derive(Clone, Default)]
struct EngineInner {
    vanilla_base: u32,
    next_id: u32,
    blocks: HashMap<String, (PendingBlock, BlockStateId)>,
    items: HashMap<String, PendingItem>,
    entity_types: HashMap<String, PendingEntityType>,
    mob_effects: HashMap<String, PendingMobEffect>,
    creative_tabs: HashMap<String, PendingCreativeTab>,
    menu_types: HashMap<String, PendingMenuType>,
    commands: Vec<(String, String)>,
    lifecycle_events: Vec<(String, String)>,
    log: Vec<String>,
    initialized_mods: Vec<String>,
    mod_namespaces: BTreeMap<String, String>,
    owned_callbacks: Vec<(String, NativeCallback)>,
}

pub struct HorizonEngine {
    inner: Arc<Mutex<EngineInner>>,
}

impl HorizonEngine {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                vanilla_base: 22_000,
                next_id: 22_000,
                ..EngineInner::default()
            })),
        }
    }

    pub fn apply(&self, cb: &NativeCallback) -> Result<EngineEvent, HorizonError> {
        let mut g = self.inner.lock().unwrap();
        match cb {
            NativeCallback::Message(msg) => {
                g.log.push(msg.clone());
                Ok(EngineEvent::Log(msg.clone()))
            }
            NativeCallback::RegisterBlock {
                id,
                name,
                template,
                hardness,
                light,
                with_item,
            } => {
                validate_id(id)?;
                if g.blocks.contains_key(id) {
                    return Err(HorizonError::Duplicate(id.clone()));
                }
                let state_id = BlockStateId(g.next_id);
                g.next_id += 1;
                note_namespace(&mut g, id);
                let pb = PendingBlock {
                    id: id.clone(),
                    display_name: name.clone(),
                    template: template.clone().unwrap_or_else(|| "minecraft:stone".into()),
                    hardness: hardness.unwrap_or(1.5),
                    light: *light,
                    item: *with_item,
                };
                g.blocks.insert(id.clone(), (pb, state_id));
                log::debug!(
                    "[HorizonEngine] staged block {} state_id={} (horizon-mods schema)",
                    id,
                    state_id.0
                );
                Ok(EngineEvent::BlockRegistered {
                    id: id.clone(),
                    state_id: state_id.0,
                })
            }
            NativeCallback::RegisterItem {
                id,
                name,
                template,
                max_stack_size,
            } => {
                validate_id(id)?;
                if g.items.contains_key(id) {
                    return Err(HorizonError::Duplicate(id.clone()));
                }
                note_namespace(&mut g, id);
                g.items.insert(
                    id.clone(),
                    PendingItem {
                        id: id.clone(),
                        display_name: name.clone(),
                        template: template.clone().unwrap_or_else(|| "minecraft:stick".into()),
                        max_stack_size: *max_stack_size,
                    },
                );
                log::debug!("[HorizonEngine] staged item {}", id);
                Ok(EngineEvent::ItemRegistered { id: id.clone() })
            }
            NativeCallback::RegisterEntityType {
                id,
                template,
                item,
                damage,
            } => {
                validate_id(id)?;
                if g.entity_types.contains_key(id) {
                    return Err(HorizonError::Duplicate(id.clone()));
                }
                note_namespace(&mut g, id);
                let template = template.clone().unwrap_or_else(|| {
                    if item.is_some() {
                        "minecraft:snowball".into()
                    } else {
                        "minecraft:zombie".into()
                    }
                });
                g.entity_types.insert(
                    id.clone(),
                    PendingEntityType {
                        id: id.clone(),
                        template,
                        item: item.clone(),
                        damage: damage.unwrap_or(0.0),
                    },
                );
                Ok(EngineEvent::EntityTypeRegistered { id: id.clone() })
            }
            NativeCallback::RegisterMobEffect {
                id,
                name,
                color,
                category,
            } => {
                validate_id(id)?;
                if g.mob_effects.contains_key(id) {
                    return Err(HorizonError::Duplicate(id.clone()));
                }
                note_namespace(&mut g, id);
                let category = match category.to_ascii_lowercase().as_str() {
                    "beneficial" => "beneficial",
                    "harmful" => "harmful",
                    _ => "neutral",
                }
                .to_owned();
                g.mob_effects.insert(
                    id.clone(),
                    PendingMobEffect {
                        id: id.clone(),
                        display_name: name.clone(),
                        color: *color,
                        category,
                    },
                );
                Ok(EngineEvent::MobEffectRegistered { id: id.clone() })
            }
            NativeCallback::RegisterCreativeTab {
                id,
                title,
                icon,
                items,
            } => {
                validate_id(id)?;
                if g.creative_tabs.contains_key(id) {
                    return Err(HorizonError::Duplicate(id.clone()));
                }
                note_namespace(&mut g, id);
                g.creative_tabs.insert(
                    id.clone(),
                    PendingCreativeTab {
                        id: id.clone(),
                        title: title.clone(),
                        icon: icon.clone(),
                        items: items.clone(),
                    },
                );
                Ok(EngineEvent::CreativeTabRegistered { id: id.clone() })
            }
            NativeCallback::RegisterMenuType {
                id,
                slots,
                data_slots,
                results,
            } => {
                validate_id(id)?;
                if g.menu_types.contains_key(id) {
                    return Err(HorizonError::Duplicate(id.clone()));
                }
                note_namespace(&mut g, id);
                g.menu_types.insert(
                    id.clone(),
                    PendingMenuType {
                        id: id.clone(),
                        slots: *slots,
                        data_slots: *data_slots,
                        results: results.clone(),
                    },
                );
                Ok(EngineEvent::MenuTypeRegistered { id: id.clone() })
            }
            NativeCallback::RegisterCommand { name, description } => {
                g.commands.push((name.clone(), description.clone()));
                warn!(
                    "[HorizonEngine] command {name} recorded; commands need the native hook API, not content mods"
                );
                Ok(EngineEvent::CommandRecorded { name: name.clone() })
            }
            NativeCallback::LifecycleEvent { name, detail } => {
                g.lifecycle_events.push((name.clone(), detail.clone()));
                Ok(EngineEvent::Lifecycle { name: name.clone() })
            }
            NativeCallback::EntrypointComplete { mod_id } => {
                if !g.initialized_mods.contains(mod_id) {
                    g.initialized_mods.push(mod_id.clone());
                }
                Ok(EngineEvent::ModInitialized {
                    mod_id: mod_id.clone(),
                })
            }
        }
    }

    /// Validate a mod's complete registration batch against a private copy of
    /// the current engine state, then publish the copy atomically. If any
    /// registration is invalid or conflicts with committed content, none of
    /// this mod's state is visible to subsequent mods or content emission.
    pub fn apply_transaction(
        &self,
        mod_id: &str,
        provided_ids: &[String],
        callbacks: &[NativeCallback],
    ) -> Result<(), HorizonError> {
        let mut committed = self
            .inner
            .lock()
            .map_err(|_| HorizonError::Other("engine lock poisoned".into()))?;
        let staged = Self {
            inner: Arc::new(Mutex::new(committed.clone())),
        };
        let allowed_namespaces: BTreeSet<&str> = std::iter::once(mod_id)
            .chain(provided_ids.iter().map(String::as_str))
            .collect();

        for callback in callbacks {
            if let Some(id) = registration_id(callback)
                && !id
                    .split_once(':')
                    .is_some_and(|(namespace, _)| allowed_namespaces.contains(namespace))
            {
                return Err(HorizonError::Other(format!(
                    "mod '{mod_id}' cannot register id '{id}' outside its namespace or provided ids"
                )));
            }
            if let NativeCallback::EntrypointComplete {
                mod_id: callback_mod,
            } = callback
                && callback_mod != mod_id
            {
                return Err(HorizonError::Other(format!(
                    "mod '{mod_id}' reported completion for '{callback_mod}'"
                )));
            }
            staged.apply(callback)?;
            staged
                .inner
                .lock()
                .map_err(|_| HorizonError::Other("staged engine lock poisoned".into()))?
                .owned_callbacks
                .push((mod_id.to_owned(), callback.clone()));
        }
        staged.mark_initialized(mod_id);
        let staged = staged
            .inner
            .lock()
            .map_err(|_| HorizonError::Other("staged engine lock poisoned".into()))?;
        *committed = staged.clone();
        info!(
            "[HorizonEngine] committed {} registration callback(s) for '{mod_id}'",
            callbacks.len()
        );
        Ok(())
    }

    /// Mark a mod initialized once, whether or not its Java code called back.
    pub fn mark_initialized(&self, mod_id: &str) {
        let mut g = self.inner.lock().unwrap();
        if !g.initialized_mods.iter().any(|m| m == mod_id) {
            g.initialized_mods.push(mod_id.to_owned());
        }
    }

    /// Write one content-mod directory per namespace and return those roots.
    pub fn emit_content_mods(&self, out_root: &Path) -> Result<Vec<PathBuf>, HorizonError> {
        let g = self.inner.lock().unwrap();
        let mut roots = Vec::new();
        for ns in g.mod_namespaces.keys() {
            let mod_dir = out_root.join(ns);
            std::fs::create_dir_all(&mod_dir)?;

            let toml = format!(
                r#"id = "{ns}"
name = "{ns}"
version = "1.0.0"
description = "Generated by Java2Horizon"
authors = ["java2horizon"]
api = "0.2"
side = "both"
capabilities = ["registries"]
"#
            );
            std::fs::write(mod_dir.join("horizon-mod.toml"), toml)?;

            let belongs = |id: &str| id.starts_with(&format!("{ns}:"));

            for (pb, _) in g.blocks.values().filter(|(b, _)| belongs(&b.id)) {
                let path = path_of(&pb.id);
                let block_dir = mod_dir.join(format!("data/{ns}/horizon/block"));
                std::fs::create_dir_all(&block_dir)?;
                let mut props = json!({ "hardness": pb.hardness });
                if let Some(l) = pb.light {
                    props["light"] = json!(l);
                }
                let body = json!({
                    "template": pb.template,
                    "properties": props,
                    "item": pb.item,
                });
                write_json(&block_dir.join(format!("{path}.json")), &body)?;
            }

            for item in g.items.values().filter(|i| belongs(&i.id)) {
                let path = path_of(&item.id);
                let item_dir = mod_dir.join(format!("data/{ns}/horizon/item"));
                std::fs::create_dir_all(&item_dir)?;
                let mut body = json!({ "template": item.template });
                if let Some(s) = item.max_stack_size {
                    body["max_stack_size"] = json!(s);
                }
                write_json(&item_dir.join(format!("{path}.json")), &body)?;
            }

            for entity in g.entity_types.values().filter(|e| belongs(&e.id)) {
                let path = path_of(&entity.id);
                let dir = mod_dir.join(format!("data/{ns}/horizon/entity_type"));
                std::fs::create_dir_all(&dir)?;
                let mut body = json!({ "template": entity.template, "damage": entity.damage });
                if let Some(item) = &entity.item {
                    body["item"] = json!(item);
                }
                write_json(&dir.join(format!("{path}.json")), &body)?;
            }

            for effect in g.mob_effects.values().filter(|e| belongs(&e.id)) {
                let path = path_of(&effect.id);
                let dir = mod_dir.join(format!("data/{ns}/horizon/mob_effect"));
                std::fs::create_dir_all(&dir)?;
                let body = json!({ "category": effect.category, "color": effect.color });
                write_json(&dir.join(format!("{path}.json")), &body)?;
            }

            for tab in g.creative_tabs.values().filter(|t| belongs(&t.id)) {
                let path = path_of(&tab.id);
                let dir = mod_dir.join(format!("data/{ns}/horizon/creative_tab"));
                std::fs::create_dir_all(&dir)?;
                let body = json!({
                    "title": tab.title,
                    "icon": tab.icon,
                    "items": tab.items,
                });
                write_json(&dir.join(format!("{path}.json")), &body)?;
            }

            for menu in g.menu_types.values().filter(|m| belongs(&m.id)) {
                let path = path_of(&menu.id);
                let dir = mod_dir.join(format!("data/{ns}/horizon/menu"));
                std::fs::create_dir_all(&dir)?;
                let body = json!({
                    "slots": menu.slots,
                    "data_slots": menu.data_slots,
                    "results": menu.results,
                });
                write_json(&dir.join(format!("{path}.json")), &body)?;
            }

            let lang_dir = mod_dir.join(format!("assets/{ns}/lang"));
            std::fs::create_dir_all(&lang_dir)?;
            let mut lang = BTreeMap::new();
            for (pb, _) in g.blocks.values().filter(|(b, _)| belongs(&b.id)) {
                lang.insert(
                    format!("block.{}", pb.id.replace(':', ".")),
                    pb.display_name.clone(),
                );
            }
            for item in g.items.values().filter(|i| belongs(&i.id)) {
                lang.insert(
                    format!("item.{}", item.id.replace(':', ".")),
                    item.display_name.clone(),
                );
            }
            for effect in g.mob_effects.values().filter(|e| belongs(&e.id)) {
                lang.insert(
                    format!("effect.{}", effect.id.replace(':', ".")),
                    effect.display_name.clone(),
                );
            }
            for entity in g.entity_types.values().filter(|e| belongs(&e.id)) {
                lang.insert(
                    format!("entity.{}", entity.id.replace(':', ".")),
                    path_of(&entity.id).replace('_', " "),
                );
            }
            for tab in g.creative_tabs.values().filter(|t| belongs(&t.id)) {
                lang.insert(
                    format!("itemGroup.{}", tab.id.replace(':', ".")),
                    tab.title.clone(),
                );
            }
            write_json(&lang_dir.join("en_us.json"), &json!(lang))?;

            roots.push(mod_dir);
        }
        Ok(roots)
    }

    /// Validate each content-mod root with Horizon's own content loader.
    pub fn validate_with_horizon_mods(&self, roots: &[PathBuf]) -> Result<(), HorizonError> {
        for root in roots {
            let manifest_path = root.join("horizon-mod.toml");
            let text = std::fs::read_to_string(&manifest_path)?;
            let manifest = Manifest::parse(&text)
                .map_err(|e| HorizonError::HorizonMods(format!("{}: {e}", root.display())))?;
            let content = Content::read(root, &manifest)
                .map_err(|e| HorizonError::HorizonMods(format!("{}: {e}", root.display())))?;
            info!(
                "[HorizonEngine] horizon-mods Content::read OK for '{}' — {} block(s), {} item(s), \
                 {} entity type(s), {} effect(s), {} tab(s), {} menu(s)",
                manifest.id,
                content.blocks.len(),
                content.items.len(),
                content.entity_types.len(),
                content.effects.len(),
                content.creative_tabs.len(),
                content.menu_types.len()
            );
            let _: Option<&horizon_mods::content::Block> = content.blocks.first();
            let _ = Rarity::Common;
            let _ = Side::Both;
            let _ = Capability::Registries;
        }
        Ok(())
    }

    pub fn vanilla_base(&self) -> u32 {
        self.inner.lock().unwrap().vanilla_base
    }
    pub fn block_count(&self) -> usize {
        self.inner.lock().unwrap().blocks.len()
    }
    pub fn item_count(&self) -> usize {
        self.inner.lock().unwrap().items.len()
    }
    pub fn entity_type_count(&self) -> usize {
        self.inner.lock().unwrap().entity_types.len()
    }
    pub fn mob_effect_count(&self) -> usize {
        self.inner.lock().unwrap().mob_effects.len()
    }
    pub fn has_block(&self, id: &str) -> bool {
        self.inner.lock().unwrap().blocks.contains_key(id)
    }
    pub fn has_item(&self, id: &str) -> bool {
        self.inner.lock().unwrap().items.contains_key(id)
    }
    pub fn has_entity_type(&self, id: &str) -> bool {
        self.inner.lock().unwrap().entity_types.contains_key(id)
    }
    pub fn has_mob_effect(&self, id: &str) -> bool {
        self.inner.lock().unwrap().mob_effects.contains_key(id)
    }
    pub fn state_id(&self, id: &str) -> Option<u32> {
        self.inner.lock().unwrap().blocks.get(id).map(|(_, s)| s.0)
    }
    pub fn initialized_mods(&self) -> Vec<String> {
        self.inner.lock().unwrap().initialized_mods.clone()
    }
    pub fn registration_owners(&self) -> Vec<(String, NativeCallback)> {
        self.inner.lock().unwrap().owned_callbacks.clone()
    }
    pub fn log_lines(&self) -> Vec<String> {
        self.inner.lock().unwrap().log.clone()
    }
    pub fn commands(&self) -> Vec<(String, String)> {
        self.inner.lock().unwrap().commands.clone()
    }
    pub fn namespaces(&self) -> BTreeSet<String> {
        self.inner
            .lock()
            .unwrap()
            .mod_namespaces
            .keys()
            .cloned()
            .collect()
    }
}

fn registration_id(callback: &NativeCallback) -> Option<&str> {
    match callback {
        NativeCallback::RegisterBlock { id, .. }
        | NativeCallback::RegisterItem { id, .. }
        | NativeCallback::RegisterEntityType { id, .. }
        | NativeCallback::RegisterMobEffect { id, .. }
        | NativeCallback::RegisterCreativeTab { id, .. }
        | NativeCallback::RegisterMenuType { id, .. } => Some(id),
        NativeCallback::Message(_)
        | NativeCallback::RegisterCommand { .. }
        | NativeCallback::LifecycleEvent { .. }
        | NativeCallback::EntrypointComplete { .. } => None,
    }
}

impl Default for HorizonEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn note_namespace(g: &mut EngineInner, id: &str) {
    if let Some((ns, _)) = id.split_once(':') {
        g.mod_namespaces
            .entry(ns.to_string())
            .or_insert_with(|| ns.to_string());
    }
}

fn path_of(id: &str) -> &str {
    id.split_once(':').map(|(_, p)| p).unwrap_or(id)
}

fn write_json(path: &Path, value: &Value) -> Result<(), HorizonError> {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap())?;
    Ok(())
}

fn validate_id(id: &str) -> Result<(), HorizonError> {
    let (ns, path) = id
        .split_once(':')
        .ok_or_else(|| HorizonError::InvalidId(id.into()))?;
    if ns.is_empty()
        || path.is_empty()
        || !ns
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        || !path
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/')
    {
        return Err(HorizonError::InvalidId(id.into()));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    BlockRegistered { id: String, state_id: u32 },
    ItemRegistered { id: String },
    EntityTypeRegistered { id: String },
    MobEffectRegistered { id: String },
    CreativeTabRegistered { id: String },
    MenuTypeRegistered { id: String },
    CommandRecorded { name: String },
    Lifecycle { name: String },
    Log(String),
    ModInitialized { mod_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str) -> NativeCallback {
        NativeCallback::RegisterBlock {
            id: id.into(),
            name: "Example".into(),
            template: Some("minecraft:stone".into()),
            hardness: Some(2.0),
            light: Some(7),
            with_item: true,
        }
    }

    #[test]
    fn register_and_validate_with_horizon_mods() {
        let engine = HorizonEngine::new();
        engine.apply(&block("testmod:example_block")).unwrap();
        engine
            .apply(&NativeCallback::RegisterItem {
                id: "testmod:example_item".into(),
                name: "Example Item".into(),
                template: Some("minecraft:stick".into()),
                max_stack_size: Some(16),
            })
            .unwrap();
        engine
            .apply(&NativeCallback::RegisterEntityType {
                id: "testmod:example_bolt".into(),
                template: None,
                item: Some("testmod:example_item".into()),
                damage: Some(2.0),
            })
            .unwrap();
        engine
            .apply(&NativeCallback::RegisterMobEffect {
                id: "testmod:glow".into(),
                name: "Glow".into(),
                color: 0x44ff88,
                category: "beneficial".into(),
            })
            .unwrap();
        engine
            .apply(&NativeCallback::RegisterCreativeTab {
                id: "testmod:tab".into(),
                title: "Test".into(),
                icon: "testmod:example_item".into(),
                items: vec!["testmod:example_item".into()],
            })
            .unwrap();
        engine
            .apply(&NativeCallback::RegisterMenuType {
                id: "testmod:machine".into(),
                slots: 9,
                data_slots: 2,
                results: vec![8],
            })
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let roots = engine.emit_content_mods(dir.path()).unwrap();
        assert_eq!(roots.len(), 1);
        engine.validate_with_horizon_mods(&roots).unwrap();
        assert!(engine.has_block("testmod:example_block"));
        assert!(engine.has_item("testmod:example_item"));
        assert!(engine.has_entity_type("testmod:example_bolt"));
        assert!(engine.has_mob_effect("testmod:glow"));
        assert!(engine.state_id("testmod:example_block").unwrap() >= 22_000);
    }

    #[test]
    fn rejects_non_vanilla_templates_before_horizon_sees_them() {
        let engine = HorizonEngine::new();
        engine.apply(&block("testmod:bad")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let roots = engine.emit_content_mods(dir.path()).unwrap();
        let path = roots[0].join("data/testmod/horizon/block/bad.json");
        std::fs::write(
            &path,
            r#"{"template":"not_a_template","properties":{"hardness":1.0},"item":true}"#,
        )
        .unwrap();
        assert!(engine.validate_with_horizon_mods(&roots).is_err());
    }

    #[test]
    fn a_failed_registration_batch_does_not_publish_earlier_entries() {
        let engine = HorizonEngine::new();
        let callbacks = [
            block("broken:first"),
            NativeCallback::RegisterItem {
                id: "invalid-id".into(),
                name: "Bad item".into(),
                template: None,
                max_stack_size: None,
            },
        ];

        assert!(engine.apply_transaction("broken", &[], &callbacks).is_err());
        assert!(!engine.has_block("broken:first"));
        assert_eq!(engine.block_count(), 0);
        assert!(engine.initialized_mods().is_empty());
    }

    #[test]
    fn successful_registration_batch_commits_once_and_records_its_owner() {
        let engine = HorizonEngine::new();
        engine
            .apply_transaction(
                "working",
                &[],
                &[
                    block("working:first"),
                    NativeCallback::Message("ready".into()),
                ],
            )
            .unwrap();

        assert!(engine.has_block("working:first"));
        assert_eq!(engine.initialized_mods(), vec!["working"]);
        assert_eq!(engine.log_lines(), vec!["ready"]);
        assert_eq!(engine.registration_owners().len(), 2);
        assert!(
            engine
                .registration_owners()
                .iter()
                .all(|(owner, _)| owner == "working")
        );
    }

    #[test]
    fn one_mod_cannot_register_content_under_another_mod_namespace() {
        let engine = HorizonEngine::new();
        assert!(
            engine
                .apply_transaction("first", &[], &[block("second:stolen")])
                .is_err()
        );
        assert!(!engine.has_block("second:stolen"));
        assert!(engine.registration_owners().is_empty());
    }
}
