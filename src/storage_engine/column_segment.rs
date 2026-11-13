use super::{CompiledSchema, ObjRef};
use crate::document::{Document, Value};
use crate::error::{OQLError, OQLResult};
use crate::schema::{FieldType, Schema};
use chrono::{DateTime, TimeZone, Utc};
use std::sync::Arc;

const MAGIC: [u8; 4] = *b"OWLC";
const VERSION_V1: u16 = 1;
const VERSION_V2: u16 = 2;
const VERSION_V3: u16 = 3;
const VERSION: u16 = 4;
const HEADER_LEN: usize = 64;
const COLUMN_HEADER_LEN: usize = 48;
const MAX_ROWS: usize = 65_536;

const KIND_STRING: u8 = 1;
const KIND_INTEGER: u8 = 2;
const KIND_FLOAT: u8 = 3;
const KIND_BOOLEAN: u8 = 4;
const KIND_DATETIME: u8 = 5;
const KIND_REFERENCE: u8 = 6;
const FLAG_INTEGER_STATS: u8 = 1;
const FLAG_VALUES_LZ4: u8 = 1 << 1;
const FLAG_AUXILIARY_LZ4: u8 = 1 << 2;
const COMPRESSION_MIN_BYTES: usize = 256;
const MAX_TYPED_COLUMN_BYTES: usize = 1 << 30;

/// One immutable, schema-version-specific columnar row group.
///
/// The `authoritative` bit is deliberately persisted rather than inferred
/// from the absence of a row record. Authoritative segments contain every
/// logical field in their compiled schema and can reconstruct exact documents;
/// accelerator segments may omit field kinds without a typed codec.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TypedSegment {
    pub authoritative: bool,
    pub collection_id: u16,
    pub schema_version: u32,
    pub object_refs: Vec<ObjRef>,
    pub document_ids: Vec<String>,
    pub document_versions: Vec<u64>,
    pub created_at: Vec<DateTime<Utc>>,
    pub updated_at: Vec<DateTime<Utc>>,
    pub columns: Vec<TypedColumn>,
}

/// Fixed-size metadata needed to discover a typed row group without reading
/// its identity arrays or column payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypedSegmentDescriptor {
    pub format_version: u16,
    pub authoritative: bool,
    pub collection_id: u16,
    pub schema_version: u32,
    pub row_count: usize,
    pub column_count: usize,
    pub lookup_len: usize,
    pub identity_len: usize,
    pub lookup_checksum: Option<u32>,
    pub encoded_len: usize,
}

/// Compact, independently checksummed identity prefix for one immutable row
/// group. It keeps IDs in the persisted byte array and performs no per-row
/// `String` or map allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct TypedSegmentIdentity {
    pub descriptor: TypedSegmentDescriptor,
    bytes: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TypedColumn {
    pub field_id: u16,
    pub presence: Vec<u8>,
    pub nulls: Vec<u8>,
    pub values: ColumnValues,
    pub integer_min: Option<i64>,
    pub integer_max: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ColumnValues {
    Strings { offsets: Vec<u32>, bytes: Vec<u8> },
    Integers(Vec<i64>),
    Floats(Vec<f64>),
    Booleans(Vec<u8>),
    DateTimes(Vec<DateTime<Utc>>),
    References(Vec<ObjRef>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TypedCell<'a> {
    Missing,
    Null,
    String(&'a str),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Reference(ObjRef),
}

impl TypedSegment {
    pub(crate) fn descriptor(header: &[u8]) -> OQLResult<TypedSegmentDescriptor> {
        if header.len() < HEADER_LEN || header[0..4] != MAGIC {
            return Err(storage_error("invalid typed segment header"));
        }
        let version = get_u16(header, 4)?;
        if !matches!(version, VERSION_V1 | VERSION_V2 | VERSION_V3 | VERSION)
            || get_u16(header, 6)? as usize != HEADER_LEN
        {
            return Err(storage_error("unsupported typed segment format"));
        }
        let authoritative = if version >= VERSION_V2 {
            match header[36] {
                0 => false,
                1 => true,
                _ => return Err(storage_error("typed segment flags are invalid")),
            }
        } else {
            false
        };
        let row_count = get_u32(header, 16)? as usize;
        let ids_len = get_u32(header, 20)? as usize;
        let body_len = usize::try_from(get_u64(header, 24)?)
            .map_err(|_| storage_error("typed segment length overflows this platform"))?;
        let identity_len = row_count
            .checked_mul(40)
            .and_then(|length| length.checked_add((row_count + 1) * 4))
            .and_then(|length| length.checked_add(ids_len))
            .ok_or_else(|| storage_error("typed segment metadata length overflow"))?;
        let lookup_len = if version >= VERSION {
            let encoded_lookup_len = get_u32(header, 44)? as usize;
            let expected_lookup_len = row_count
                .checked_mul(8)
                .and_then(|length| length.checked_add((row_count + 1) * 4))
                .and_then(|length| length.checked_add(ids_len))
                .ok_or_else(|| storage_error("typed segment lookup length overflow"))?;
            if encoded_lookup_len != expected_lookup_len {
                return Err(storage_error("typed segment lookup length is invalid"));
            }
            encoded_lookup_len
        } else {
            identity_len
        };
        let encoded_len = HEADER_LEN
            .checked_add(body_len)
            .ok_or_else(|| storage_error("typed segment length overflow"))?;
        if row_count > MAX_ROWS || identity_len > body_len {
            return Err(storage_error(
                "typed segment length or row count is invalid",
            ));
        }
        Ok(TypedSegmentDescriptor {
            format_version: version,
            authoritative,
            collection_id: get_u16(header, 8)?,
            schema_version: get_u32(header, 12)?,
            row_count,
            column_count: get_u16(header, 10)? as usize,
            lookup_len,
            identity_len,
            lookup_checksum: (version >= VERSION_V3)
                .then(|| get_u32(header, 40))
                .transpose()?,
            encoded_len,
        })
    }

    pub(crate) fn build(
        compiled: &CompiledSchema,
        schema: &Schema,
        rows: &[(ObjRef, Arc<Document>)],
    ) -> OQLResult<Self> {
        if rows.len() > MAX_ROWS {
            return Err(storage_error("typed segment exceeds its row limit"));
        }
        let mut segment = Self {
            authoritative: false,
            collection_id: compiled.collection_id,
            schema_version: compiled.version,
            object_refs: Vec::with_capacity(rows.len()),
            document_ids: Vec::with_capacity(rows.len()),
            document_versions: Vec::with_capacity(rows.len()),
            created_at: Vec::with_capacity(rows.len()),
            updated_at: Vec::with_capacity(rows.len()),
            columns: Vec::new(),
        };
        for (object_ref, document) in rows {
            if object_ref.collection_id() != compiled.collection_id
                || document.schema != compiled.name
            {
                return Err(storage_error(
                    "typed segment row conflicts with its compiled schema",
                ));
            }
            segment.object_refs.push(*object_ref);
            segment.document_ids.push(document.id.clone());
            segment.document_versions.push(document.version);
            segment.created_at.push(document.created_at);
            segment.updated_at.push(document.updated_at);
        }

        for field in &compiled.fields {
            let Some(field_type) = schema.fields.get(&field.name) else {
                continue;
            };
            let Some(kind) = column_kind(field_type) else {
                continue;
            };
            // Distributed references need a wider physical representation than
            // the single-node ObjRef column. The typed segment is only an
            // accelerator, so omit this field instead of making checkpoint
            // publication fail or silently narrowing the reference.
            if kind == KIND_REFERENCE
                && rows.iter().any(|(_, document)| {
                    matches!(
                        document.fields.get(&field.name),
                        Some(Value::GlobalReference(_))
                    )
                })
            {
                continue;
            }
            segment
                .columns
                .push(build_column(field.id, &field.name, kind, rows)?);
        }
        segment.columns.sort_by_key(|column| column.field_id);
        segment.authoritative = segment.columns.len() == compiled.fields.len();
        Ok(segment)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn decode_identity(
        descriptor: TypedSegmentDescriptor,
        bytes: Arc<[u8]>,
    ) -> OQLResult<TypedSegmentIdentity> {
        Self::decode_identity_inner(descriptor, bytes, true)
    }

    pub(crate) fn open_identity(
        descriptor: TypedSegmentDescriptor,
        bytes: Arc<[u8]>,
    ) -> OQLResult<TypedSegmentIdentity> {
        Self::decode_identity_inner(descriptor, bytes, false)
    }

    fn decode_identity_inner(
        descriptor: TypedSegmentDescriptor,
        bytes: Arc<[u8]>,
        audit: bool,
    ) -> OQLResult<TypedSegmentIdentity> {
        if bytes.len() != descriptor.lookup_len {
            return Err(storage_error("typed segment lookup prefix is truncated"));
        }
        let Some(expected_checksum) = descriptor.lookup_checksum else {
            return Err(storage_error(
                "typed segment format has no independent lookup checksum",
            ));
        };
        if crc32fast::hash(&bytes) != expected_checksum {
            return Err(storage_error("typed segment lookup checksum mismatch"));
        }
        let identity = TypedSegmentIdentity { descriptor, bytes };
        if audit {
            identity.validate()?;
        }
        Ok(identity)
    }

    pub(crate) fn row_count(&self) -> usize {
        self.object_refs.len()
    }

    pub(crate) fn column(&self, field_id: u16) -> Option<&TypedColumn> {
        self.columns
            .binary_search_by_key(&field_id, |column| column.field_id)
            .ok()
            .map(|index| &self.columns[index])
    }

    pub(crate) fn encode(&self) -> OQLResult<Vec<u8>> {
        self.validate()?;
        let row_count = self.row_count();
        let mut ids = Vec::new();
        let mut id_offsets = Vec::with_capacity(row_count + 1);
        id_offsets.push(0u32);
        for id in &self.document_ids {
            ids.extend_from_slice(id.as_bytes());
            id_offsets.push(as_u32(ids.len(), "typed segment document IDs")?);
        }

        let mut body = Vec::new();
        for object_ref in &self.object_refs {
            body.extend_from_slice(&object_ref.packed().to_le_bytes());
        }
        for offset in &id_offsets {
            body.extend_from_slice(&offset.to_le_bytes());
        }
        body.extend_from_slice(&ids);
        let lookup_len = body.len();
        for version in &self.document_versions {
            body.extend_from_slice(&version.to_le_bytes());
        }
        for timestamp in &self.created_at {
            encode_timestamp(*timestamp, &mut body);
        }
        for timestamp in &self.updated_at {
            encode_timestamp(*timestamp, &mut body);
        }
        for column in &self.columns {
            encode_column(column, row_count, &mut body)?;
        }
        let mut header = [0u8; HEADER_LEN];
        header[0..4].copy_from_slice(&MAGIC);
        put_u16(&mut header, 4, VERSION);
        put_u16(&mut header, 6, HEADER_LEN as u16);
        put_u16(&mut header, 8, self.collection_id);
        put_u16(
            &mut header,
            10,
            as_u16(self.columns.len(), "typed segment columns")?,
        );
        put_u32(&mut header, 12, self.schema_version);
        put_u32(&mut header, 16, as_u32(row_count, "typed segment rows")?);
        put_u32(&mut header, 20, as_u32(ids.len(), "typed segment IDs")?);
        put_u64(&mut header, 24, body.len() as u64);
        put_u32(&mut header, 32, crc32fast::hash(&body));
        header[36] = u8::from(self.authoritative);
        put_u32(&mut header, 40, crc32fast::hash(&body[..lookup_len]));
        put_u32(
            &mut header,
            44,
            as_u32(lookup_len, "typed segment lookup prefix")?,
        );
        let mut encoded = Vec::with_capacity(HEADER_LEN + body.len());
        encoded.extend_from_slice(&header);
        encoded.extend_from_slice(&body);
        Ok(encoded)
    }

    pub(crate) fn decode(encoded: &[u8]) -> OQLResult<Self> {
        let descriptor = Self::descriptor(encoded)?;
        let authoritative = descriptor.authoritative;
        let collection_id = descriptor.collection_id;
        let column_count = descriptor.column_count;
        let schema_version = descriptor.schema_version;
        let row_count = descriptor.row_count;
        let ids_len = get_u32(encoded, 20)? as usize;
        let body = &encoded[HEADER_LEN..];
        if descriptor.encoded_len != encoded.len()
            || crc32fast::hash(body) != get_u32(encoded, 32)?
            || descriptor
                .lookup_checksum
                .is_some_and(|checksum| checksum != crc32fast::hash(&body[..descriptor.lookup_len]))
        {
            return Err(storage_error(
                "typed segment length, row count, or checksum is invalid",
            ));
        }
        let fixed = descriptor.identity_len;
        if fixed > body.len() {
            return Err(storage_error("typed segment metadata is truncated"));
        }
        let mut cursor = 0;
        let object_refs = read_u64_array(body, &mut cursor, row_count)?
            .into_iter()
            .map(ObjRef::new)
            .collect();
        let (offsets, id_bytes, document_versions, created_at, updated_at) =
            if descriptor.format_version >= VERSION {
                let offsets = read_u32_array(body, &mut cursor, row_count + 1)?;
                let id_bytes = take(body, &mut cursor, ids_len)?;
                let document_versions = read_u64_array(body, &mut cursor, row_count)?;
                let created_at = read_datetime_array(body, &mut cursor, row_count)?;
                let updated_at = read_datetime_array(body, &mut cursor, row_count)?;
                (offsets, id_bytes, document_versions, created_at, updated_at)
            } else {
                let document_versions = read_u64_array(body, &mut cursor, row_count)?;
                let created_at = read_datetime_array(body, &mut cursor, row_count)?;
                let updated_at = read_datetime_array(body, &mut cursor, row_count)?;
                let offsets = read_u32_array(body, &mut cursor, row_count + 1)?;
                let id_bytes = take(body, &mut cursor, ids_len)?;
                (offsets, id_bytes, document_versions, created_at, updated_at)
            };
        let document_ids = decode_strings(&offsets, id_bytes, row_count)?;
        let mut columns = Vec::with_capacity(column_count);
        for _ in 0..column_count {
            columns.push(decode_column(body, &mut cursor, row_count)?);
        }
        if cursor != body.len() {
            return Err(storage_error("typed segment has trailing bytes"));
        }
        let segment = Self {
            authoritative,
            collection_id,
            schema_version,
            object_refs,
            document_ids,
            document_versions,
            created_at,
            updated_at,
            columns,
        };
        segment.validate()?;
        Ok(segment)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn decode_all(mut encoded: &[u8]) -> OQLResult<Vec<Self>> {
        let mut segments = Vec::new();
        while !encoded.is_empty() {
            let descriptor = Self::descriptor(encoded)?;
            let length = descriptor.encoded_len;
            if length > encoded.len() {
                return Err(storage_error("typed segment region length is invalid"));
            }
            segments.push(Self::decode(&encoded[..length])?);
            encoded = &encoded[length..];
        }
        Ok(segments)
    }

    fn validate(&self) -> OQLResult<()> {
        let rows = self.row_count();
        if rows > MAX_ROWS
            || self.document_ids.len() != rows
            || self.document_versions.len() != rows
            || self.created_at.len() != rows
            || self.updated_at.len() != rows
            || self
                .object_refs
                .iter()
                .any(|object_ref| object_ref.collection_id() != self.collection_id)
        {
            return Err(storage_error("typed segment row arrays are inconsistent"));
        }
        let bitmap_len = rows.div_ceil(8);
        let mut previous = None;
        for column in &self.columns {
            if previous.is_some_and(|field_id| field_id >= column.field_id)
                || column.presence.len() != bitmap_len
                || column.nulls.len() != bitmap_len
            {
                return Err(storage_error(
                    "typed segment column directory is inconsistent",
                ));
            }
            for index in 0..rows {
                if bit(&column.nulls, index) && !bit(&column.presence, index) {
                    return Err(storage_error(
                        "typed segment null bitmap marks a missing value",
                    ));
                }
            }
            column.values.validate(rows)?;
            previous = Some(column.field_id);
        }
        Ok(())
    }

    pub(crate) fn decode_document(
        &self,
        row: usize,
        compiled: &CompiledSchema,
    ) -> OQLResult<Document> {
        if !self.authoritative
            || row >= self.row_count()
            || compiled.collection_id != self.collection_id
            || compiled.version != self.schema_version
            || compiled.fields.len() != self.columns.len()
        {
            return Err(storage_error(
                "typed segment cannot authoritatively reconstruct this row",
            ));
        }
        let mut fields = std::collections::HashMap::with_capacity(compiled.fields.len());
        for field in &compiled.fields {
            let column = self
                .column(field.id)
                .ok_or_else(|| storage_error("authoritative typed segment is missing a field"))?;
            let value = match column.cell(row)? {
                TypedCell::Missing => continue,
                TypedCell::Null => Value::Null,
                TypedCell::String(value) => Value::String(value.to_owned()),
                TypedCell::Integer(value) => Value::Integer(value),
                TypedCell::Float(value) => Value::Float(value),
                TypedCell::Boolean(value) => Value::Boolean(value),
                TypedCell::DateTime(value) => Value::DateTime(value),
                TypedCell::Reference(value) => Value::Reference(value),
            };
            fields.insert(field.name.clone(), value);
        }
        Ok(Document {
            id: self.document_ids[row].clone(),
            schema: compiled.name.clone(),
            fields,
            created_at: self.created_at[row],
            updated_at: self.updated_at[row],
            version: self.document_versions[row],
        })
    }

    pub(crate) fn matches_schema(&self, compiled: &CompiledSchema, schema: &Schema) -> bool {
        if compiled.collection_id != self.collection_id
            || compiled.version != self.schema_version
            || (self.authoritative && self.columns.len() != compiled.fields.len())
        {
            return false;
        }
        self.columns.iter().all(|column| {
            compiled
                .fields
                .iter()
                .find(|field| field.id == column.field_id)
                .and_then(|field| schema.fields.get(&field.name))
                .and_then(column_kind)
                == Some(column.values.kind())
        })
    }
}

impl TypedSegmentIdentity {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn object_ref(&self, row: usize) -> OQLResult<ObjRef> {
        if row >= self.descriptor.row_count {
            return Err(storage_error("typed identity row is out of bounds"));
        }
        Ok(ObjRef::new(get_u64(&self.bytes, row * 8)?))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn document_id(&self, row: usize) -> OQLResult<&str> {
        if row >= self.descriptor.row_count {
            return Err(storage_error("typed identity row is out of bounds"));
        }
        let offsets_start = if self.descriptor.format_version >= VERSION {
            self.descriptor.row_count * 8
        } else {
            self.descriptor.row_count * 40
        };
        let ids_start = offsets_start + (self.descriptor.row_count + 1) * 4;
        let start = get_u32(&self.bytes, offsets_start + row * 4)? as usize;
        let end = get_u32(&self.bytes, offsets_start + (row + 1) * 4)? as usize;
        let bytes = self
            .bytes
            .get(ids_start + start..ids_start + end)
            .ok_or_else(|| storage_error("typed identity ID offset is invalid"))?;
        std::str::from_utf8(bytes)
            .map_err(|_| storage_error("typed identity document ID is not UTF-8"))
    }

    pub(crate) fn find_document_id(&self, id: &str) -> OQLResult<Option<usize>> {
        let mut low = 0usize;
        let mut high = self.descriptor.row_count;
        while low < high {
            let middle = low + (high - low) / 2;
            match self.document_id(middle)?.cmp(id) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Ok(Some(middle)),
            }
        }
        Ok(None)
    }

    pub(crate) fn document_version(&self, row: usize) -> OQLResult<u64> {
        if !self.has_row_metadata() {
            return Err(storage_error(
                "typed lookup prefix does not contain row metadata",
            ));
        }
        if row >= self.descriptor.row_count {
            return Err(storage_error("typed identity row is out of bounds"));
        }
        get_u64(&self.bytes, self.descriptor.row_count * 8 + row * 8)
    }

    pub(crate) fn created_at(&self, row: usize) -> OQLResult<DateTime<Utc>> {
        if !self.has_row_metadata() {
            return Err(storage_error(
                "typed lookup prefix does not contain row metadata",
            ));
        }
        if row >= self.descriptor.row_count {
            return Err(storage_error("typed identity row is out of bounds"));
        }
        decode_timestamp_at(&self.bytes, self.descriptor.row_count * 16 + row * 12)
    }

    pub(crate) fn updated_at(&self, row: usize) -> OQLResult<DateTime<Utc>> {
        if !self.has_row_metadata() {
            return Err(storage_error(
                "typed lookup prefix does not contain row metadata",
            ));
        }
        if row >= self.descriptor.row_count {
            return Err(storage_error("typed identity row is out of bounds"));
        }
        decode_timestamp_at(&self.bytes, self.descriptor.row_count * 28 + row * 12)
    }

    pub(crate) fn has_row_metadata(&self) -> bool {
        self.descriptor.format_version < VERSION
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> OQLResult<()> {
        let mut previous_id = None;
        for row in 0..self.descriptor.row_count {
            let object_ref = self.object_ref(row)?;
            if object_ref.is_null() || object_ref.collection_id() != self.descriptor.collection_id {
                return Err(storage_error(
                    "typed identity object reference conflicts with its descriptor",
                ));
            }
            let id = self.document_id(row)?;
            if id.is_empty() || previous_id.is_some_and(|previous| previous >= id) {
                return Err(storage_error(
                    "typed identity document IDs are not strictly ordered",
                ));
            }
            previous_id = Some(id);
        }
        Ok(())
    }
}

fn decode_timestamp_at(bytes: &[u8], offset: usize) -> OQLResult<DateTime<Utc>> {
    let seconds = i64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or_else(|| storage_error("typed identity timestamp is truncated"))?
            .try_into()
            .unwrap(),
    );
    let nanos = get_u32(bytes, offset + 8)?;
    Utc.timestamp_opt(seconds, nanos)
        .single()
        .ok_or_else(|| storage_error("typed identity timestamp is invalid"))
}

impl TypedColumn {
    pub(crate) fn is_reference(&self) -> bool {
        matches!(self.values, ColumnValues::References(_))
    }

    pub(crate) fn cell(&self, row: usize) -> OQLResult<TypedCell<'_>> {
        if row >= self.values.len() {
            return Err(storage_error("typed segment row index is out of bounds"));
        }
        if !bit(&self.presence, row) {
            return Ok(TypedCell::Missing);
        }
        if bit(&self.nulls, row) {
            return Ok(TypedCell::Null);
        }
        match &self.values {
            ColumnValues::Strings { offsets, bytes } => {
                let start = offsets[row] as usize;
                let end = offsets[row + 1] as usize;
                Ok(TypedCell::String(
                    std::str::from_utf8(&bytes[start..end])
                        .map_err(|_| storage_error("typed string is not valid UTF-8"))?,
                ))
            }
            ColumnValues::Integers(values) => Ok(TypedCell::Integer(values[row])),
            ColumnValues::Floats(values) => Ok(TypedCell::Float(values[row])),
            ColumnValues::Booleans(values) => Ok(TypedCell::Boolean(values[row] != 0)),
            ColumnValues::DateTimes(values) => Ok(TypedCell::DateTime(values[row])),
            ColumnValues::References(values) => Ok(TypedCell::Reference(values[row])),
        }
    }
}

impl ColumnValues {
    fn kind(&self) -> u8 {
        match self {
            Self::Strings { .. } => KIND_STRING,
            Self::Integers(_) => KIND_INTEGER,
            Self::Floats(_) => KIND_FLOAT,
            Self::Booleans(_) => KIND_BOOLEAN,
            Self::DateTimes(_) => KIND_DATETIME,
            Self::References(_) => KIND_REFERENCE,
        }
    }

    fn validate(&self, rows: usize) -> OQLResult<()> {
        let valid = match self {
            Self::Strings { offsets, bytes } => {
                validate_offsets(offsets, bytes.len(), rows).is_ok()
                    && offsets.windows(2).all(|range| {
                        std::str::from_utf8(&bytes[range[0] as usize..range[1] as usize]).is_ok()
                    })
            }
            Self::Integers(values) => values.len() == rows,
            Self::Floats(values) => values.len() == rows,
            Self::Booleans(values) => {
                values.len() == rows && values.iter().all(|value| *value <= 1)
            }
            Self::DateTimes(values) => values.len() == rows,
            Self::References(values) => values.len() == rows,
        };
        if valid {
            Ok(())
        } else {
            Err(storage_error("typed segment column values are invalid"))
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Strings { offsets, .. } => offsets.len().saturating_sub(1),
            Self::Integers(values) => values.len(),
            Self::Floats(values) => values.len(),
            Self::Booleans(values) => values.len(),
            Self::DateTimes(values) => values.len(),
            Self::References(values) => values.len(),
        }
    }
}

fn build_column(
    field_id: u16,
    field_name: &str,
    kind: u8,
    rows: &[(ObjRef, Arc<Document>)],
) -> OQLResult<TypedColumn> {
    let bitmap_len = rows.len().div_ceil(8);
    let mut presence = vec![0; bitmap_len];
    let mut nulls = vec![0; bitmap_len];
    let mut values = empty_values(kind, rows.len());
    let mut integer_min = None;
    let mut integer_max = None;
    for (index, (_, document)) in rows.iter().enumerate() {
        let Some(value) = document.fields.get(field_name) else {
            push_default(&mut values)?;
            continue;
        };
        set_bit(&mut presence, index);
        if value.is_null() {
            set_bit(&mut nulls, index);
            push_default(&mut values)?;
            continue;
        }
        push_value(&mut values, value)?;
        if let Value::Integer(value) = value {
            integer_min = Some(integer_min.map_or(*value, |current: i64| current.min(*value)));
            integer_max = Some(integer_max.map_or(*value, |current: i64| current.max(*value)));
        }
    }
    finish_strings(&mut values)?;
    Ok(TypedColumn {
        field_id,
        presence,
        nulls,
        values,
        integer_min,
        integer_max,
    })
}

fn empty_values(kind: u8, rows: usize) -> ColumnValues {
    match kind {
        KIND_STRING => ColumnValues::Strings {
            offsets: Vec::with_capacity(rows + 1),
            bytes: Vec::new(),
        },
        KIND_INTEGER => ColumnValues::Integers(Vec::with_capacity(rows)),
        KIND_FLOAT => ColumnValues::Floats(Vec::with_capacity(rows)),
        KIND_BOOLEAN => ColumnValues::Booleans(Vec::with_capacity(rows)),
        KIND_DATETIME => ColumnValues::DateTimes(Vec::with_capacity(rows)),
        KIND_REFERENCE => ColumnValues::References(Vec::with_capacity(rows)),
        _ => unreachable!("column kind was validated"),
    }
}

fn push_default(values: &mut ColumnValues) -> OQLResult<()> {
    match values {
        ColumnValues::Strings { offsets, bytes } => {
            offsets.push(as_u32(bytes.len(), "typed string column")?)
        }
        ColumnValues::Integers(values) => values.push(0),
        ColumnValues::Floats(values) => values.push(0.0),
        ColumnValues::Booleans(values) => values.push(0),
        ColumnValues::DateTimes(values) => values.push(timestamp(0, 0)?),
        ColumnValues::References(values) => values.push(ObjRef::null()),
    }
    Ok(())
}

fn push_value(values: &mut ColumnValues, value: &Value) -> OQLResult<()> {
    match (values, value) {
        (ColumnValues::Strings { offsets, bytes }, Value::String(value)) => {
            offsets.push(as_u32(bytes.len(), "typed string column")?);
            bytes.extend_from_slice(value.as_bytes());
        }
        (ColumnValues::Integers(values), Value::Integer(value)) => values.push(*value),
        (ColumnValues::Floats(values), Value::Float(value)) => values.push(*value),
        (ColumnValues::Booleans(values), Value::Boolean(value)) => values.push(u8::from(*value)),
        (ColumnValues::DateTimes(values), Value::DateTime(value)) => values.push(*value),
        (ColumnValues::References(values), Value::Reference(value)) => values.push(*value),
        _ => {
            return Err(storage_error(
                "typed segment value disagrees with its schema field type",
            ))
        }
    }
    Ok(())
}

fn finish_strings(values: &mut ColumnValues) -> OQLResult<()> {
    if let ColumnValues::Strings { offsets, bytes } = values {
        offsets.push(as_u32(bytes.len(), "typed string column")?);
    }
    Ok(())
}

fn column_kind(field_type: &FieldType) -> Option<u8> {
    match field_type {
        FieldType::Optional(inner) => column_kind(inner),
        FieldType::String => Some(KIND_STRING),
        FieldType::Integer => Some(KIND_INTEGER),
        FieldType::Float => Some(KIND_FLOAT),
        FieldType::Boolean => Some(KIND_BOOLEAN),
        FieldType::DateTime => Some(KIND_DATETIME),
        FieldType::Reference(_) => Some(KIND_REFERENCE),
        FieldType::Object | FieldType::Vector { .. } | FieldType::List(_) => None,
    }
}

fn encode_column(column: &TypedColumn, rows: usize, body: &mut Vec<u8>) -> OQLResult<()> {
    let (value_bytes, auxiliary) = encode_values(&column.values)?;
    let (value_bytes, values_compressed) = compress_if_useful(value_bytes);
    let (auxiliary, auxiliary_compressed) = compress_if_useful(auxiliary);
    let mut header = [0u8; COLUMN_HEADER_LEN];
    put_u16(&mut header, 0, column.field_id);
    header[2] = column.values.kind();
    header[3] = if column.integer_min.is_some() {
        FLAG_INTEGER_STATS
    } else {
        0
    } | if values_compressed {
        FLAG_VALUES_LZ4
    } else {
        0
    } | if auxiliary_compressed {
        FLAG_AUXILIARY_LZ4
    } else {
        0
    };
    put_u32(
        &mut header,
        4,
        as_u32(column.presence.len(), "typed presence bitmap")?,
    );
    put_u32(
        &mut header,
        8,
        as_u32(column.nulls.len(), "typed null bitmap")?,
    );
    put_u64(&mut header, 12, value_bytes.len() as u64);
    put_u64(&mut header, 20, auxiliary.len() as u64);
    put_i64(&mut header, 28, column.integer_min.unwrap_or_default());
    put_i64(&mut header, 36, column.integer_max.unwrap_or_default());
    put_u32(
        &mut header,
        44,
        checksum_parts(&[&column.presence, &column.nulls, &value_bytes, &auxiliary]),
    );
    debug_assert_eq!(column.presence.len(), rows.div_ceil(8));
    body.extend_from_slice(&header);
    body.extend_from_slice(&column.presence);
    body.extend_from_slice(&column.nulls);
    body.extend_from_slice(&value_bytes);
    body.extend_from_slice(&auxiliary);
    Ok(())
}

fn decode_column(body: &[u8], cursor: &mut usize, rows: usize) -> OQLResult<TypedColumn> {
    let header = take(body, cursor, COLUMN_HEADER_LEN)?;
    let field_id = get_u16(header, 0)?;
    let kind = header[2];
    let flags = header[3];
    let has_integer_stats = flags & FLAG_INTEGER_STATS != 0;
    if flags & !(FLAG_INTEGER_STATS | FLAG_VALUES_LZ4 | FLAG_AUXILIARY_LZ4) != 0 {
        return Err(storage_error("typed segment column flags are invalid"));
    }
    let presence_len = get_u32(header, 4)? as usize;
    let null_len = get_u32(header, 8)? as usize;
    let value_len = usize::try_from(get_u64(header, 12)?)
        .map_err(|_| storage_error("typed column value length overflows this platform"))?;
    let auxiliary_len = usize::try_from(get_u64(header, 20)?)
        .map_err(|_| storage_error("typed column auxiliary length overflows this platform"))?;
    if presence_len != rows.div_ceil(8) || null_len != rows.div_ceil(8) {
        return Err(storage_error("typed segment bitmap length is invalid"));
    }
    let presence = take(body, cursor, presence_len)?.to_vec();
    let nulls = take(body, cursor, null_len)?.to_vec();
    let encoded_values = take(body, cursor, value_len)?;
    let encoded_auxiliary = take(body, cursor, auxiliary_len)?;
    let checksum = checksum_parts(&[&presence, &nulls, encoded_values, encoded_auxiliary]);
    if checksum != get_u32(header, 44)? {
        return Err(storage_error("typed segment column checksum is invalid"));
    }
    let expected_values = encoded_value_len(kind, rows)?;
    let value_bytes = decompress_if_flagged(
        encoded_values,
        flags & FLAG_VALUES_LZ4 != 0,
        expected_values,
    )?;
    let auxiliary = decompress_if_flagged(
        encoded_auxiliary,
        flags & FLAG_AUXILIARY_LZ4 != 0,
        MAX_TYPED_COLUMN_BYTES,
    )?;
    if value_bytes.len() != expected_values {
        return Err(storage_error(
            "typed column decompressed value length is invalid",
        ));
    }
    let values = decode_values(kind, &value_bytes, &auxiliary, rows)?;
    Ok(TypedColumn {
        field_id,
        presence,
        nulls,
        values,
        integer_min: has_integer_stats.then(|| get_i64(header, 28)).transpose()?,
        integer_max: has_integer_stats.then(|| get_i64(header, 36)).transpose()?,
    })
}

fn compress_if_useful(bytes: Vec<u8>) -> (Vec<u8>, bool) {
    if bytes.len() < COMPRESSION_MIN_BYTES {
        return (bytes, false);
    }
    let compressed = lz4_flex::compress_prepend_size(&bytes);
    if compressed.len().saturating_add(8) < bytes.len() {
        (compressed, true)
    } else {
        (bytes, false)
    }
}

fn decompress_if_flagged(bytes: &[u8], compressed: bool, maximum: usize) -> OQLResult<Vec<u8>> {
    if !compressed {
        return Ok(bytes.to_vec());
    }
    let declared = bytes
        .get(..4)
        .map(|prefix| u32::from_le_bytes(prefix.try_into().expect("four-byte prefix")) as usize)
        .ok_or_else(|| storage_error("typed column compression frame is truncated"))?;
    if declared > maximum {
        return Err(storage_error(
            "typed column decompressed length exceeds its bound",
        ));
    }
    lz4_flex::decompress_size_prepended(bytes)
        .map_err(|_| storage_error("typed column compression frame is invalid"))
}

fn encoded_value_len(kind: u8, rows: usize) -> OQLResult<usize> {
    let width = match kind {
        KIND_STRING => 4,
        KIND_INTEGER | KIND_FLOAT | KIND_REFERENCE => 8,
        KIND_BOOLEAN => 1,
        KIND_DATETIME => 12,
        _ => return Err(storage_error("typed column kind is unknown")),
    };
    let elements = if kind == KIND_STRING {
        rows.checked_add(1)
    } else {
        Some(rows)
    }
    .ok_or_else(|| storage_error("typed column row count overflow"))?;
    elements
        .checked_mul(width)
        .ok_or_else(|| storage_error("typed column value length overflow"))
}

fn encode_values(values: &ColumnValues) -> OQLResult<(Vec<u8>, Vec<u8>)> {
    let mut bytes = Vec::new();
    let auxiliary = match values {
        ColumnValues::Strings {
            offsets,
            bytes: strings,
        } => {
            for offset in offsets {
                bytes.extend_from_slice(&offset.to_le_bytes());
            }
            strings.clone()
        }
        ColumnValues::Integers(values) => {
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            Vec::new()
        }
        ColumnValues::DateTimes(values) => {
            for value in values {
                encode_timestamp(*value, &mut bytes);
            }
            Vec::new()
        }
        ColumnValues::Floats(values) => {
            for value in values {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            Vec::new()
        }
        ColumnValues::Booleans(values) => {
            bytes.extend_from_slice(values);
            Vec::new()
        }
        ColumnValues::References(values) => {
            for value in values {
                bytes.extend_from_slice(&value.packed().to_le_bytes());
            }
            Vec::new()
        }
    };
    Ok((bytes, auxiliary))
}

fn decode_values(
    kind: u8,
    values: &[u8],
    auxiliary: &[u8],
    rows: usize,
) -> OQLResult<ColumnValues> {
    let mut cursor = 0;
    let decoded = match kind {
        KIND_STRING => {
            let offsets = read_u32_array(values, &mut cursor, rows + 1)?;
            validate_offsets(&offsets, auxiliary.len(), rows)?;
            ColumnValues::Strings {
                offsets,
                bytes: auxiliary.to_vec(),
            }
        }
        KIND_INTEGER => ColumnValues::Integers(read_i64_array(values, &mut cursor, rows)?),
        KIND_FLOAT => ColumnValues::Floats(
            read_u64_array(values, &mut cursor, rows)?
                .into_iter()
                .map(f64::from_bits)
                .collect(),
        ),
        KIND_BOOLEAN => {
            let booleans = take(values, &mut cursor, rows)?.to_vec();
            ColumnValues::Booleans(booleans)
        }
        KIND_DATETIME => ColumnValues::DateTimes(read_datetime_array(values, &mut cursor, rows)?),
        KIND_REFERENCE => ColumnValues::References(
            read_u64_array(values, &mut cursor, rows)?
                .into_iter()
                .map(ObjRef::new)
                .collect(),
        ),
        _ => return Err(storage_error("unknown typed segment column kind")),
    };
    if cursor != values.len() || (kind != KIND_STRING && !auxiliary.is_empty()) {
        return Err(storage_error("typed segment column length is invalid"));
    }
    Ok(decoded)
}

fn validate_offsets(offsets: &[u32], bytes_len: usize, rows: usize) -> OQLResult<()> {
    if offsets.len() != rows + 1
        || offsets.first() != Some(&0)
        || offsets.last().map(|offset| *offset as usize) != Some(bytes_len)
        || offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(storage_error("typed string offsets are invalid"));
    }
    Ok(())
}

fn decode_strings(offsets: &[u32], bytes: &[u8], rows: usize) -> OQLResult<Vec<String>> {
    validate_offsets(offsets, bytes.len(), rows)?;
    offsets
        .windows(2)
        .map(|range| {
            std::str::from_utf8(&bytes[range[0] as usize..range[1] as usize])
                .map(str::to_owned)
                .map_err(|_| storage_error("typed segment string is not valid UTF-8"))
        })
        .collect()
}

fn bit(bitmap: &[u8], index: usize) -> bool {
    bitmap[index / 8] & (1 << (index % 8)) != 0
}

fn set_bit(bitmap: &mut [u8], index: usize) {
    bitmap[index / 8] |= 1 << (index % 8);
}

fn timestamp(seconds: i64, nanoseconds: u32) -> OQLResult<DateTime<Utc>> {
    Utc.timestamp_opt(seconds, nanoseconds)
        .single()
        .ok_or_else(|| storage_error("typed segment timestamp is out of range"))
}

fn encode_timestamp(timestamp: DateTime<Utc>, output: &mut Vec<u8>) {
    output.extend_from_slice(&timestamp.timestamp().to_le_bytes());
    output.extend_from_slice(&timestamp.timestamp_subsec_nanos().to_le_bytes());
}

fn read_datetime_array(
    bytes: &[u8],
    cursor: &mut usize,
    count: usize,
) -> OQLResult<Vec<DateTime<Utc>>> {
    let raw = take(
        bytes,
        cursor,
        count
            .checked_mul(12)
            .ok_or_else(|| storage_error("typed segment datetime array overflow"))?,
    )?;
    raw.chunks_exact(12)
        .map(|chunk| {
            let seconds = i64::from_le_bytes(chunk[..8].try_into().expect("eight-byte seconds"));
            let nanoseconds =
                u32::from_le_bytes(chunk[8..].try_into().expect("four-byte nanoseconds"));
            timestamp(seconds, nanoseconds)
        })
        .collect()
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, length: usize) -> OQLResult<&'a [u8]> {
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| storage_error("typed segment offset overflow"))?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| storage_error("typed segment is truncated"))?;
    *cursor = end;
    Ok(value)
}

fn read_u64_array(bytes: &[u8], cursor: &mut usize, count: usize) -> OQLResult<Vec<u64>> {
    let raw = take(
        bytes,
        cursor,
        count
            .checked_mul(8)
            .ok_or_else(|| storage_error("typed segment array overflow"))?,
    )?;
    Ok(raw
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().expect("eight-byte chunk")))
        .collect())
}

fn read_i64_array(bytes: &[u8], cursor: &mut usize, count: usize) -> OQLResult<Vec<i64>> {
    Ok(read_u64_array(bytes, cursor, count)?
        .into_iter()
        .map(|value| value as i64)
        .collect())
}

fn read_u32_array(bytes: &[u8], cursor: &mut usize, count: usize) -> OQLResult<Vec<u32>> {
    let raw = take(
        bytes,
        cursor,
        count
            .checked_mul(4)
            .ok_or_else(|| storage_error("typed segment array overflow"))?,
    )?;
    Ok(raw
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().expect("four-byte chunk")))
        .collect())
}

fn as_u16(value: usize, context: &str) -> OQLResult<u16> {
    u16::try_from(value).map_err(|_| storage_error(&format!("{context} exceeds its format limit")))
}

fn as_u32(value: usize, context: &str) -> OQLResult<u32> {
    u32::try_from(value).map_err(|_| storage_error(&format!("{context} exceeds its format limit")))
}

fn get_u16(bytes: &[u8], offset: usize) -> OQLResult<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| storage_error("typed segment integer is truncated"))?;
    Ok(u16::from_le_bytes(raw.try_into().expect("two-byte slice")))
}

fn get_u32(bytes: &[u8], offset: usize) -> OQLResult<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| storage_error("typed segment integer is truncated"))?;
    Ok(u32::from_le_bytes(raw.try_into().expect("four-byte slice")))
}

fn get_u64(bytes: &[u8], offset: usize) -> OQLResult<u64> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| storage_error("typed segment integer is truncated"))?;
    Ok(u64::from_le_bytes(
        raw.try_into().expect("eight-byte slice"),
    ))
}

fn get_i64(bytes: &[u8], offset: usize) -> OQLResult<i64> {
    Ok(get_u64(bytes, offset)? as i64)
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn put_i64(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn checksum_parts(parts: &[&[u8]]) -> u32 {
    let mut checksum = crc32fast::Hasher::new();
    for part in parts {
        checksum.update(part);
    }
    checksum.finalize()
}

fn storage_error(message: &str) -> OQLError {
    OQLError::storage_error(message, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::ReferenceType;
    use std::collections::HashMap;

    fn fixture() -> (CompiledSchema, Schema, Vec<(ObjRef, Arc<Document>)>) {
        let schema = Schema::new(
            "events".into(),
            HashMap::from([
                ("active".into(), FieldType::Boolean),
                ("message".into(), FieldType::String),
                (
                    "optional".into(),
                    FieldType::Optional(Box::new(FieldType::String)),
                ),
                ("sequence".into(), FieldType::Integer),
                (
                    "parent".into(),
                    FieldType::Optional(Box::new(FieldType::Reference(ReferenceType::new(
                        "events".into(),
                    )))),
                ),
            ]),
        );
        let compiled = CompiledSchema::first(&schema, 7).unwrap();
        let now = Utc::now();
        let mut first = Document::with_id(
            "a".into(),
            "events".into(),
            HashMap::from([
                ("active".into(), Value::Boolean(true)),
                ("message".into(), Value::String("alpha".into())),
                ("sequence".into(), Value::Integer(-3)),
                ("optional".into(), Value::Null),
                ("parent".into(), Value::Null),
            ]),
        );
        first.created_at = now;
        first.updated_at = now;
        first.version = 4;
        let mut second = Document::with_id(
            "b".into(),
            "events".into(),
            HashMap::from([
                ("active".into(), Value::Boolean(false)),
                ("message".into(), Value::String("βeta".into())),
                ("sequence".into(), Value::Integer(9)),
                (
                    "parent".into(),
                    Value::Reference(ObjRef::from_parts(7, 1, 1)),
                ),
            ]),
        );
        second.created_at = now;
        second.updated_at = now;
        (
            compiled,
            schema,
            vec![
                (ObjRef::from_parts(7, 1, 1), Arc::new(first)),
                (ObjRef::from_parts(7, 2, 1), Arc::new(second)),
            ],
        )
    }

    #[test]
    fn scalar_segment_round_trips_missing_null_utf8_stats_and_metadata() {
        let (compiled, schema, rows) = fixture();
        let segment = TypedSegment::build(&compiled, &schema, &rows).unwrap();
        let encoded = segment.encode().unwrap();
        let decoded = TypedSegment::decode(&encoded).unwrap();
        assert_eq!(decoded, segment);
        assert!(decoded.authoritative);
        assert_eq!(decoded.decode_document(0, &compiled).unwrap(), *rows[0].1);
        assert_eq!(decoded.decode_document(1, &compiled).unwrap(), *rows[1].1);
        assert_eq!(decoded.document_ids, ["a", "b"]);
        let optional_id = compiled
            .fields
            .iter()
            .find(|field| field.name == "optional")
            .unwrap()
            .id;
        let optional = decoded.column(optional_id).unwrap();
        assert!(bit(&optional.presence, 0));
        assert!(bit(&optional.nulls, 0));
        assert!(!bit(&optional.presence, 1));
        assert!(!bit(&optional.nulls, 1));
        assert_eq!(optional.cell(0).unwrap(), TypedCell::Null);
        assert_eq!(optional.cell(1).unwrap(), TypedCell::Missing);
        let sequence_id = compiled
            .fields
            .iter()
            .find(|field| field.name == "sequence")
            .unwrap()
            .id;
        let sequence = decoded.column(sequence_id).unwrap();
        assert_eq!(sequence.integer_min, Some(-3));
        assert_eq!(sequence.integer_max, Some(9));
        assert_eq!(sequence.cell(0).unwrap(), TypedCell::Integer(-3));
        assert_eq!(sequence.cell(1).unwrap(), TypedCell::Integer(9));
    }

    #[test]
    fn descriptor_discovers_row_group_without_decoding_payload() {
        let (compiled, schema, rows) = fixture();
        let encoded = TypedSegment::build(&compiled, &schema, &rows)
            .unwrap()
            .encode()
            .unwrap();
        let descriptor = TypedSegment::descriptor(&encoded[..HEADER_LEN]).unwrap();
        assert!(descriptor.authoritative);
        assert_eq!(descriptor.collection_id, compiled.collection_id);
        assert_eq!(descriptor.schema_version, compiled.version);
        assert_eq!(descriptor.row_count, rows.len());
        assert_eq!(descriptor.column_count, compiled.fields.len());
        assert_eq!(descriptor.encoded_len, encoded.len());
        assert!(descriptor.lookup_len < descriptor.identity_len);
        assert!(descriptor.identity_len < encoded.len() - HEADER_LEN);
        assert!(descriptor.lookup_checksum.is_some());

        let identity = TypedSegment::decode_identity(
            descriptor,
            Arc::from(&encoded[HEADER_LEN..HEADER_LEN + descriptor.lookup_len]),
        )
        .unwrap();
        assert_eq!(identity.object_ref(0).unwrap(), rows[0].0);
        assert_eq!(identity.object_ref(1).unwrap(), rows[1].0);
        assert_eq!(identity.document_id(0).unwrap(), "a");
        assert_eq!(identity.document_id(1).unwrap(), "b");

        let mut corrupt_identity = encoded[HEADER_LEN..HEADER_LEN + descriptor.lookup_len].to_vec();
        *corrupt_identity.last_mut().unwrap() ^= 0x01;
        assert!(TypedSegment::decode_identity(descriptor, Arc::from(corrupt_identity)).is_err());

        let mut corrupt_metadata = encoded.clone();
        corrupt_metadata[HEADER_LEN + descriptor.lookup_len] ^= 0x5a;
        TypedSegment::decode_identity(
            descriptor,
            Arc::from(&corrupt_metadata[HEADER_LEN..HEADER_LEN + descriptor.lookup_len]),
        )
        .unwrap();
        assert!(TypedSegment::decode(&corrupt_metadata).is_err());

        let mut corrupt_payload = encoded;
        *corrupt_payload.last_mut().unwrap() ^= 0x5a;
        assert_eq!(
            TypedSegment::descriptor(&corrupt_payload[..HEADER_LEN]).unwrap(),
            descriptor
        );
        assert!(TypedSegment::decode(&corrupt_payload).is_err());
    }

    #[test]
    fn segment_rejects_header_body_and_column_corruption() {
        let (compiled, schema, rows) = fixture();
        let encoded = TypedSegment::build(&compiled, &schema, &rows)
            .unwrap()
            .encode()
            .unwrap();
        for offset in [0, HEADER_LEN, encoded.len() - 1] {
            let mut corrupted = encoded.clone();
            corrupted[offset] ^= 0x5a;
            assert!(TypedSegment::decode(&corrupted).is_err(), "offset={offset}");
        }
        assert!(TypedSegment::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    #[test]
    fn segment_rejects_too_many_rows() {
        let (compiled, schema, fixture_rows) = fixture();
        let row = fixture_rows[0].clone();
        let rows = vec![row; MAX_ROWS + 1];
        assert!(TypedSegment::build(&compiled, &schema, &rows).is_err());
    }

    #[test]
    fn repetitive_columns_are_compressed_and_remain_authoritative() {
        let schema = Schema::new(
            "logs".into(),
            HashMap::from([
                ("message".into(), FieldType::String),
                ("level".into(), FieldType::Integer),
            ]),
        );
        let compiled = CompiledSchema::first(&schema, 9).unwrap();
        let rows = (0..2_048)
            .map(|index| {
                (
                    ObjRef::from_parts(9, index + 1, 1),
                    Arc::new(Document::with_id(
                        format!("log-{index:04}"),
                        "logs".into(),
                        HashMap::from([
                            (
                                "message".into(),
                                Value::String("repeated stack frame".repeat(8)),
                            ),
                            ("level".into(), Value::Integer(50)),
                        ]),
                    )),
                )
            })
            .collect::<Vec<_>>();
        let uncompressed_strings = rows
            .iter()
            .map(|(_, document)| match &document.fields["message"] {
                Value::String(value) => value.len(),
                _ => 0,
            })
            .sum::<usize>();
        let encoded = TypedSegment::build(&compiled, &schema, &rows)
            .unwrap()
            .encode()
            .unwrap();
        assert!(
            encoded.len() < uncompressed_strings / 2,
            "encoded={} raw_strings={uncompressed_strings}",
            encoded.len()
        );
        let decoded = TypedSegment::decode(&encoded).unwrap();
        assert!(decoded.authoritative);
        assert_eq!(
            decoded.decode_document(1_337, &compiled).unwrap(),
            *rows[1_337].1
        );
    }
}
