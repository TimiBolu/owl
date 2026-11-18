//! The fields of a row or an object.
//!
//! A row used to hold a map of its own from each field's name to its value:
//! every row its own table, every name its own allocation. Rows of one
//! collection have the same names, so that paid for the same thing once per
//! row and made reading a field a hash and several jumps through memory.
//!
//! Here a row holds its values in order and shares the names with every
//! other row built the same way. The shared part is a [`Shape`]: the names,
//! in the order they were added. Reading a field by name finds its place in
//! the shape and takes the value at that place. Code that reads the same
//! field of many rows can find the place once ([`Fields::place`]) and read
//! by it ([`Fields::at`]) for every row of that shape.
//!
//! The interface is a map's, so a `Fields` is used as one.

use crate::document::Value;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::sync::{Arc, OnceLock, RwLock};

/// A quick hash for field names. A shape's names are looked up by name
/// constantly and its table is shared, so it does not pay for resistance to
/// chosen keys; a shape that grows too many successors stops sharing them
/// instead (see [`Shape::with`]).
#[derive(Default, Clone, Copy)]
pub struct FieldNameHasher(u64);

impl std::hash::Hasher for FieldNameHasher {
    fn finish(&self) -> u64 {
        // The multiply leaves the low bits weakest and a table picks its
        // slot by them, so the strong bits are turned down to meet it.
        self.0.rotate_left(26)
    }

    fn write(&mut self, bytes: &[u8]) {
        const SEED: u64 = 0x517c_c1b7_2722_0a95;
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let word = u64::from_le_bytes(chunk.try_into().expect("eight bytes"));
            self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
        }
        for byte in chunks.remainder() {
            self.0 = (self.0.rotate_left(5) ^ u64::from(*byte)).wrapping_mul(SEED);
        }
    }
}

type ByName<T> = HashMap<String, T, BuildHasherDefault<FieldNameHasher>>;

/// A shape with this many names or fewer is searched from the front; a
/// longer one by its table.
const SEARCHED_IN_ORDER: usize = 6;

/// A shape shares at most this many successors. Past it, a new name gets a
/// shape of its own, so that rows whose names are data (ids used as keys,
/// say) cannot grow the shared shapes without limit.
const MOST_SUCCESSORS: usize = 256;

/// The names of some fields, in the order they were added. Shared by every
/// row that added the same names in the same order.
pub struct Shape {
    names: Vec<String>,
    places: ByName<u32>,
    /// The shape reached by adding one more name, for each name added so far.
    successors: RwLock<ByName<Arc<Shape>>>,
}

impl Shape {
    fn empty() -> Arc<Shape> {
        static EMPTY: OnceLock<Arc<Shape>> = OnceLock::new();
        EMPTY
            .get_or_init(|| {
                Arc::new(Shape {
                    names: Vec::new(),
                    places: ByName::default(),
                    successors: RwLock::new(ByName::default()),
                })
            })
            .clone()
    }

    #[inline]
    fn place(&self, name: &str) -> Option<usize> {
        if self.names.len() <= SEARCHED_IN_ORDER {
            self.names.iter().position(|known| known == name)
        } else {
            self.places.get(name).map(|place| *place as usize)
        }
    }

    /// This shape with `name` added at the end.
    fn with(self: &Arc<Self>, name: &str) -> Arc<Shape> {
        if let Some(known) = self
            .successors
            .read()
            .ok()
            .and_then(|successors| successors.get(name).cloned())
        {
            return known;
        }
        let mut names = Vec::with_capacity(self.names.len() + 1);
        names.extend(self.names.iter().cloned());
        names.push(name.to_string());
        let places = names
            .iter()
            .enumerate()
            .map(|(place, name)| (name.clone(), place as u32))
            .collect();
        let made = Arc::new(Shape {
            names,
            places,
            successors: RwLock::new(ByName::default()),
        });
        let Ok(mut successors) = self.successors.write() else {
            return made;
        };
        if let Some(known) = successors.get(name) {
            return known.clone();
        }
        if successors.len() < MOST_SUCCESSORS {
            successors.insert(name.to_string(), made.clone());
        }
        made
    }
}

/// The fields of a row or an object, by name. See the module's description.
#[derive(Clone)]
pub struct Fields {
    shape: Arc<Shape>,
    /// One for each of the shape's names. `None` where the field was taken
    /// out again.
    values: Vec<Option<Value>>,
}

impl Default for Fields {
    fn default() -> Self {
        Self {
            shape: Shape::empty(),
            values: Vec::new(),
        }
    }
}

impl Fields {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            shape: Shape::empty(),
            values: Vec::with_capacity(capacity),
        }
    }

    /// As a map built with a hasher is; the hasher is not used.
    pub fn with_capacity_and_hasher(capacity: usize, _hasher: ()) -> Self {
        Self::with_capacity(capacity)
    }

    /// Something that is the same for two sets of fields exactly when they
    /// have the same shape, so that a place found for one holds for the
    /// other.
    #[inline]
    pub fn shape_id(&self) -> usize {
        Arc::as_ptr(&self.shape) as usize
    }

    /// Where `name` is held in fields of this shape, whether or not these
    /// fields have a value there.
    #[inline]
    pub fn place(&self, name: &str) -> Option<usize> {
        self.shape.place(name)
    }

    /// The value at a place that [`Self::place`] gave for fields of this
    /// shape.
    #[inline]
    pub fn at(&self, place: usize) -> Option<&Value> {
        self.values.get(place)?.as_ref()
    }

    #[inline]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values[self.shape.place(name)?].as_ref()
    }

    pub fn get_key_value(&self, name: &str) -> Option<(&String, &Value)> {
        let place = self.shape.place(name)?;
        Some((&self.shape.names[place], self.values[place].as_ref()?))
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        let place = self.shape.place(name)?;
        self.values[place].as_mut()
    }

    #[inline]
    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Set a field, giving back the value it had.
    pub fn insert(&mut self, name: String, value: Value) -> Option<Value> {
        self.set(&name, value)
    }

    /// [`Self::insert`] for a name that is borrowed.
    pub fn set(&mut self, name: &str, value: Value) -> Option<Value> {
        match self.shape.place(name) {
            Some(place) => self.values[place].replace(value),
            None => {
                self.shape = self.shape.with(name);
                self.values.push(Some(value));
                None
            }
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        let place = self.shape.place(name)?;
        self.values[place].take()
    }

    pub fn len(&self) -> usize {
        self.values.iter().filter(|value| value.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.values.iter().all(Option::is_none)
    }

    pub fn clear(&mut self) {
        self.shape = Shape::empty();
        self.values.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> + '_ {
        self.shape
            .names
            .iter()
            .zip(&self.values)
            .filter_map(|(name, value)| Some((name, value.as_ref()?)))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut Value)> + '_ {
        self.shape
            .names
            .iter()
            .zip(&mut self.values)
            .filter_map(|(name, value)| Some((name, value.as_mut()?)))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> + '_ {
        self.iter().map(|(name, _)| name)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> + '_ {
        self.values.iter().flatten()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Value> + '_ {
        self.values.iter_mut().flatten()
    }

    /// Keep the fields `keep` says to.
    pub fn retain(&mut self, mut keep: impl FnMut(&String, &mut Value) -> bool) {
        for (name, value) in self.shape.names.iter().zip(&mut self.values) {
            if value.as_mut().is_some_and(|held| !keep(name, held)) {
                *value = None;
            }
        }
    }

    /// The value of a field, set first to what `make` gives when it has
    /// none.
    pub fn get_or_insert_with(&mut self, name: &str, make: impl FnOnce() -> Value) -> &mut Value {
        let place = match self.shape.place(name) {
            Some(place) => place,
            None => {
                self.shape = self.shape.with(name);
                self.values.push(None);
                self.values.len() - 1
            }
        };
        self.values[place].get_or_insert_with(make)
    }

    pub fn entry(&mut self, name: String) -> Entry<'_> {
        Entry { fields: self, name }
    }
}

/// Reads one field, by name, from many sets of fields. The field's place
/// is found once for each shape met, not once for each read, so reading a
/// field of every row of a collection costs little more than indexing.
pub struct FieldReader {
    name: String,
    /// The shapes read from lately and the field's place in each. Rows of
    /// one collection come in a few shapes, by which of its optional fields
    /// each holds, and they arrive mixed. A shape is held, so that another
    /// cannot take its address meanwhile.
    seen: std::cell::RefCell<Vec<(Arc<Shape>, Option<usize>)>>,
}

/// A reader remembers this many shapes.
const SHAPES_REMEMBERED: usize = 8;

impl FieldReader {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            seen: std::cell::RefCell::new(Vec::new()),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn read<'a>(&self, fields: &'a Fields) -> Option<&'a Value> {
        let mut seen = self.seen.borrow_mut();
        let known = seen
            .iter()
            .find(|(shape, _)| Arc::ptr_eq(shape, &fields.shape))
            .map(|(_, place)| *place);
        let place = match known {
            Some(place) => place,
            None => {
                let place = fields.shape.place(&self.name);
                if seen.len() == SHAPES_REMEMBERED {
                    seen.remove(0);
                }
                seen.push((fields.shape.clone(), place));
                place
            }
        };
        fields.values[place?].as_ref()
    }
}

/// A field that may or may not be set, as a map's entry is.
pub struct Entry<'a> {
    fields: &'a mut Fields,
    name: String,
}

impl<'a> Entry<'a> {
    pub fn or_insert(self, value: Value) -> &'a mut Value {
        self.fields.get_or_insert_with(&self.name, || value)
    }

    pub fn or_insert_with(self, make: impl FnOnce() -> Value) -> &'a mut Value {
        self.fields.get_or_insert_with(&self.name, make)
    }
}

impl std::ops::Index<&str> for Fields {
    type Output = Value;

    fn index(&self, name: &str) -> &Value {
        self.get(name).expect("no such field")
    }
}

/// Two sets of fields are equal when they hold the same values under the
/// same names, in whatever order they were added.
impl PartialEq for Fields {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.shape, &other.shape) {
            return self.values == other.values;
        }
        self.len() == other.len()
            && self
                .iter()
                .all(|(name, value)| other.get(name).is_some_and(|theirs| theirs == value))
    }
}

impl std::fmt::Debug for Fields {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl FromIterator<(String, Value)> for Fields {
    fn from_iter<Entries: IntoIterator<Item = (String, Value)>>(entries: Entries) -> Self {
        let entries = entries.into_iter();
        let mut fields = Fields::with_capacity(entries.size_hint().0);
        fields.extend(entries);
        fields
    }
}

impl Extend<(String, Value)> for Fields {
    fn extend<Entries: IntoIterator<Item = (String, Value)>>(&mut self, entries: Entries) {
        for (name, value) in entries {
            self.set(&name, value);
        }
    }
}

impl<const COUNT: usize> From<[(String, Value); COUNT]> for Fields {
    fn from(entries: [(String, Value); COUNT]) -> Self {
        entries.into_iter().collect()
    }
}

impl From<HashMap<String, Value>> for Fields {
    fn from(entries: HashMap<String, Value>) -> Self {
        entries.into_iter().collect()
    }
}

impl IntoIterator for Fields {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.shape
            .names
            .iter()
            .zip(self.values)
            .filter_map(|(name, value)| Some((name.clone(), value?)))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl<'a> IntoIterator for &'a Fields {
    type Item = (&'a String, &'a Value);
    type IntoIter = Box<dyn Iterator<Item = (&'a String, &'a Value)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl<'a> IntoIterator for &'a mut Fields {
    type Item = (&'a String, &'a mut Value);
    type IntoIter = Box<dyn Iterator<Item = (&'a String, &'a mut Value)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter_mut())
    }
}

/// Written as a map is, so that what was written before reads back.
impl Serialize for Fields {
    fn serialize<Target: Serializer>(&self, serializer: Target) -> Result<Target::Ok, Target::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (name, value) in self.iter() {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<Source: Deserializer<'de>>(deserializer: Source) -> Result<Self, Source::Error> {
        struct Reader;

        impl<'de> Visitor<'de> for Reader {
            type Value = Fields;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of field names to values")
            }

            fn visit_map<Entries: MapAccess<'de>>(self, mut entries: Entries) -> Result<Fields, Entries::Error> {
                let mut fields = Fields::with_capacity(entries.size_hint().unwrap_or(0));
                while let Some((name, value)) = entries.next_entry::<String, Value>()? {
                    fields.set(&name, value);
                }
                Ok(fields)
            }
        }

        deserializer.deserialize_map(Reader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(value: i64) -> Value {
        Value::Integer(value)
    }

    #[test]
    fn fields_behave_as_a_map() {
        let mut fields = Fields::new();
        assert!(fields.is_empty());
        assert_eq!(fields.insert("a".into(), int(1)), None);
        assert_eq!(fields.insert("b".into(), int(2)), None);
        assert_eq!(fields.insert("a".into(), int(3)), Some(int(1)));
        assert_eq!(fields.len(), 2);
        assert_eq!(fields.get("a"), Some(&int(3)));
        assert_eq!(fields.get("c"), None);
        assert!(fields.contains_key("b") && !fields.contains_key("c"));
        assert_eq!(fields.remove("a"), Some(int(3)));
        assert_eq!(fields.remove("a"), None);
        assert_eq!(fields.len(), 1);
        assert!(!fields.contains_key("a"));
        // A field taken out can be put back, and is not seen meanwhile.
        assert_eq!(fields.keys().collect::<Vec<_>>(), ["b"]);
        fields.insert("a".into(), int(4));
        assert_eq!(fields.iter().count(), 2);
        *fields.entry("n".into()).or_insert(int(0)) = int(9);
        assert_eq!(fields["n"], int(9));
        fields.retain(|name, _| name != "b");
        assert_eq!(fields.len(), 2);
    }

    #[test]
    fn rows_built_alike_share_a_shape_and_places() {
        let build = |pay: i64| -> Fields {
            [("id".to_string(), int(pay)), ("pay".to_string(), int(pay * 10))].into()
        };
        let (one, two) = (build(1), build(2));
        assert_eq!(one.shape_id(), two.shape_id());
        let place = one.place("pay").unwrap();
        assert_eq!(two.at(place), Some(&int(20)));
        // The same names in another order are another shape, and equal.
        let other: Fields = [("pay".to_string(), int(10)), ("id".to_string(), int(1))].into();
        assert_ne!(other.shape_id(), one.shape_id());
        assert_eq!(other, one);
        assert_ne!(other, two);
    }

    #[test]
    fn a_reader_follows_rows_from_shape_to_shape() {
        let reader = FieldReader::new("pay");
        let one: Fields = [("id".to_string(), int(1)), ("pay".to_string(), int(10))].into();
        let two: Fields = [("pay".to_string(), int(20)), ("id".to_string(), int(2))].into();
        let none: Fields = [("id".to_string(), int(3))].into();
        let mut gone = one.clone();
        gone.remove("pay");
        for _ in 0..2 {
            assert_eq!(reader.read(&one), Some(&int(10)));
            assert_eq!(reader.read(&two), Some(&int(20)));
            assert_eq!(reader.read(&none), None);
            assert_eq!(reader.read(&gone), None);
        }
    }

    #[test]
    fn many_names_are_found_and_names_that_are_data_do_not_pile_up() {
        let mut wide = Fields::new();
        for field in 0..40 {
            wide.insert(format!("f{field}"), int(field));
        }
        for field in 0..40 {
            assert_eq!(wide.get(&format!("f{field}")), Some(&int(field)));
        }
        // Thousands of one-off names from the same start.
        let start: Fields = [("kind".to_string(), int(0))].into();
        for name in 0..5_000 {
            let mut row = start.clone();
            row.insert(format!("key-{name}"), int(name));
            assert_eq!(row.get(&format!("key-{name}")), Some(&int(name)));
        }
        assert!(start.shape.successors.read().unwrap().len() <= MOST_SUCCESSORS);
    }

    #[test]
    fn fields_are_written_and_read_as_a_map() {
        let fields: Fields = [("a".to_string(), int(1)), ("b".to_string(), Value::Null)].into();
        let as_map: HashMap<String, Value> = fields.clone().into_iter().collect();
        assert_eq!(bincode::serialize(&fields).unwrap().len(), bincode::serialize(&as_map).unwrap().len());
        let back: Fields = bincode::deserialize(&bincode::serialize(&as_map).unwrap()).unwrap();
        assert_eq!(back, fields);
        let json = serde_json::to_string(&fields).unwrap();
        assert_eq!(serde_json::from_str::<Fields>(&json).unwrap(), fields);
    }
}
