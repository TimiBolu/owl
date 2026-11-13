use super::delta::RecordPageDelta;
use super::ObjRef;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const RECORD_SHARDS: usize = 256;

/// A small persistent record directory.
///
/// The root is a fixed fan-out table of immutable ordered maps. Updating one
/// object copies only its shard and reuses every other shard and record blob.
/// Capturing a snapshot clones 256 `Arc`s, independent of the object count.
pub(crate) struct CowRecordDirectory {
    shards: Vec<Arc<BTreeMap<ObjRef, Arc<[u8]>>>>,
    len: usize,
    dirty: BTreeSet<ObjRef>,
}

#[derive(Clone)]
pub(crate) struct RecordSnapshot {
    shards: Vec<Arc<BTreeMap<ObjRef, Arc<[u8]>>>>,
}

impl Default for CowRecordDirectory {
    fn default() -> Self {
        Self {
            shards: (0..RECORD_SHARDS)
                .map(|_| Arc::new(BTreeMap::new()))
                .collect(),
            len: 0,
            dirty: BTreeSet::new(),
        }
    }
}

impl CowRecordDirectory {
    pub(crate) fn get(&self, object_ref: &ObjRef) -> Option<&Arc<[u8]>> {
        self.shards[shard_for(*object_ref)].get(object_ref)
    }

    pub(crate) fn insert(
        &mut self,
        object_ref: ObjRef,
        record: impl Into<Arc<[u8]>>,
    ) -> Option<Arc<[u8]>> {
        let shard = Arc::make_mut(&mut self.shards[shard_for(object_ref)]);
        let previous = shard.insert(object_ref, record.into());
        if previous.is_none() {
            self.len += 1;
        }
        self.dirty.insert(object_ref);
        previous
    }

    pub(crate) fn remove(&mut self, object_ref: &ObjRef) -> Option<Arc<[u8]>> {
        let shard = Arc::make_mut(&mut self.shards[shard_for(*object_ref)]);
        let previous = shard.remove(object_ref);
        if previous.is_some() {
            self.len -= 1;
            self.dirty.insert(*object_ref);
        }
        previous
    }

    pub(crate) fn remove_known(&mut self, object_ref: &ObjRef) -> Option<Arc<[u8]>> {
        let previous = self.remove(object_ref);
        self.dirty.insert(*object_ref);
        previous
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_dirty(&self, object_ref: &ObjRef) -> bool {
        self.dirty.contains(object_ref)
    }

    pub(crate) fn snapshot(&self) -> RecordSnapshot {
        RecordSnapshot {
            shards: self.shards.clone(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn take_delta(&mut self, base_lsn: u64, target_lsn: u64) -> RecordPageDelta {
        let dirty = std::mem::take(&mut self.dirty);
        let mut puts = Vec::new();
        let mut deletes = Vec::new();
        for object_ref in dirty {
            match self.get(&object_ref) {
                Some(record) => puts.push((object_ref, record.clone())),
                None => deletes.push(object_ref),
            }
        }
        RecordPageDelta {
            base_lsn,
            target_lsn,
            puts,
            deletes,
        }
    }

    pub(crate) fn mark_clean(&mut self) {
        self.dirty.clear();
    }

    pub(crate) fn evict_clean(&mut self) {
        let dirty = &self.dirty;
        let mut len = 0;
        for shard in &mut self.shards {
            Arc::make_mut(shard).retain(|object_ref, _| dirty.contains(object_ref));
            len += shard.len();
        }
        self.len = len;
    }
}

impl RecordSnapshot {
    pub(crate) fn records(&self) -> Vec<(ObjRef, Arc<[u8]>)> {
        let mut records = self
            .shards
            .iter()
            .flat_map(|shard| {
                shard
                    .iter()
                    .map(|(object_ref, record)| (*object_ref, record.clone()))
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|(object_ref, _)| object_ref.packed());
        records
    }
}

fn shard_for(object_ref: ObjRef) -> usize {
    let mut value = object_ref.packed();
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    value as usize & (RECORD_SHARDS - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_keep_old_records_while_updates_copy_one_shard() {
        let first = ObjRef::from_parts(1, 1, 1);
        let second = ObjRef::from_parts(1, 2, 1);
        let mut directory = CowRecordDirectory::default();
        directory.insert(first, Vec::from(&b"old"[..]));
        directory.insert(second, Vec::from(&b"stable"[..]));
        let snapshot = directory.snapshot();

        directory.insert(first, Vec::from(&b"new"[..]));
        directory.remove(&second);

        let records = snapshot.records().into_iter().collect::<BTreeMap<_, _>>();
        assert_eq!(records[&first].as_ref(), b"old");
        assert_eq!(records[&second].as_ref(), b"stable");
        assert_eq!(directory.get(&first).unwrap().as_ref(), b"new");
        assert!(directory.get(&second).is_none());
    }

    #[test]
    fn snapshot_length_and_identity_are_stable() {
        let mut directory = CowRecordDirectory::default();
        for object_id in 1..=1_000 {
            directory.insert(
                ObjRef::from_parts(7, object_id, 1),
                object_id.to_le_bytes().to_vec(),
            );
        }
        let snapshot = directory.snapshot();
        assert_eq!(snapshot.records().len(), 1_000);
    }

    #[test]
    fn delta_capture_coalesces_updates_and_emits_tombstones() {
        let retained = ObjRef::from_parts(1, 1, 1);
        let deleted = ObjRef::from_parts(1, 2, 1);
        let mut directory = CowRecordDirectory::default();
        directory.insert(retained, Vec::from(&b"old"[..]));
        directory.insert(deleted, Vec::from(&b"gone"[..]));
        directory.mark_clean();
        directory.insert(retained, Vec::from(&b"intermediate"[..]));
        directory.insert(retained, Vec::from(&b"final"[..]));
        directory.remove(&deleted);

        let delta = directory.take_delta(4, 7);
        assert_eq!(delta.puts.len(), 1);
        assert_eq!(delta.puts[0].0, retained);
        assert_eq!(delta.puts[0].1.as_ref(), b"final");
        assert_eq!(delta.deletes, vec![deleted]);
        let empty = directory.take_delta(7, 7);
        assert!(empty.puts.is_empty() && empty.deletes.is_empty());
    }

    #[test]
    fn clean_records_can_be_evicted_while_dirty_bytes_and_lazy_deletes_survive() {
        let clean = ObjRef::from_parts(1, 1, 1);
        let dirty = ObjRef::from_parts(1, 2, 1);
        let deleted_lazy = ObjRef::from_parts(1, 3, 1);
        let mut directory = CowRecordDirectory::default();
        directory.insert(clean, Vec::from(&b"clean"[..]));
        directory.mark_clean();
        directory.insert(dirty, Vec::from(&b"dirty"[..]));
        directory.remove_known(&deleted_lazy);

        directory.evict_clean();
        assert!(directory.get(&clean).is_none());
        assert_eq!(directory.get(&dirty).unwrap().as_ref(), b"dirty");
        let delta = directory.take_delta(1, 2);
        assert_eq!(delta.puts[0].0, dirty);
        assert_eq!(delta.deletes, vec![deleted_lazy]);
    }
}
