//! Revision-based routing for active and inactive config views.
//!
//! A view entity opts in by carrying both [`crate::ConfigBind<A>`] and
//! [`AppliedRevision<A>`]. Add [`ActiveConfigView`] while the page is active.
//! Active views receive [`ConfigViewSync<A>`] after a config state change;
//! inactive views keep their old applied revision and receive one sync with
//! the final state when activated later.

use crate::{ConfigRefresh, ConfigReloadFailed, ConfigRemoved, HmrSource};
use bevy::asset::{AssetId, Handle};
use bevy::ecs::message::{Message, MessageReader, MessageWriter};
use bevy::prelude::{Added, Component, Entity, Has, Query, Res, Resource};
use std::collections::{HashMap, HashSet};

/// Current lifecycle state for one config asset revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetRevisionStatus {
    Available,
    Removed,
    LoadFailed { source_path: String, error: String },
}

/// Revision and lifecycle state for one asset id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRevisionEntry {
    pub revision: u64,
    pub status: AssetRevisionStatus,
}

/// Per-asset monotonic revision registry for one HMR asset type.
#[derive(Resource)]
pub struct AssetRevision<A: HmrSource> {
    entries: HashMap<AssetId<A>, AssetRevisionEntry>,
    next_revision: u64,
}

impl<A: HmrSource> Default for AssetRevision<A> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            next_revision: 0,
        }
    }
}

impl<A: HmrSource> AssetRevision<A> {
    pub fn get(&self, id: AssetId<A>) -> Option<&AssetRevisionEntry> {
        self.entries.get(&id)
    }

    pub fn revision(&self, id: AssetId<A>) -> Option<u64> {
        self.get(id).map(|entry| entry.revision)
    }

    pub fn status(&self, id: AssetId<A>) -> Option<&AssetRevisionStatus> {
        self.get(id).map(|entry| &entry.status)
    }

    pub(crate) fn record_available(&mut self, id: AssetId<A>) -> u64 {
        self.record(id, AssetRevisionStatus::Available)
    }

    pub(crate) fn record_removed(&mut self, id: AssetId<A>) -> u64 {
        self.record(id, AssetRevisionStatus::Removed)
    }

    pub(crate) fn record_failed(
        &mut self,
        id: AssetId<A>,
        source_path: String,
        error: String,
    ) -> u64 {
        self.record(id, AssetRevisionStatus::LoadFailed { source_path, error })
    }

    fn record(&mut self, id: AssetId<A>, status: AssetRevisionStatus) -> u64 {
        self.next_revision = self.next_revision.saturating_add(1);
        self.entries.insert(
            id,
            AssetRevisionEntry {
                revision: self.next_revision,
                status,
            },
        );
        self.next_revision
    }
}

/// Revision last applied by a derived view.
#[derive(Component, Clone, Debug)]
pub struct AppliedRevision<A: HmrSource> {
    pub handle: Handle<A>,
    pub revision: u64,
}

impl<A: HmrSource> AppliedRevision<A> {
    pub fn new(handle: impl Into<Handle<A>>) -> Self {
        Self {
            handle: handle.into(),
            revision: 0,
        }
    }

    pub fn asset_id(&self) -> AssetId<A> {
        self.handle.id()
    }

    pub fn is_stale(&self, revisions: &AssetRevision<A>) -> bool {
        revisions
            .revision(self.asset_id())
            .is_some_and(|revision| revision > self.revision)
    }
}

/// Marker for a page/view that should be synchronized immediately.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ActiveConfigView;

/// Lightweight request to rebuild one active view from current `Assets<A>`.
#[derive(Message)]
pub struct ConfigViewSync<A: HmrSource> {
    pub entity: Entity,
    pub asset_id: AssetId<A>,
    pub revision: u64,
    pub status: AssetRevisionStatus,
}

impl<A: HmrSource> Clone for ConfigViewSync<A> {
    fn clone(&self) -> Self {
        Self {
            entity: self.entity,
            asset_id: self.asset_id,
            revision: self.revision,
            status: self.status.clone(),
        }
    }
}

impl<A: HmrSource> ConfigViewSync<A> {
    /// Resolve the current asset without cloning it into the sync message.
    /// Returns `None` for a removed asset.
    pub fn asset<'a>(&self, assets: &'a bevy::asset::Assets<A>) -> Option<&'a A> {
        assets.get(self.asset_id)
    }
}

/// Route refresh/removal/failure messages only to currently active views.
pub fn route_active_config_views<A: HmrSource>(
    mut refreshes: MessageReader<ConfigRefresh<A::Config>>,
    mut removed: MessageReader<ConfigRemoved<A::Config>>,
    mut failed: MessageReader<ConfigReloadFailed<A::Config>>,
    revisions: Res<AssetRevision<A>>,
    mut views: Query<(&mut AppliedRevision<A>, Has<ActiveConfigView>)>,
    mut syncs: MessageWriter<ConfigViewSync<A>>,
) {
    let mut pending = HashSet::new();
    for event in refreshes.read() {
        pending.extend(
            event
                .target_entities
                .iter()
                .copied()
                .map(|entity| (entity, event.asset_id)),
        );
    }
    for event in removed.read() {
        pending.extend(
            event
                .target_entities
                .iter()
                .copied()
                .map(|entity| (entity, event.asset_id)),
        );
    }
    for event in failed.read() {
        pending.extend(
            event
                .target_entities
                .iter()
                .copied()
                .map(|entity| (entity, event.asset_id)),
        );
    }

    for (entity, event_asset_id) in pending {
        let Ok((mut applied, active)) = views.get_mut(entity) else {
            continue;
        };
        let asset_id = applied.asset_id();
        if !active || asset_id.untyped() != event_asset_id {
            continue;
        }
        emit_if_stale(entity, &mut applied, &revisions, &mut syncs);
    }
}

/// Synchronize a view once when `ActiveConfigView` is newly inserted.
pub fn sync_activated_config_views<A: HmrSource>(
    revisions: Res<AssetRevision<A>>,
    mut views: Query<(Entity, &mut AppliedRevision<A>), Added<ActiveConfigView>>,
    mut syncs: MessageWriter<ConfigViewSync<A>>,
) {
    for (entity, mut applied) in views.iter_mut() {
        emit_if_stale(entity, &mut applied, &revisions, &mut syncs);
    }
}

fn emit_if_stale<A: HmrSource>(
    entity: Entity,
    applied: &mut AppliedRevision<A>,
    revisions: &AssetRevision<A>,
    syncs: &mut MessageWriter<ConfigViewSync<A>>,
) {
    let asset_id = applied.asset_id();
    let Some(entry) = revisions.get(asset_id) else {
        return;
    };
    if entry.revision <= applied.revision {
        return;
    }

    applied.revision = entry.revision;
    syncs.write(ConfigViewSync {
        entity,
        asset_id,
        revision: entry.revision,
        status: entry.status.clone(),
    });
}
