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
//! It is built from the rows and never stored: a database opened afresh
//! builds it on the first search that wants it.

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
/// Rounds of moving each centre to the middle of its vectors.
const ROUNDS: usize = 6;

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
        self.remove(row);
        if vector.len() != self.dimension {
            return;
        }
        let kept = self.kept(vector);
        let list = self.list_of(&kept);
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
        let nearer = |a: &(ObjRef, f32), b: &(ObjRef, f32)| b.1.total_cmp(&a.1);
        if found.len() > k {
            found.select_nth_unstable_by(k - 1, nearer);
            let last = found[k - 1].1;
            // Keep what is as near as the last of the k.
            found.retain(|(_, nearness)| *nearness >= last);
        }
        found.sort_by(nearer);
        (found, searched)
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
