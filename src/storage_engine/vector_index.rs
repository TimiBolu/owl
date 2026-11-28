//! An index for finding the vectors nearest a given one without comparing
//! them all.
//!
//! The vectors of a field are kept in lists, each around a centre: a
//! vector goes in the list whose centre it is nearest. A search compares
//! the query with the centres, which are few, and then with the vectors
//! of the nearest lists only, nearest list first, until it has compared
//! as many as it was asked to. Vectors that are alike are in the same few
//! lists, so the nearest are found among far fewer than all.
//!
//! That is approximate: a near vector in a list not reached is missed.
//! Asked to be exact, a search compares every list, which is still done
//! here, on packed numbers, with no row read.
//!
//! The index is kept in step as rows are written: a vector is put in its
//! list, taken out, or moved, at the cost of comparing it with the
//! centres. The centres are worked out from the vectors themselves
//! (k-means) once there are enough to be worth dividing, and again each
//! time the collection has doubled. Until then there is one list.
//!
//! The vectors themselves are never stored twice: a database opened afresh
//! reads them from the rows on the first search that wants the index.
//! What is kept beside the database is what was costly to work out: the
//! centres, and which list each row was put in. Those are a hint and no
//! more. A row the hint does not name is given its list by comparing it
//! with the centres, and a hint that no longer suits the rows is thrown
//! away, so a wrong or stale hint can make a search slower or miss more,
//! and can never make it give a row that should not be given.

use super::ObjRef;
use crate::vector::{dot, norm, squared_distance, Metric};
use rayon::prelude::*;
use std::collections::HashMap;

/// Fewer vectors than this are one list: comparing them all is quick, and
/// centres worked out from few vectors divide them badly.
const WORTH_DIVIDING: usize = 4_096;
/// The most lists, however many vectors.
const MOST_LISTS: usize = 4_096;
/// Vectors looked at for each centre when working the centres out: the
/// centres come from a sample, since their work grows with the sample
/// times the number of centres.
const SAMPLE_FOR_EACH_CENTRE: usize = 24;
/// Lists for each one the square root of the number of vectors gives.
const LISTS_FOR_EACH_ROOT: usize = 4;
/// How a kept hint begins.
const HINT_MAGIC: &[u8; 8] = b"OWLVHNT1";
/// Rows looked at to tell whether a kept hint still suits the rows.
const HINT_SAMPLE: usize = 64;
/// Rounds of moving each centre to the middle of its vectors.
const ROUNDS: usize = 6;

thread_local! {
    /// How many times this thread has worked an index's centres out.
    static DIVISIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How many times this thread has worked out the centres of a vector
/// index, which is the costly part of making one. For tests.
#[doc(hidden)]
pub fn divisions() -> u64 {
    DIVISIONS.with(std::cell::Cell::get)
}

#[derive(Clone, Default)]
struct List {
    rows: Vec<ObjRef>,
    /// One vector after another, as long as the index's dimension.
    vectors: Vec<f32>,
}

/// What a search did, for `explain()` and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Searched {
    /// The lists the index holds.
    pub lists: usize,
    /// The lists whose vectors were compared.
    pub probed: usize,
    /// The vectors compared.
    pub compared: usize,
    /// The vectors the index holds.
    pub held: usize,
}

#[derive(Clone)]
pub(crate) struct VectorIndex {
    dimension: usize,
    metric: Metric,
    /// One centre after another; empty while there is one list.
    centres: Vec<f32>,
    lists: Vec<List>,
    /// Where each row's vector is: which list, and where in it.
    place: HashMap<ObjRef, (u32, u32)>,
    /// How many vectors there were when the centres were worked out.
    divided_at: usize,
    /// The commit the index is good as of, for the collection and for
    /// the schemas: the store's stamps when it was last brought up to date.
    pub(crate) stamps: (u64, u64),
}

/// SplitMix64: the same vectors always give the same centres.
struct Random(u64);

impl Random {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) % bound.max(1) as u64) as usize
    }
}

impl VectorIndex {
    /// An empty index for vectors of `dimension` numbers, compared by
    /// `metric`: by direction, or by straight-line distance. The dot
    /// product has no index; a search by it compares every vector.
    pub(crate) fn new(dimension: usize, metric: Metric) -> Option<Self> {
        if dimension == 0 || metric == Metric::Dot {
            return None;
        }
        Some(Self {
            dimension,
            metric,
            centres: Vec::new(),
            lists: vec![List::default()],
            place: HashMap::new(),
            divided_at: 0,
            stamps: (0, 0),
        })
    }

    pub(crate) fn dimension(&self) -> usize {
        self.dimension
    }

    pub(crate) fn metric(&self) -> Metric {
        self.metric
    }

    /// How many vectors are held.
    pub(crate) fn len(&self) -> usize {
        self.place.len()
    }

    /// A vector as the index keeps it. By direction, only the direction
    /// matters, so every vector is kept at length one and the cosine is
    /// then the sum of the products.
    fn kept(&self, vector: &[f32]) -> Vec<f32> {
        match self.metric {
            Metric::Cosine => {
                let length = norm(vector);
                match length == 0.0 {
                    true => vector.to_vec(),
                    false => vector.iter().map(|number| number / length).collect(),
                }
            }
            _ => vector.to_vec(),
        }
    }

    /// How near `vector` is to `other`, both as kept: higher is nearer.
    fn nearness(&self, vector: &[f32], other: &[f32]) -> f32 {
        match self.metric {
            Metric::L2 => -squared_distance(vector, other),
            _ => dot(vector, other),
        }
    }

    /// The list whose centre is nearest `kept`.
    fn list_of(&self, kept: &[f32]) -> usize {
        if self.centres.is_empty() {
            return 0;
        }
        self.centres
            .chunks_exact(self.dimension)
            .map(|centre| self.nearness(kept, centre))
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(0, |(list, _)| list)
    }

    /// Put a row's vector in, in place of any it had. A vector of another
    /// length than the index's is not one of its vectors: the row is taken
    /// out.
    pub(crate) fn put(&mut self, row: ObjRef, vector: &[f32]) {
        self.put_in(row, vector, None);
    }

    /// [`Self::put`], into the list a hint names when it names one that
    /// exists, and otherwise into the list of the nearest centre.
    fn put_in(&mut self, row: ObjRef, vector: &[f32], hinted: Option<u32>) {
        self.remove(row);
        if vector.len() != self.dimension {
            return;
        }
        let kept = self.kept(vector);
        let list = match hinted.map(|list| list as usize).filter(|list| *list < self.lists.len()) {
            Some(list) => list,
            None => self.list_of(&kept),
        };
        let held = &mut self.lists[list];
        self.place.insert(row, (list as u32, held.rows.len() as u32));
        held.rows.push(row);
        held.vectors.extend_from_slice(&kept);
    }

    /// Take a row's vector out, if it has one.
    pub(crate) fn remove(&mut self, row: ObjRef) {
        let Some((list, at)) = self.place.remove(&row) else {
            return;
        };
        let (dimension, at) = (self.dimension, at as usize);
        let held = &mut self.lists[list as usize];
        let last = held.rows.len() - 1;
        // The last vector of the list takes the place of the one removed.
        if at != last {
            let moved = held.rows[last];
            held.rows[at] = moved;
            held.vectors.copy_within(last * dimension..(last + 1) * dimension, at * dimension);
            self.place.insert(moved, (list, at as u32));
        }
        held.rows.pop();
        held.vectors.truncate(last * dimension);
    }

    /// What is worth keeping of the index beside the database: its
    /// centres, and which list each row is in. `None` while there is one
    /// list, when there is nothing that was costly to work out.
    pub(crate) fn hint(&self) -> Option<Vec<u8>> {
        if self.centres.is_empty() {
            return None;
        }
        let mut bytes = Vec::with_capacity(32 + self.centres.len() * 4 + self.place.len() * 12);
        bytes.extend_from_slice(HINT_MAGIC);
        bytes.extend_from_slice(&(self.dimension as u32).to_le_bytes());
        bytes.push(u8::from(self.metric == Metric::L2));
        bytes.extend_from_slice(&(self.divided_at as u64).to_le_bytes());
        bytes.extend_from_slice(&(self.lists.len() as u32).to_le_bytes());
        for number in &self.centres {
            bytes.extend_from_slice(&number.to_le_bytes());
        }
        bytes.extend_from_slice(&(self.place.len() as u64).to_le_bytes());
        for (row, (list, _)) in &self.place {
            bytes.extend_from_slice(&row.packed().to_le_bytes());
            bytes.extend_from_slice(&list.to_le_bytes());
        }
        let checksum = crc32fast::hash(&bytes);
        bytes.extend_from_slice(&checksum.to_le_bytes());
        Some(bytes)
    }

    /// An index of `rows`, each with its vector, made with the help of a
    /// hint kept from an earlier one: its centres are taken as they are,
    /// and a row it names goes to the list it names. `None` when the hint
    /// is damaged, is for another length or metric, or no longer suits
    /// the rows: of a sample of the rows it names, too few are in the
    /// list of their nearest centre.
    pub(crate) fn from_hint(
        dimension: usize,
        metric: Metric,
        hint: &[u8],
        rows: &[(ObjRef, Vec<f32>)],
    ) -> Option<Self> {
        let (body, checksum) = hint.split_at_checked(hint.len().checked_sub(4)?)?;
        if crc32fast::hash(body) != u32::from_le_bytes(checksum.try_into().ok()?) {
            return None;
        }
        let mut at = 0usize;
        let mut take = |length: usize| -> Option<&[u8]> {
            let bytes = body.get(at..at.checked_add(length)?)?;
            at += length;
            Some(bytes)
        };
        if take(HINT_MAGIC.len())? != HINT_MAGIC {
            return None;
        }
        let kept_dimension = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        let by_distance = take(1)?[0] == 1;
        let divided_at = u64::from_le_bytes(take(8)?.try_into().ok()?) as usize;
        let lists = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        if kept_dimension != dimension || by_distance != (metric == Metric::L2) || !(2..=MOST_LISTS).contains(&lists) {
            return None;
        }
        let centres: Vec<f32> = take(lists.checked_mul(dimension)?.checked_mul(4)?)?
            .chunks_exact(4)
            .map(|number| f32::from_le_bytes(number.try_into().unwrap()))
            .collect();
        let named = u64::from_le_bytes(take(8)?.try_into().ok()?) as usize;
        let mut hinted: HashMap<ObjRef, u32> = HashMap::with_capacity(named);
        for pair in take(named.checked_mul(12)?)?.chunks_exact(12) {
            hinted.insert(
                ObjRef::new(u64::from_le_bytes(pair[..8].try_into().unwrap())),
                u32::from_le_bytes(pair[8..].try_into().unwrap()),
            );
        }
        let mut index = Self::new(dimension, metric)?;
        index.centres = centres;
        index.lists = vec![List::default(); lists];
        index.divided_at = divided_at;
        // Does the hint still suit these rows? Of a sample of the rows it
        // names, most should be where their nearest centre is. A hint
        // kept by another database at the same place, or from before the
        // vectors were all rewritten, fails this.
        let known: Vec<&(ObjRef, Vec<f32>)> =
            rows.iter().filter(|(row, vector)| hinted.contains_key(row) && vector.len() == dimension).collect();
        let step = (known.len() / HINT_SAMPLE).max(1);
        let sampled: Vec<&&(ObjRef, Vec<f32>)> = known.iter().step_by(step).collect();
        let suited = sampled
            .iter()
            .filter(|(row, vector)| index.list_of(&index.kept(vector)) as u32 == hinted[row])
            .count();
        if suited * 4 < sampled.len() * 3 {
            return None;
        }
        for (row, vector) in rows {
            index.put_in(*row, vector, hinted.get(row).copied());
        }
        Some(index)
    }

    /// Whether the centres should be worked out again before the next
    /// search: there are now enough vectors to divide, or twice as many
    /// as when they last were.
    pub(crate) fn wants_dividing(&self) -> bool {
        let held = self.len();
        held >= WORTH_DIVIDING && held >= self.divided_at.saturating_mul(2)
    }

    /// Work out the centres from the vectors held, and put each vector in
    /// the list of its nearest centre.
    pub(crate) fn divide(&mut self) {
        DIVISIONS.with(|count| count.set(count.get() + 1));
        let dimension = self.dimension;
        let mut rows: Vec<ObjRef> = Vec::with_capacity(self.len());
        let mut vectors: Vec<f32> = Vec::with_capacity(self.len() * dimension);
        for list in self.lists.drain(..) {
            rows.extend(list.rows);
            vectors.extend(list.vectors);
        }
        let held = rows.len();
        self.divided_at = held;
        self.place.clear();
        self.centres.clear();
        if held < WORTH_DIVIDING {
            self.lists = vec![List::default()];
        } else {
            // About as many lists as there are vectors in a list.
            let lists = ((held as f64).sqrt() as usize * LISTS_FOR_EACH_ROOT).clamp(2, MOST_LISTS);
            self.centres = self.centres_of(&vectors, lists);
            self.lists = vec![List::default(); lists];
        }
        // Finding each vector's list is most of the work, and each is
        // found on its own.
        let found: Vec<usize> = vectors.par_chunks_exact(dimension).map(|kept| self.list_of(kept)).collect();
        for (at, row) in rows.into_iter().enumerate() {
            let kept = &vectors[at * dimension..(at + 1) * dimension];
            let list = found[at];
            let into = &mut self.lists[list];
            self.place.insert(row, (list as u32, into.rows.len() as u32));
            into.rows.push(row);
            into.vectors.extend_from_slice(kept);
        }
    }

    /// `lists` centres for `vectors`: start from vectors chosen at random,
    /// then, a few times over, give each sampled vector to its nearest
    /// centre and move each centre to the middle of what it was given.
    fn centres_of(&self, vectors: &[f32], lists: usize) -> Vec<f32> {
        let dimension = self.dimension;
        let held = vectors.len() / dimension;
        let mut random = Random(held as u64 ^ 0x5EED);
        let vector = |at: usize| &vectors[at * dimension..(at + 1) * dimension];
        let sample: Vec<usize> = match held <= lists * SAMPLE_FOR_EACH_CENTRE {
            true => (0..held).collect(),
            false => (0..lists * SAMPLE_FOR_EACH_CENTRE).map(|_| random.below(held)).collect(),
        };
        let mut centres: Vec<f32> = Vec::with_capacity(lists * dimension);
        for _ in 0..lists {
            centres.extend_from_slice(vector(sample[random.below(sample.len())]));
        }
        for _ in 0..ROUNDS {
            let mut sums = vec![0f64; lists * dimension];
            let mut given = vec![0usize; lists];
            let nearest_of: Vec<usize> = sample
                .par_iter()
                .map(|at| {
                    centres
                        .chunks_exact(dimension)
                        .map(|centre| self.nearness(vector(*at), centre))
                        .enumerate()
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map_or(0, |(list, _)| list)
                })
                .collect();
            for (at, nearest) in sample.iter().zip(nearest_of) {
                given[nearest] += 1;
                for (sum, number) in sums[nearest * dimension..(nearest + 1) * dimension].iter_mut().zip(vector(*at)) {
                    *sum += f64::from(*number);
                }
            }
            for list in 0..lists {
                let centre = &mut centres[list * dimension..(list + 1) * dimension];
                if given[list] == 0 {
                    // A centre nothing was nearest to starts again from a
                    // vector, so that no list is left empty for good.
                    centre.copy_from_slice(vector(sample[random.below(sample.len())]));
                    continue;
                }
                for (number, sum) in centre.iter_mut().zip(&sums[list * dimension..(list + 1) * dimension]) {
                    *number = (*sum / given[list] as f64) as f32;
                }
                // By direction, a centre is a direction too.
                if self.metric == Metric::Cosine {
                    let length = norm(centre);
                    if length > 0.0 {
                        centre.iter_mut().for_each(|number| *number /= length);
                    }
                }
            }
        }
        centres
    }

    /// How many vectors a search compares when it is not told: enough of
    /// the nearest lists to hold an eighth of the vectors, and never fewer
    /// than a hundred for each row wanted.
    fn compared_unasked(&self, k: usize) -> usize {
        (self.len() / 8).max(k.saturating_mul(100))
    }

    /// The rows nearest `query`, nearest first, each with how near the
    /// index found it, and what the search did. At least `k` rows when
    /// that many are compared; rows as near as the last of them are
    /// given too, so that the caller can put equals in its own order.
    ///
    /// `exact` compares every vector. Otherwise the nearest lists are
    /// compared until `compare` vectors have been, or, when it is not
    /// given, as many as [`Self::compared_unasked`] says.
    pub(crate) fn search(
        &self,
        query: &[f32],
        k: usize,
        exact: bool,
        compare: Option<usize>,
    ) -> (Vec<(ObjRef, f32)>, Searched) {
        let mut searched = Searched { lists: self.lists.len(), held: self.len(), ..Searched::default() };
        if query.len() != self.dimension || k == 0 {
            return (Vec::new(), searched);
        }
        let dimension = self.dimension;
        let query = self.kept(query);
        // The lists, nearest centre first.
        let mut order: Vec<(usize, f32)> = match self.centres.is_empty() {
            true => vec![(0, 0.0)],
            false => self
                .centres
                .chunks_exact(dimension)
                .map(|centre| self.nearness(&query, centre))
                .enumerate()
                .collect(),
        };
        let enough = match exact {
            true => usize::MAX,
            false => compare.unwrap_or_else(|| self.compared_unasked(k)).max(k),
        };
        if !exact {
            order.sort_by(|a, b| b.1.total_cmp(&a.1));
        }
        let mut found: Vec<(ObjRef, f32)> = Vec::new();
        for (list, _) in order {
            if searched.compared >= enough {
                break;
            }
            let held = &self.lists[list];
            searched.probed += 1;
            searched.compared += held.rows.len();
            found.extend(
                held.rows
                    .iter()
                    .zip(held.vectors.chunks_exact(dimension))
                    .map(|(row, vector)| (*row, self.nearness(&query, vector))),
            );
        }
        (Self::nearest_of(found, k), searched)
    }

    /// The `k` nearest of `found`, nearest first, with whatever is as near
    /// as the last of them, give or take what rounding can account for.
    /// The index compares vectors as it keeps them, and the caller scores
    /// the rows it is given from their own vectors, which can differ in
    /// the last place: a row that close to the last is the caller's to
    /// put in or leave out.
    fn nearest_of(mut found: Vec<(ObjRef, f32)>, k: usize) -> Vec<(ObjRef, f32)> {
        let nearer = |a: &(ObjRef, f32), b: &(ObjRef, f32)| b.1.total_cmp(&a.1);
        if found.len() > k {
            found.select_nth_unstable_by(k - 1, nearer);
            let last = found[k - 1].1;
            let within = last - last.abs().max(1.0) * 1e-5;
            found.retain(|(_, nearness)| *nearness >= within);
        }
        found.sort_by(nearer);
        found
    }

    /// The `k` nearest `query` of `rows` only, every one of them compared:
    /// for a search confined to rows chosen some other way, by a filter or
    /// by a relationship. A row the index does not hold has no vector and
    /// is passed over. As [`Self::search`] gives them, with how many were
    /// compared.
    pub(crate) fn nearest_among(
        &self,
        rows: impl Iterator<Item = ObjRef>,
        query: &[f32],
        k: usize,
    ) -> (Vec<(ObjRef, f32)>, Searched) {
        let mut searched = Searched { lists: self.lists.len(), held: self.len(), ..Searched::default() };
        if query.len() != self.dimension || k == 0 {
            return (Vec::new(), searched);
        }
        let dimension = self.dimension;
        let query = self.kept(query);
        let found: Vec<(ObjRef, f32)> = rows
            .filter_map(|row| {
                let (list, at) = *self.place.get(&row)?;
                let at = at as usize;
                let vector = &self.lists[list as usize].vectors[at * dimension..(at + 1) * dimension];
                Some((row, self.nearness(&query, vector)))
            })
            .collect();
        searched.compared = found.len();
        (Self::nearest_of(found, k), searched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(number: u32) -> ObjRef {
        ObjRef::from_parts(1, number, 0)
    }

    /// `count` vectors in clumps: each is a clump's centre with a little
    /// noise, as the embeddings of documents on a few subjects are.
    fn clumped(count: usize, dimension: usize, clumps: usize, seed: u64) -> Vec<Vec<f32>> {
        let mut random = Random(seed);
        let mut unit = move || random.below(2_000_001) as f32 / 1_000_000.0 - 1.0;
        let centres: Vec<Vec<f32>> = (0..clumps).map(|_| (0..dimension).map(|_| unit()).collect()).collect();
        (0..count)
            .map(|at| centres[at % clumps].iter().map(|number| number + unit() * 0.25).collect())
            .collect()
    }

    fn exact(vectors: &[(ObjRef, Vec<f32>)], query: &[f32], metric: Metric, k: usize) -> Vec<ObjRef> {
        let mut scored: Vec<(ObjRef, f32)> = vectors
            .iter()
            .map(|(row, vector)| {
                let nearness = match metric {
                    Metric::L2 => -squared_distance(vector, query),
                    _ => crate::vector::similarity(vector, query),
                };
                (*row, nearness)
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        scored.into_iter().map(|(row, _)| row).collect()
    }

    #[test]
    fn a_dot_product_has_no_index_and_a_vector_of_another_length_is_not_held() {
        assert!(VectorIndex::new(3, Metric::Dot).is_none());
        assert!(VectorIndex::new(0, Metric::Cosine).is_none());
        let mut index = VectorIndex::new(3, Metric::Cosine).unwrap();
        index.put(row(1), &[1.0, 0.0, 0.0]);
        index.put(row(2), &[1.0, 0.0]);
        assert_eq!(index.len(), 1);
        // Put again with another length, it is taken out.
        index.put(row(1), &[1.0, 0.0]);
        assert_eq!(index.len(), 0);
        assert!(index.search(&[1.0, 0.0, 0.0], 3, true, None).0.is_empty());
    }

    #[test]
    fn asked_to_be_exact_it_finds_what_comparing_every_vector_finds() {
        for metric in [Metric::Cosine, Metric::L2] {
            let vectors = clumped(6_000, 24, 30, 11);
            let mut index = VectorIndex::new(24, metric).unwrap();
            let mut held = Vec::new();
            for (at, vector) in vectors.iter().enumerate() {
                index.put(row(at as u32 + 1), vector);
                held.push((row(at as u32 + 1), vector.clone()));
            }
            assert!(index.wants_dividing());
            index.divide();
            assert!(!index.wants_dividing());
            assert_eq!(index.len(), 6_000);
            for query in clumped(20, 24, 30, 99) {
                let (found, searched) = index.search(&query, 10, true, None);
                let found: Vec<ObjRef> = found.into_iter().take(10).map(|(row, _)| row).collect();
                assert_eq!(found, exact(&held, &query, metric, 10), "{metric:?}");
                assert_eq!(searched.compared, 6_000);
                assert_eq!(searched.probed, searched.lists);
                assert!(searched.lists > 1);
            }
        }
    }

    #[test]
    fn left_to_itself_it_compares_a_fraction_and_finds_nearly_all() {
        for metric in [Metric::Cosine, Metric::L2] {
            let vectors = clumped(20_000, 32, 200, 5);
            let mut index = VectorIndex::new(32, metric).unwrap();
            let mut held = Vec::new();
            for (at, vector) in vectors.iter().enumerate() {
                index.put(row(at as u32 + 1), vector);
                held.push((row(at as u32 + 1), vector.clone()));
            }
            index.divide();
            let (mut right, mut wanted, mut compared) = (0, 0, 0);
            for query in clumped(50, 32, 200, 77) {
                let truth = exact(&held, &query, metric, 10);
                let (found, searched) = index.search(&query, 10, false, None);
                let found: Vec<ObjRef> = found.into_iter().take(10).map(|(row, _)| row).collect();
                right += truth.iter().filter(|row| found.contains(row)).count();
                wanted += truth.len();
                compared += searched.compared;
            }
            let recall = right as f64 / wanted as f64;
            let share = compared as f64 / (50.0 * 20_000.0);
            assert!(recall >= 0.95, "{metric:?}: found {recall:.3} of the true nearest");
            assert!(share <= 0.25, "{metric:?}: compared {share:.3} of the vectors");
        }
    }

    /// Not a test of right and wrong: how much of the true nearest ten is
    /// found for how many vectors compared, on vectors in clumps and on
    /// vectors with no clumps at all, which is the hardest case for lists.
    /// Run with `--release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn what_finding_more_costs() {
        for (clumps, noise_label) in [(200usize, "clumped"), (20_000, "no clumps")] {
            for metric in [Metric::Cosine, Metric::L2] {
                let vectors = clumped(20_000, 32, clumps, 5);
                let mut index = VectorIndex::new(32, metric).unwrap();
                let mut held = Vec::new();
                for (at, vector) in vectors.iter().enumerate() {
                    index.put(row(at as u32 + 1), vector);
                    held.push((row(at as u32 + 1), vector.clone()));
                }
                index.divide();
                let queries = clumped(40, 32, clumps, 77);
                let truths: Vec<Vec<ObjRef>> = queries.iter().map(|q| exact(&held, q, metric, 10)).collect();
                for share in [64usize, 32, 16, 12, 8, 4, 2] {
                    let (mut right, mut compared) = (0, 0);
                    for (query, truth) in queries.iter().zip(&truths) {
                        let (found, searched) = index.search(query, 10, false, Some(20_000 / share));
                        let found: Vec<ObjRef> = found.into_iter().take(10).map(|(row, _)| row).collect();
                        right += truth.iter().filter(|row| found.contains(row)).count();
                        compared += searched.compared;
                    }
                    println!("{noise_label} {metric:?} ask 1/{share}: recall {:.3}, compared {:.3}", right as f64 / 400.0, compared as f64 / (40.0 * 20_000.0));
                }
            }
        }
    }

    #[test]
    fn it_stays_right_as_vectors_are_put_changed_and_taken_out() {
        let vectors = clumped(9_000, 16, 40, 3);
        let mut index = VectorIndex::new(16, Metric::Cosine).unwrap();
        let mut held: HashMap<ObjRef, Vec<f32>> = HashMap::new();
        let mut random = Random(42);
        for (at, vector) in vectors.iter().enumerate() {
            // Divided part way, so that later puts go to a list by centre.
            if at == 5_000 {
                index.divide();
            }
            let to = row(random.below(6_000) as u32 + 1);
            match random.below(5) {
                0 => {
                    index.remove(to);
                    held.remove(&to);
                }
                _ => {
                    index.put(to, vector);
                    held.insert(to, vector.clone());
                }
            }
        }
        assert_eq!(index.len(), held.len());
        let mut held: Vec<(ObjRef, Vec<f32>)> = held.into_iter().collect();
        held.sort_by_key(|(row, _)| *row);
        for query in clumped(20, 16, 40, 9) {
            let found: Vec<ObjRef> =
                index.search(&query, 5, true, None).0.into_iter().take(5).map(|(row, _)| row).collect();
            assert_eq!(found, exact(&held, &query, Metric::Cosine, 5));
        }
    }

    #[test]
    fn confined_to_some_rows_it_finds_the_nearest_of_those() {
        let vectors = clumped(6_000, 24, 30, 11);
        let mut index = VectorIndex::new(24, Metric::Cosine).unwrap();
        for (at, vector) in vectors.iter().enumerate() {
            index.put(row(at as u32 + 1), vector);
        }
        index.divide();
        // Every third row, and some rows the index has never heard of.
        let chosen: Vec<ObjRef> = (1..=6_000u32).filter(|number| number % 3 == 0).map(row).collect();
        let held: Vec<(ObjRef, Vec<f32>)> =
            chosen.iter().map(|row| (*row, vectors[row.object_id() as usize - 1].clone())).collect();
        for query in clumped(20, 24, 30, 99) {
            let asked = chosen.iter().copied().chain([row(900_000), row(900_001)]);
            let (found, searched) = index.nearest_among(asked, &query, 10);
            let found: Vec<ObjRef> = found.into_iter().take(10).map(|(row, _)| row).collect();
            assert_eq!(found, exact(&held, &query, Metric::Cosine, 10));
            assert_eq!(searched.compared, 2_000);
        }
        assert!(index.nearest_among(std::iter::empty(), &vectors[0], 10).0.is_empty());
    }

    #[test]
    fn a_kept_hint_gives_the_same_lists_back_without_working_them_out() {
        // Three hundred more of the same clumps than the index is first
        // made of: the rows written after the hint was kept.
        let all = clumped(6_300, 24, 30, 11);
        let vectors = &all[..6_000];
        let rows: Vec<(ObjRef, Vec<f32>)> =
            vectors.iter().enumerate().map(|(at, vector)| (row(at as u32 + 1), vector.clone())).collect();
        let mut index = VectorIndex::new(24, Metric::Cosine).unwrap();
        assert!(index.hint().is_none(), "one list is nothing to keep");
        for (row, vector) in &rows {
            index.put(*row, vector);
        }
        index.divide();
        let hint = index.hint().unwrap();

        // The same rows, with some gone and some new since the hint.
        let mut later: Vec<(ObjRef, Vec<f32>)> = rows[500..].to_vec();
        later.extend(all[6_000..].iter().enumerate().map(|(at, vector)| (row(at as u32 + 7_000), vector.clone())));
        let again = VectorIndex::from_hint(24, Metric::Cosine, &hint, &later).expect("the hint suits these rows");
        assert_eq!(again.len(), later.len());
        assert!(!again.wants_dividing(), "the centres are taken as they are");
        let mut right = 0;
        for query in clumped(20, 24, 30, 99) {
            let found: Vec<ObjRef> = again.search(&query, 10, true, None).0.into_iter().take(10).map(|(row, _)| row).collect();
            assert_eq!(found, exact(&later, &query, Metric::Cosine, 10));
            let (near, searched) = again.search(&query, 10, false, None);
            assert!(searched.compared < later.len() / 4);
            let near: Vec<ObjRef> = near.into_iter().take(10).map(|(row, _)| row).collect();
            right += found.iter().filter(|row| near.contains(row)).count();
        }
        // Left to itself it finds about what an index divided afresh from
        // the same rows finds.
        let mut afresh = VectorIndex::new(24, Metric::Cosine).unwrap();
        for (row, vector) in &later {
            afresh.put(*row, vector);
        }
        afresh.divide();
        let mut right_afresh = 0;
        for query in clumped(20, 24, 30, 99) {
            let truth = exact(&later, &query, Metric::Cosine, 10);
            let near: Vec<ObjRef> = afresh.search(&query, 10, false, None).0.into_iter().take(10).map(|(row, _)| row).collect();
            right_afresh += truth.iter().filter(|row| near.contains(row)).count();
        }
        assert!(right + 10 >= right_afresh, "from the hint {right} of 200, afresh {right_afresh}");
        assert!(right >= 150, "from the hint {right} of 200");

        // A hint is refused when it is damaged, is for another length or
        // metric, or names rows whose vectors are no longer what they were.
        let mut damaged = hint.clone();
        damaged[40] ^= 1;
        assert!(VectorIndex::from_hint(24, Metric::Cosine, &damaged, &later).is_none());
        assert!(VectorIndex::from_hint(24, Metric::L2, &hint, &later).is_none());
        assert!(VectorIndex::from_hint(16, Metric::Cosine, &hint, &later).is_none());
        assert!(VectorIndex::from_hint(24, Metric::Cosine, &hint[..hint.len() / 2], &later).is_none());
        let shuffled: Vec<(ObjRef, Vec<f32>)> =
            rows.iter().enumerate().map(|(at, (row, _))| (*row, vectors[(at * 7 + 13) % vectors.len()].clone())).collect();
        assert!(VectorIndex::from_hint(24, Metric::Cosine, &hint, &shuffled).is_none());
    }

    #[test]
    fn rows_as_near_as_the_last_are_all_given() {
        let mut index = VectorIndex::new(2, Metric::Cosine).unwrap();
        for number in 1..=5 {
            index.put(row(number), &[1.0, 0.0]);
        }
        index.put(row(6), &[0.0, 1.0]);
        let (found, _) = index.search(&[1.0, 0.0], 2, false, None);
        assert_eq!(found.len(), 5, "the five that are equally near, not an arbitrary two");
    }
}
