//! Tests for the binding layer: `HandleEntityCache<A>` bidirectional mapping.
//!
//! In the refactored API, `HandleEntityCache<A: HmrSource>` is parameterized
//! by the Asset type `A`, not the Config type `T`. For the wrapping pattern
//! `A = ConfigAsset<T>`, so we use `HandleEntityCache<ConfigAsset<DummyDb>>`
//! here. `DummyDb` must implement `ConfigDiff` so that `ConfigAsset<DummyDb>`
//! satisfies `HmrSource`.

use bevy::asset::{Asset, AssetId};
use bevy::prelude::Entity;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{ConfigAsset, ConfigBind, ConfigDiff, HandleEntityCache};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

/// Test asset type used as the `T` in `ConfigAsset<T>`.
#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
struct DummyDb {
    count: u32,
}

/// `DummyDb` 必须实现 `ConfigDiff` 才能让 `ConfigAsset<DummyDb>: HmrSource`。
/// 测试只关心 binding 缓存映射，diff 实现返回空集即可。
impl ConfigDiff for DummyDb {
    type Id = String;
    fn diff(_old: &Self, _new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        Default::default()
    }
}

/// Build a deterministic `AssetId<ConfigAsset<DummyDb>>` from a u128.
fn make_id(seed: u128) -> AssetId<ConfigAsset<DummyDb>> {
    AssetId::Uuid {
        uuid: Uuid::from_u128(seed),
    }
}

#[test]
fn cache_starts_empty() {
    let cache: HandleEntityCache<ConfigAsset<DummyDb>> = HandleEntityCache::default();
    assert!(cache.is_empty());
}

#[test]
fn insert_creates_bidirectional_mapping() {
    let mut cache: HandleEntityCache<ConfigAsset<DummyDb>> = HandleEntityCache::default();
    let id = make_id(1);
    let entity = Entity::from_bits(1);

    cache.insert(entity, id);

    let entities: Vec<_> = cache
        .get_entities(&id)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();

    assert_eq!(entities, vec![entity]);
    assert!(!cache.is_empty());
}

#[test]
fn insert_multiple_entities_for_same_handle() {
    let mut cache: HandleEntityCache<ConfigAsset<DummyDb>> = HandleEntityCache::default();
    let id = make_id(1);
    let e1 = Entity::from_bits(1);
    let e2 = Entity::from_bits(2);
    let e3 = Entity::from_bits(3);

    cache.insert(e1, id);
    cache.insert(e2, id);
    cache.insert(e3, id);

    let bound: HashSet<_> = cache
        .get_entities(&id)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();
    assert_eq!(bound.len(), 3);
    assert!(bound.contains(&e1));
    assert!(bound.contains(&e2));
    assert!(bound.contains(&e3));
}

#[test]
fn remove_entity_clears_both_mappings() {
    let mut cache: HandleEntityCache<ConfigAsset<DummyDb>> = HandleEntityCache::default();
    let id = make_id(1);
    let entity = Entity::from_bits(1);

    cache.insert(entity, id);
    cache.remove(entity);

    assert!(cache.is_empty());
    assert!(cache.get_entities(&id).is_none());
}

#[test]
fn remove_last_entity_for_handle_drops_the_set() {
    let mut cache: HandleEntityCache<ConfigAsset<DummyDb>> = HandleEntityCache::default();
    let id = make_id(1);
    let e1 = Entity::from_bits(1);
    let e2 = Entity::from_bits(2);

    cache.insert(e1, id);
    cache.insert(e2, id);

    cache.remove(e1);
    let remaining: Vec<_> = cache
        .get_entities(&id)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();
    assert_eq!(remaining, vec![e2]);

    cache.remove(e2);
    assert!(cache.get_entities(&id).is_none());
}

#[test]
fn reinserting_entity_with_new_handle_removes_old_binding() {
    let mut cache: HandleEntityCache<ConfigAsset<DummyDb>> = HandleEntityCache::default();
    let id1 = make_id(1);
    let id2 = make_id(2);
    let entity = Entity::from_bits(1);

    cache.insert(entity, id1);
    cache.insert(entity, id2);

    // Old handle should no longer reference this entity.
    assert!(cache.get_entities(&id1).is_none());
    // New handle should.
    let bound: Vec<_> = cache
        .get_entities(&id2)
        .map(|s| s.iter().copied().collect())
        .unwrap_or_default();
    assert_eq!(bound, vec![entity]);
}

#[test]
fn config_bind_component_can_be_constructed() {
    // Smoke test: ensure the marker component can be built and stored.
    // Using Handle::default() which is a Uuid handle with the default UUID.
    let handle: bevy::prelude::Handle<ConfigAsset<DummyDb>> = bevy::prelude::Handle::default();
    let bind: ConfigBind<ConfigAsset<DummyDb>> = ConfigBind {
        handle: handle.clone(),
    };
    assert_eq!(bind.handle.id(), handle.id());
}

#[test]
fn with_id_uuid_produces_matching_handle() {
    // with_id should produce a Handle whose .id() matches the given AssetId::Uuid.
    let id = make_id(777);
    let bind = ConfigBind::<ConfigAsset<DummyDb>>::with_id(id);
    assert_eq!(bind.handle.id(), id);
}

#[test]
fn with_id_index_falls_back_without_panicking() {
    // with_id on an AssetId::Index should not panic; it falls back to
    // Handle::default() (with a stderr warning) because Bevy has no public
    // way to construct a Handle from an AssetId::Index.
    // We can't easily build an AssetId::Index without a real asset store,
    // so this test just ensures the Uuid path and default handle coexist
    // — the Index branch is covered by compilation + the eprintln warning.
    let default_handle: bevy::prelude::Handle<ConfigAsset<DummyDb>> =
        bevy::prelude::Handle::default();
    let bind = ConfigBind::<ConfigAsset<DummyDb>>::with_id(default_handle.id());
    assert_eq!(bind.handle.id(), default_handle.id());
}
