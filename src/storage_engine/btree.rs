use crate::storage_engine::ObjRef;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::sync::{Arc, RwLock};

pub(crate) trait OrderedIndexBase: Send + Sync {
    fn search_key(&self, key: &[u8]) -> Result<Vec<ObjRef>, String>;
    fn range_entries(
        &self,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
        limit: Option<usize>,
        descending: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<ObjRef>)>, String>;
    fn visit_entries(
        &self,
        visitor: &mut dyn FnMut(&[u8], &[ObjRef]) -> Result<(), String>,
    ) -> Result<(), String>;
}

/// Ordered postings index.
///
/// Keys are versioned memcomparable bytes produced by `TypedKey`. A key maps to
/// an ordered set of stable object identities, so non-unique values never
/// overwrite one another. The `u64` methods remain as a compatibility surface
/// for the legacy query layer while it migrates to typed keys.
#[derive(Default)]
pub struct BTreeIndex {
    inner: RwLock<BTreeIndexInner>,
}

#[derive(Default)]
struct BTreeIndexInner {
    entries: BTreeMap<Vec<u8>, BTreeSet<ObjRef>>,
    tombstones: BTreeSet<Vec<u8>>,
    dirty: BTreeSet<Vec<u8>>,
    base: Option<Arc<dyn OrderedIndexBase>>,
}

impl std::fmt::Debug for BTreeIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("BTreeIndex").finish_non_exhaustive()
    }
}

impl BTreeIndex {
    pub fn create(_order: usize, _flags: u32) -> Result<Self, String> {
        Ok(Self::default())
    }

    pub(crate) fn with_base(base: Arc<dyn OrderedIndexBase>) -> Self {
        Self {
            inner: RwLock::new(BTreeIndexInner {
                base: Some(base),
                ..BTreeIndexInner::default()
            }),
        }
    }

    pub(crate) fn replace_base_preserving_dirty(
        &self,
        base: Arc<dyn OrderedIndexBase>,
    ) -> Result<(), String> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?;
        let dirty = inner.dirty.clone();
        inner.entries.retain(|key, _| dirty.contains(key));
        inner.tombstones.retain(|key| dirty.contains(key));
        inner.base = Some(base);
        Ok(())
    }

    pub(crate) fn clear_overlay(&self) -> Result<(), String> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?;
        inner.entries.clear();
        inner.tombstones.clear();
        inner.dirty.clear();
        Ok(())
    }

    pub(crate) fn install_overlay_key(
        &self,
        key: Vec<u8>,
        postings: Option<Vec<ObjRef>>,
    ) -> Result<(), String> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?;
        inner.entries.remove(&key);
        inner.tombstones.remove(&key);
        match postings {
            Some(postings) => {
                inner.entries.insert(key, postings.into_iter().collect());
            }
            None => {
                inner.tombstones.insert(key);
            }
        }
        Ok(())
    }

    pub(crate) fn visit_entries(
        &self,
        visitor: &mut dyn FnMut(&[u8], &[ObjRef]) -> Result<(), String>,
    ) -> Result<(), String> {
        let (base, overlays) = {
            let inner = self
                .inner
                .read()
                .map_err(|_| "B-tree read lock poisoned".to_string())?;
            let mut overlays = inner
                .entries
                .iter()
                .map(|(key, postings)| {
                    (
                        key.clone(),
                        Some(postings.iter().copied().collect::<Vec<_>>()),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            for key in &inner.tombstones {
                overlays.insert(key.clone(), None);
            }
            (inner.base.clone(), overlays)
        };
        let mut overlays = overlays.into_iter().peekable();
        if let Some(base) = base {
            base.visit_entries(&mut |base_key, base_postings| {
                while overlays
                    .peek()
                    .is_some_and(|(overlay_key, _)| overlay_key.as_slice() < base_key)
                {
                    let (key, postings) = overlays.next().unwrap();
                    visit_overlay(visitor, key, postings)?;
                }
                if overlays
                    .peek()
                    .is_some_and(|(overlay_key, _)| overlay_key.as_slice() == base_key)
                {
                    let (key, postings) = overlays.next().unwrap();
                    visit_overlay(visitor, key, postings)
                } else {
                    visitor(base_key, base_postings)
                }
            })?;
        }
        for (key, postings) in overlays {
            visit_overlay(visitor, key, postings)?;
        }
        Ok(())
    }

    pub fn insert_key(&self, key: Vec<u8>, value: ObjRef, unique: bool) -> Result<(), String> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?;
        if !inner.entries.contains_key(&key) && !inner.tombstones.contains(&key) {
            let base_postings = inner
                .base
                .as_ref()
                .map(|base| base.search_key(&key))
                .transpose()?
                .unwrap_or_default();
            if !base_postings.is_empty() {
                inner
                    .entries
                    .insert(key.clone(), base_postings.into_iter().collect());
            }
        }
        inner.tombstones.remove(&key);
        let postings = inner.entries.entry(key.clone()).or_default();
        if unique && !postings.is_empty() && !postings.contains(&value) {
            return Err("unique index key already belongs to another object".to_string());
        }
        if postings.insert(value) {
            inner.dirty.insert(key);
        }
        Ok(())
    }

    pub fn search_key(&self, key: &[u8]) -> Result<Vec<ObjRef>, String> {
        let inner = self
            .inner
            .read()
            .map_err(|_| "B-tree read lock poisoned".to_string())?;
        if let Some(postings) = inner.entries.get(key) {
            return Ok(postings.iter().copied().collect());
        }
        if inner.tombstones.contains(key) {
            return Ok(Vec::new());
        }
        inner
            .base
            .as_ref()
            .map(|base| base.search_key(key))
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    pub fn delete_key(&self, key: &[u8], value: ObjRef) -> Result<bool, String> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?;
        if !inner.entries.contains_key(key) && !inner.tombstones.contains(key) {
            let base_postings = inner
                .base
                .as_ref()
                .map(|base| base.search_key(key))
                .transpose()?
                .unwrap_or_default();
            if !base_postings.is_empty() {
                inner
                    .entries
                    .insert(key.to_vec(), base_postings.into_iter().collect());
            }
        }
        let Some(postings) = inner.entries.get_mut(key) else {
            return Ok(false);
        };
        let removed = postings.remove(&value);
        if postings.is_empty() {
            inner.entries.remove(key);
            inner.tombstones.insert(key.to_vec());
        }
        if removed {
            inner.dirty.insert(key.to_vec());
        }
        Ok(removed)
    }

    pub fn range_keys(&self, start: &[u8], end: &[u8]) -> Result<Vec<ObjRef>, String> {
        if start > end {
            return Ok(Vec::new());
        }
        Ok(self
            .merged_entries(
                Bound::Included(start.to_vec()),
                Bound::Included(end.to_vec()),
                None,
                false,
            )?
            .into_iter()
            .flat_map(|(_, postings)| postings.into_iter())
            .collect())
    }

    pub fn range_key_bounds(
        &self,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
    ) -> Result<Vec<ObjRef>, String> {
        self.range_key_bounds_limit(start, end, None)
    }

    pub fn range_key_bounds_limit(
        &self,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
        limit: Option<usize>,
    ) -> Result<Vec<ObjRef>, String> {
        Ok(self.range_key_page(start, end, None, limit, false)?.0)
    }

    pub(crate) fn range_key_page(
        &self,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
        after: Option<(&[u8], ObjRef)>,
        limit: Option<usize>,
        descending: bool,
    ) -> Result<(Vec<ObjRef>, Option<(Vec<u8>, ObjRef)>), String> {
        let (entries, next) = self.range_entry_page(start, end, after, limit, descending)?;
        Ok((
            entries
                .into_iter()
                .map(|(_, object_ref)| object_ref)
                .collect(),
            next,
        ))
    }

    pub(crate) fn range_entry_page(
        &self,
        mut start: Bound<Vec<u8>>,
        mut end: Bound<Vec<u8>>,
        after: Option<(&[u8], ObjRef)>,
        limit: Option<usize>,
        descending: bool,
    ) -> Result<(Vec<(Vec<u8>, ObjRef)>, Option<(Vec<u8>, ObjRef)>), String> {
        let lower = match &start {
            Bound::Included(value) | Bound::Excluded(value) => Some(value),
            Bound::Unbounded => None,
        };
        let upper = match &end {
            Bound::Included(value) | Bound::Excluded(value) => Some(value),
            Bound::Unbounded => None,
        };
        if let (Some(lower), Some(upper)) = (lower, upper) {
            if lower > upper
                || (lower == upper
                    && (matches!(&start, Bound::Excluded(_)) || matches!(&end, Bound::Excluded(_))))
            {
                return Ok((Vec::new(), None));
            }
        }
        if limit == Some(0) {
            return Ok((Vec::new(), None));
        }
        if let Some((cursor_key, _)) = after {
            if descending {
                let can_seek = match &end {
                    Bound::Unbounded => true,
                    Bound::Included(upper) => cursor_key <= upper.as_slice(),
                    Bound::Excluded(upper) => cursor_key < upper.as_slice(),
                };
                if can_seek {
                    end = Bound::Included(cursor_key.to_vec());
                }
            } else {
                let can_seek = match &start {
                    Bound::Unbounded => true,
                    Bound::Included(lower) => cursor_key >= lower.as_slice(),
                    Bound::Excluded(lower) => cursor_key > lower.as_slice(),
                };
                if can_seek {
                    start = Bound::Included(cursor_key.to_vec());
                }
            }
        }
        let lower = match &start {
            Bound::Included(value) | Bound::Excluded(value) => Some(value),
            Bound::Unbounded => None,
        };
        let upper = match &end {
            Bound::Included(value) | Bound::Excluded(value) => Some(value),
            Bound::Unbounded => None,
        };
        if let (Some(lower), Some(upper)) = (lower, upper) {
            if lower > upper
                || (lower == upper
                    && (matches!(&start, Bound::Excluded(_)) || matches!(&end, Bound::Excluded(_))))
            {
                return Ok((Vec::new(), None));
            }
        }
        // One entry is lookahead for `has_more`. After a keyset seek, the
        // source range deliberately includes the cursor key so duplicate
        // postings can resume within that key; reserve one additional source
        // key because the cursor entry itself is filtered below.
        let take = limit.map(|limit| {
            limit
                .saturating_add(1)
                .saturating_add(usize::from(after.is_some()))
        });
        let base_only = {
            let inner = self
                .inner
                .read()
                .map_err(|_| "B-tree read lock poisoned".to_string())?;
            (inner.entries.is_empty() && inner.tombstones.is_empty())
                .then(|| inner.base.clone())
                .flatten()
        };
        if let Some(base) = base_only {
            let entries = base.range_entries(start, end, take, descending)?;
            let mut page = if descending {
                entries
                    .into_iter()
                    .flat_map(|(key, postings)| {
                        postings
                            .into_iter()
                            .rev()
                            .map(move |object_ref| (key.clone(), object_ref))
                    })
                    .filter(|(key, object_ref)| {
                        after.is_none_or(|(cursor_key, cursor_ref)| {
                            key.as_slice() < cursor_key
                                || (key.as_slice() == cursor_key && *object_ref < cursor_ref)
                        })
                    })
                    .take(take.unwrap_or(usize::MAX))
                    .collect::<Vec<_>>()
            } else {
                entries
                    .into_iter()
                    .flat_map(|(key, postings)| {
                        postings
                            .into_iter()
                            .map(move |object_ref| (key.clone(), object_ref))
                    })
                    .filter(|(key, object_ref)| {
                        after.is_none_or(|(cursor_key, cursor_ref)| {
                            key.as_slice() > cursor_key
                                || (key.as_slice() == cursor_key && *object_ref > cursor_ref)
                        })
                    })
                    .take(take.unwrap_or(usize::MAX))
                    .collect::<Vec<_>>()
            };
            let has_more = limit.is_some_and(|limit| page.len() > limit);
            if has_more {
                page.pop();
            }
            let next = has_more.then(|| page.last().cloned()).flatten();
            return Ok((page, next));
        }
        let entries = self.merged_entries(start.clone(), end.clone(), take, descending)?;
        let mut page = if descending {
            let values = entries
                .range((start, end))
                .rev()
                .flat_map(|(key, postings)| {
                    postings
                        .iter()
                        .rev()
                        .copied()
                        .map(move |object_ref| (key.clone(), object_ref))
                })
                .filter(|(key, object_ref)| {
                    after.is_none_or(|(cursor_key, cursor_ref)| {
                        key.as_slice() < cursor_key
                            || (key.as_slice() == cursor_key && *object_ref < cursor_ref)
                    })
                });
            match take {
                Some(take) => values.take(take).collect::<Vec<_>>(),
                None => values.collect(),
            }
        } else {
            let values = entries
                .range((start, end))
                .flat_map(|(key, postings)| {
                    postings
                        .iter()
                        .copied()
                        .map(move |object_ref| (key.clone(), object_ref))
                })
                .filter(|(key, object_ref)| {
                    after.is_none_or(|(cursor_key, cursor_ref)| {
                        key.as_slice() > cursor_key
                            || (key.as_slice() == cursor_key && *object_ref > cursor_ref)
                    })
                });
            match take {
                Some(take) => values.take(take).collect::<Vec<_>>(),
                None => values.collect(),
            }
        };
        let has_more = limit.is_some_and(|limit| page.len() > limit);
        if has_more {
            page.pop();
        }
        let next = has_more.then(|| page.last().cloned()).flatten();
        Ok((page, next))
    }

    pub(crate) fn snapshot(&self) -> Result<Vec<(Vec<u8>, Vec<ObjRef>)>, String> {
        Ok(self
            .merged_entries(Bound::Unbounded, Bound::Unbounded, None, false)?
            .iter()
            .map(|(key, postings)| (key.clone(), postings.iter().copied().collect()))
            .collect())
    }

    fn merged_entries(
        &self,
        start: Bound<Vec<u8>>,
        end: Bound<Vec<u8>>,
        limit: Option<usize>,
        descending: bool,
    ) -> Result<BTreeMap<Vec<u8>, BTreeSet<ObjRef>>, String> {
        let (base, local, tombstones) = {
            let inner = self
                .inner
                .read()
                .map_err(|_| "B-tree read lock poisoned".to_string())?;
            let tombstones = inner
                .tombstones
                .range((start.clone(), end.clone()))
                .cloned()
                .collect::<Vec<_>>();
            // The first N keys of a merged ordered stream can only contain
            // keys from the first N keys of each input. Base tombstones may
            // remove candidates, so allow one additional base key per
            // tombstone in the requested range.
            let source_limit = limit.map(|limit| limit.saturating_add(tombstones.len()));
            let local: Vec<_> = if descending {
                let entries = inner.entries.range((start.clone(), end.clone())).rev();
                match source_limit {
                    Some(limit) => entries
                        .take(limit)
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                    None => entries
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                }
            } else {
                let entries = inner.entries.range((start.clone(), end.clone()));
                match source_limit {
                    Some(limit) => entries
                        .take(limit)
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                    None => entries
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                }
            };
            (inner.base.clone(), local, tombstones)
        };
        let base_limit = limit.map(|limit| limit.saturating_add(tombstones.len()));
        let mut merged = base
            .map(|base| base.range_entries(start, end, base_limit, descending))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|(key, postings)| (key, postings.into_iter().collect()))
            .collect::<BTreeMap<_, _>>();
        for key in tombstones {
            merged.remove(&key);
        }
        for (key, postings) in local {
            merged.insert(key, postings);
        }
        Ok(merged)
    }

    pub(crate) fn take_delta(&self) -> Result<Vec<(Vec<u8>, Option<Vec<ObjRef>>)>, String> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?;
        let dirty = std::mem::take(&mut inner.dirty);
        Ok(dirty
            .into_iter()
            .map(|key| {
                let postings = inner
                    .entries
                    .get(&key)
                    .map(|postings| postings.iter().copied().collect());
                (key, postings)
            })
            .collect())
    }

    pub(crate) fn mark_clean(&self) -> Result<(), String> {
        self.inner
            .write()
            .map_err(|_| "B-tree write lock poisoned".to_string())?
            .dirty
            .clear();
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn delta_size_estimate(&self) -> Result<u64, String> {
        let inner = self
            .inner
            .read()
            .map_err(|_| "B-tree read lock poisoned".to_string())?;
        Ok(inner.dirty.iter().fold(0u64, |total, key| {
            let posting_bytes = inner
                .entries
                .get(key)
                .map_or(0, |postings| postings.len() as u64 * 8);
            total.saturating_add(8 + key.len() as u64 + posting_bytes)
        }))
    }

    pub fn insert(&self, key: u64, value: ObjRef) -> Result<(), String> {
        self.insert_key(key.to_be_bytes().to_vec(), value, false)
    }

    pub fn search(&self, key: u64) -> Result<Option<ObjRef>, String> {
        Ok(self.search_key(&key.to_be_bytes())?.into_iter().next())
    }

    pub fn delete(&self, key: u64) -> Result<bool, String> {
        let key = key.to_be_bytes();
        let values = self.search_key(&key)?;
        let mut removed = false;
        for value in values {
            removed |= self.delete_key(&key, value)?;
        }
        Ok(removed)
    }

    pub fn range_query(&self, start: u64, end: u64) -> Result<Vec<ObjRef>, String> {
        self.range_keys(&start.to_be_bytes(), &end.to_be_bytes())
    }
}

fn visit_overlay(
    visitor: &mut dyn FnMut(&[u8], &[ObjRef]) -> Result<(), String>,
    key: Vec<u8>,
    postings: Option<Vec<ObjRef>>,
) -> Result<(), String> {
    if let Some(postings) = postings {
        visitor(&key, &postings)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use crate::storage_engine::TypedKey;

    struct TestBase(BTreeMap<Vec<u8>, Vec<ObjRef>>);

    impl OrderedIndexBase for TestBase {
        fn search_key(&self, key: &[u8]) -> Result<Vec<ObjRef>, String> {
            Ok(self.0.get(key).cloned().unwrap_or_default())
        }

        fn range_entries(
            &self,
            start: Bound<Vec<u8>>,
            end: Bound<Vec<u8>>,
            limit: Option<usize>,
            descending: bool,
        ) -> Result<Vec<(Vec<u8>, Vec<ObjRef>)>, String> {
            let entries = self.0.range((start, end));
            Ok(if descending {
                match limit {
                    Some(limit) => entries
                        .rev()
                        .take(limit)
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                    None => entries
                        .rev()
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                }
            } else {
                match limit {
                    Some(limit) => entries
                        .take(limit)
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                    None => entries
                        .map(|(key, postings)| (key.clone(), postings.clone()))
                        .collect(),
                }
            })
        }

        fn visit_entries(
            &self,
            visitor: &mut dyn FnMut(&[u8], &[ObjRef]) -> Result<(), String>,
        ) -> Result<(), String> {
            for (key, postings) in &self.0 {
                visitor(key, postings)?;
            }
            Ok(())
        }
    }

    #[test]
    fn file_base_and_complete_key_overlay_merge_exactly() {
        let first = ObjRef::from_parts(1, 1, 1);
        let second = ObjRef::from_parts(1, 2, 1);
        let third = ObjRef::from_parts(1, 3, 1);
        let index = BTreeIndex::with_base(Arc::new(TestBase(BTreeMap::from([
            (b"a".to_vec(), vec![first]),
            (b"b".to_vec(), vec![second]),
        ]))));

        assert_eq!(index.search_key(b"a").unwrap(), vec![first]);
        assert!(index.delete_key(b"a", first).unwrap());
        assert!(index.search_key(b"a").unwrap().is_empty());
        index.insert_key(b"b".to_vec(), third, false).unwrap();
        index.insert_key(b"c".to_vec(), first, false).unwrap();
        assert_eq!(index.search_key(b"b").unwrap(), vec![second, third]);
        assert_eq!(
            index
                .range_key_bounds(Bound::Unbounded, Bound::Unbounded)
                .unwrap(),
            vec![second, third, first]
        );
        assert_eq!(
            index.take_delta().unwrap(),
            vec![
                (b"a".to_vec(), None),
                (b"b".to_vec(), Some(vec![second, third])),
                (b"c".to_vec(), Some(vec![first])),
            ]
        );
    }

    #[test]
    fn paged_merge_bounds_base_and_overlay_without_losing_order() {
        let object = |id| ObjRef::from_parts(1, id, 1);
        let index = BTreeIndex::with_base(Arc::new(TestBase(BTreeMap::from([
            (b"a".to_vec(), vec![object(1)]),
            (b"b".to_vec(), vec![object(2)]),
            (b"c".to_vec(), vec![object(3)]),
            (b"d".to_vec(), vec![object(4)]),
            (b"e".to_vec(), vec![object(5)]),
        ]))));
        assert!(index.delete_key(b"a", object(1)).unwrap());
        assert!(index.delete_key(b"b", object(2)).unwrap());
        index.insert_key(b"bb".to_vec(), object(6), false).unwrap();

        let (ascending, next) = index
            .range_entry_page(Bound::Unbounded, Bound::Unbounded, None, Some(3), false)
            .unwrap();
        assert_eq!(
            ascending
                .iter()
                .map(|(key, _)| key.as_slice())
                .collect::<Vec<_>>(),
            vec![b"bb".as_slice(), b"c".as_slice(), b"d".as_slice()]
        );
        assert_eq!(
            next.as_ref().map(|(key, _)| key.as_slice()),
            Some(b"d".as_slice())
        );

        let (descending, next) = index
            .range_entry_page(Bound::Unbounded, Bound::Unbounded, None, Some(3), true)
            .unwrap();
        assert_eq!(
            descending
                .iter()
                .map(|(key, _)| key.as_slice())
                .collect::<Vec<_>>(),
            vec![b"e".as_slice(), b"d".as_slice(), b"c".as_slice()]
        );
        assert_eq!(
            next.as_ref().map(|(key, _)| key.as_slice()),
            Some(b"c".as_slice())
        );
    }

    #[test]
    fn keyset_pagination_continues_beyond_the_second_page() {
        let object = |id| ObjRef::from_parts(1, id, 1);
        let index = BTreeIndex::with_base(Arc::new(TestBase(BTreeMap::from([
            (b"a".to_vec(), vec![object(1)]),
            (b"b".to_vec(), vec![object(2)]),
            (b"c".to_vec(), vec![object(3)]),
            (b"d".to_vec(), vec![object(4)]),
            (b"e".to_vec(), vec![object(5)]),
        ]))));

        let mut cursor: Option<(Vec<u8>, ObjRef)> = None;
        let mut keys = Vec::new();
        let mut page_count = 0;
        loop {
            let (page, next) = index
                .range_entry_page(
                    Bound::Unbounded,
                    Bound::Unbounded,
                    cursor
                        .as_ref()
                        .map(|(key, object_ref)| (key.as_slice(), *object_ref)),
                    Some(2),
                    false,
                )
                .unwrap();
            page_count += 1;
            keys.extend(page.into_iter().map(|(key, _)| key));
            let Some(next) = next else { break };
            cursor = Some(next);
        }

        assert_eq!(page_count, 3);
        assert_eq!(
            keys,
            [b"a", b"b", b"c", b"d", b"e"]
                .into_iter()
                .map(|key| key.to_vec())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn inclusive_range_keys_merge_untouched_base_entries_with_overlay() {
        let base_object = ObjRef::from_parts(1, 1, 1);
        let overlay_object = ObjRef::from_parts(1, 2, 1);
        let index = BTreeIndex::with_base(Arc::new(TestBase(BTreeMap::from([(
            b"b".to_vec(),
            vec![base_object],
        )]))));
        index
            .insert_key(b"c".to_vec(), overlay_object, false)
            .unwrap();
        assert_eq!(
            index.range_keys(b"a", b"z").unwrap(),
            vec![base_object, overlay_object]
        );
    }

    #[test]
    fn non_unique_keys_keep_all_postings_in_identity_order() {
        let index = BTreeIndex::create(16, 0).unwrap();
        let key = TypedKey::encode_one(&Value::String("error".into())).unwrap();
        let first = ObjRef::from_parts(1, 1, 1);
        let second = ObjRef::from_parts(1, 2, 1);
        index.insert_key(key.clone(), second, false).unwrap();
        index.insert_key(key.clone(), first, false).unwrap();
        assert_eq!(index.search_key(&key).unwrap(), vec![first, second]);
        assert!(index.delete_key(&key, first).unwrap());
        assert_eq!(index.search_key(&key).unwrap(), vec![second]);
    }

    #[test]
    fn unique_keys_reject_a_different_object_but_allow_idempotence() {
        let index = BTreeIndex::create(16, 0).unwrap();
        let key = TypedKey::encode_one(&Value::Integer(42)).unwrap();
        let owner = ObjRef::from_parts(1, 1, 1);
        index.insert_key(key.clone(), owner, true).unwrap();
        index.insert_key(key.clone(), owner, true).unwrap();
        assert!(index
            .insert_key(key, ObjRef::from_parts(1, 2, 1), true)
            .is_err());
    }

    #[test]
    fn typed_range_flattens_postings_without_losing_duplicates() {
        let index = BTreeIndex::create(16, 0).unwrap();
        for (object, value) in [(1, -5), (2, 0), (3, 0), (4, 10)] {
            index
                .insert_key(
                    TypedKey::encode_one(&Value::Integer(value)).unwrap(),
                    ObjRef::from_parts(1, object, 1),
                    false,
                )
                .unwrap();
        }
        let found = index
            .range_keys(
                &TypedKey::encode_one(&Value::Integer(-1)).unwrap(),
                &TypedKey::encode_one(&Value::Integer(5)).unwrap(),
            )
            .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].object_id(), 2);
        assert_eq!(found[1].object_id(), 3);
    }

    #[test]
    fn excluded_and_contradictory_bounds_are_exact() {
        let index = BTreeIndex::create(16, 0).unwrap();
        for value in [1, 2, 3] {
            index
                .insert_key(
                    TypedKey::encode_one(&Value::Integer(value)).unwrap(),
                    ObjRef::from_parts(1, value as u32, 1),
                    false,
                )
                .unwrap();
        }
        let two = TypedKey::encode_one(&Value::Integer(2)).unwrap();
        let three = TypedKey::encode_one(&Value::Integer(3)).unwrap();
        assert_eq!(
            index
                .range_key_bounds(Bound::Excluded(two.clone()), Bound::Included(three.clone()))
                .unwrap()
                .len(),
            1
        );
        assert!(index
            .range_key_bounds(Bound::Excluded(two.clone()), Bound::Included(two))
            .unwrap()
            .is_empty());
        assert!(index
            .range_key_bounds(Bound::Included(three), Bound::Unbounded)
            .unwrap()
            .iter()
            .all(|reference| reference.object_id() == 3));
    }
}
