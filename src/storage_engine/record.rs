use crate::document::{Document, ObjRef, Value};
use crate::error::{OQLError, OQLResult};
use crate::schema::Schema;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const RECORD_MAGIC: [u8; 4] = *b"OWLR";
const RECORD_VERSION_V1: u16 = 1;
const RECORD_VERSION: u16 = 2;
const HEADER_LEN: usize = 80;
const SLOT_LEN: usize = 16;
const MAX_NESTING: usize = 64;
const MAX_FIELDS: usize = u16::MAX as usize;

const TAG_STRING: u8 = 1;
const TAG_INTEGER: u8 = 2;
const TAG_FLOAT: u8 = 3;
const TAG_BOOLEAN: u8 = 4;
const TAG_DATETIME: u8 = 5;
const TAG_REFERENCE: u8 = 6;
const TAG_GLOBAL_REFERENCE: u8 = 7;
const TAG_LIST: u8 = 8;
const TAG_OBJECT: u8 = 9;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompiledField {
    pub id: u16,
    pub name: String,
}

/// Stable field directory used by the record codec.
///
/// Field names live once in this catalog entry, not in each top-level record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompiledSchema {
    pub name: String,
    pub collection_id: u16,
    pub version: u32,
    pub fields: Vec<CompiledField>,
}

impl CompiledSchema {
    pub(crate) fn first(schema: &Schema, collection_id: u16) -> OQLResult<Self> {
        Self::from_names(
            schema.name.clone(),
            collection_id,
            1,
            schema.fields.keys(),
            None,
        )
    }

    pub(crate) fn infer(document: &Document, collection_id: u16) -> OQLResult<Self> {
        Self::from_names(
            document.schema.clone(),
            collection_id,
            1,
            document.fields.keys(),
            None,
        )
    }

    pub(crate) fn evolve<'a>(
        previous: &CompiledSchema,
        names: impl Iterator<Item = &'a String>,
    ) -> OQLResult<Self> {
        let requested = names.cloned().collect::<HashSet<_>>();
        let current = previous
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect::<HashSet<_>>();
        if requested == current {
            return Ok(previous.clone());
        }
        Self::from_names(
            previous.name.clone(),
            previous.collection_id,
            previous.version.checked_add(1).ok_or_else(|| {
                OQLError::storage_error("compiled schema version space exhausted", None)
            })?,
            requested.iter(),
            Some(previous),
        )
    }

    fn from_names<'a>(
        name: String,
        collection_id: u16,
        version: u32,
        names: impl Iterator<Item = &'a String>,
        previous: Option<&CompiledSchema>,
    ) -> OQLResult<Self> {
        let mut unique = names.cloned().collect::<HashSet<_>>();
        if unique.len() > MAX_FIELDS {
            return Err(OQLError::storage_error(
                "record schema contains too many fields",
                None,
            ));
        }
        let previous_ids = previous
            .map(|schema| {
                schema
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.id))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        let mut next_id =
            match previous.and_then(|schema| schema.fields.iter().map(|field| field.id).max()) {
                Some(id) => id.checked_add(1).ok_or_else(|| {
                    OQLError::storage_error("field identifier space exhausted", None)
                })?,
                None => 0,
            };
        let mut names = unique.drain().collect::<Vec<_>>();
        names.sort();
        let mut fields = Vec::with_capacity(names.len());
        for name in names {
            let id = match previous_ids.get(&name).copied() {
                Some(id) => id,
                None => {
                    let id = next_id;
                    next_id = next_id.checked_add(1).ok_or_else(|| {
                        OQLError::storage_error("field identifier space exhausted", None)
                    })?;
                    id
                }
            };
            fields.push(CompiledField { id, name });
        }
        fields.sort_by_key(|field| field.id);
        Ok(Self {
            name,
            collection_id,
            version,
            fields,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecordHeader {
    pub format_version: u16,
    pub schema_id: u32,
    pub schema_version: u32,
    pub object_ref: ObjRef,
    pub document_version: u64,
    pub field_count: u16,
    pub total_len: u32,
}

pub(crate) struct RecordCodec;

#[derive(Debug)]
pub(crate) enum BorrowedRecordValue<'a> {
    Missing,
    Null,
    String(&'a str),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Reference(ObjRef),
    Owned(Value),
}

pub(crate) struct RecordView<'a> {
    record: &'a [u8],
    schema: &'a CompiledSchema,
    regions: RecordRegions<'a>,
}

impl<'a> RecordView<'a> {
    pub(crate) fn field_at(&self, index: usize) -> OQLResult<BorrowedRecordValue<'a>> {
        let regions = &self.regions;
        if index >= self.schema.fields.len() {
            return Ok(BorrowedRecordValue::Missing);
        }
        if regions.header.format_version >= RECORD_VERSION
            && regions.presence[index / 8] & (1 << (index % 8)) == 0
        {
            return Ok(BorrowedRecordValue::Missing);
        }
        if regions.nulls[index / 8] & (1 << (index % 8)) != 0 {
            return Ok(BorrowedRecordValue::Null);
        }
        let slot = &regions.fixed[index * SLOT_LEN..(index + 1) * SLOT_LEN];
        match slot[0] {
            TAG_INTEGER => Ok(BorrowedRecordValue::Integer(get_i64(slot, 8)?)),
            TAG_FLOAT => Ok(BorrowedRecordValue::Float(f64::from_bits(get_u64(
                slot, 8,
            )?))),
            TAG_BOOLEAN => match get_u64(slot, 8)? {
                0 => Ok(BorrowedRecordValue::Boolean(false)),
                1 => Ok(BorrowedRecordValue::Boolean(true)),
                _ => Err(OQLError::storage_error("invalid encoded boolean", None)),
            },
            TAG_DATETIME => Ok(BorrowedRecordValue::DateTime(timestamp(get_i64(slot, 8)?)?)),
            TAG_REFERENCE => Ok(BorrowedRecordValue::Reference(ObjRef::new(get_u64(
                slot, 8,
            )?))),
            TAG_STRING => {
                let payload = dynamic_slot_payload(slot, regions.variable)?;
                let mut cursor = 0;
                if take(payload, &mut cursor, 1)?[0] != TAG_STRING {
                    return Err(OQLError::storage_error(
                        "record string slot tag disagrees with its payload",
                        None,
                    ));
                }
                let value = std::str::from_utf8(take_sized(payload, &mut cursor)?)
                    .map_err(|_| OQLError::storage_error("string is not valid UTF-8", None))?;
                if cursor != payload.len() {
                    return Err(OQLError::storage_error(
                        "record string slot has trailing bytes",
                        None,
                    ));
                }
                Ok(BorrowedRecordValue::String(value))
            }
            TAG_GLOBAL_REFERENCE | TAG_LIST | TAG_OBJECT => Ok(BorrowedRecordValue::Owned(
                decode_slot(slot, regions.variable)?,
            )),
            _ => Err(OQLError::storage_error("unknown record slot tag", None)),
        }
    }

    pub(crate) fn external_id(&self) -> OQLResult<&'a str> {
        std::str::from_utf8(&self.regions.variable[..self.regions.external_id_len])
            .map_err(|_| OQLError::storage_error("document ID is not valid UTF-8", None))
    }

    pub(crate) fn created_at(&self) -> OQLResult<DateTime<Utc>> {
        timestamp(get_i64(self.record, 32)?)
    }

    pub(crate) fn updated_at(&self) -> OQLResult<DateTime<Utc>> {
        timestamp(get_i64(self.record, 40)?)
    }

    pub(crate) fn version(&self) -> u64 {
        self.regions.header.document_version
    }

    pub(crate) fn decode_slots(&self, projection: &[(usize, String)]) -> OQLResult<Document> {
        let mut fields = HashMap::with_capacity(projection.len());
        for (index, name) in projection {
            if *index >= self.schema.fields.len()
                || (self.regions.header.format_version >= RECORD_VERSION
                    && self.regions.presence[*index / 8] & (1 << (*index % 8)) == 0)
            {
                continue;
            }
            let value = if self.regions.nulls[*index / 8] & (1 << (*index % 8)) != 0 {
                Value::Null
            } else {
                decode_slot(
                    &self.regions.fixed[*index * SLOT_LEN..(*index + 1) * SLOT_LEN],
                    self.regions.variable,
                )?
            };
            fields.insert(name.clone(), value);
        }
        Ok(Document {
            id: self.external_id()?.to_string(),
            schema: self.schema.name.clone(),
            fields,
            created_at: self.created_at()?,
            updated_at: self.updated_at()?,
            version: self.version(),
        })
    }
}

impl RecordCodec {
    /// View a record already authenticated while entering live state or
    /// checkpoint storage. Structural bounds and schema identity remain
    /// checked; only the redundant full-record checksum pass is omitted.
    pub(crate) fn trusted_view<'a>(
        record: &'a [u8],
        schema: &'a CompiledSchema,
    ) -> OQLResult<RecordView<'a>> {
        Ok(RecordView {
            record,
            schema,
            regions: record_regions_with_checksum(record, schema, false)?,
        })
    }

    pub(crate) fn trusted_header(record: &[u8]) -> OQLResult<RecordHeader> {
        parse_header(record, false)
    }

    pub(crate) fn encode(
        document: &Document,
        object_ref: ObjRef,
        schema: &CompiledSchema,
    ) -> OQLResult<Vec<u8>> {
        if document.schema != schema.name || object_ref.collection_id() != schema.collection_id {
            return Err(OQLError::storage_error(
                "document, object reference, and compiled schema disagree",
                None,
            ));
        }
        if schema.fields.len() > MAX_FIELDS {
            return Err(OQLError::storage_error("too many record fields", None));
        }

        let presence_bitmap_len = schema.fields.len().div_ceil(8);
        let null_bitmap_len = schema.fields.len().div_ceil(8);
        let fixed_len = schema
            .fields
            .len()
            .checked_mul(SLOT_LEN)
            .ok_or_else(|| OQLError::storage_error("record fixed region overflow", None))?;
        let mut presence_bitmap = vec![0u8; presence_bitmap_len];
        let mut null_bitmap = vec![0u8; null_bitmap_len];
        let mut slots = vec![0u8; fixed_len];
        let mut variable = document.id.as_bytes().to_vec();

        for (index, field) in schema.fields.iter().enumerate() {
            let Some(value) = document.fields.get(&field.name) else {
                continue;
            };
            presence_bitmap[index / 8] |= 1 << (index % 8);
            if matches!(value, Value::Null) {
                null_bitmap[index / 8] |= 1 << (index % 8);
                continue;
            }
            let slot = &mut slots[index * SLOT_LEN..(index + 1) * SLOT_LEN];
            encode_slot(value, slot, &mut variable)?;
        }

        let payload_len = presence_bitmap_len
            .checked_add(null_bitmap_len)
            .and_then(|len| len.checked_add(fixed_len))
            .and_then(|len| len.checked_add(variable.len()))
            .ok_or_else(|| OQLError::storage_error("record length overflow", None))?;
        let total_len = HEADER_LEN
            .checked_add(payload_len)
            .ok_or_else(|| OQLError::storage_error("record length overflow", None))?;
        let total_len_u32 = u32::try_from(total_len)
            .map_err(|_| OQLError::storage_error("record exceeds format limit", None))?;
        let variable_len_u32 = u32::try_from(variable.len())
            .map_err(|_| OQLError::storage_error("variable region exceeds format limit", None))?;
        let external_id_len = u32::try_from(document.id.len())
            .map_err(|_| OQLError::storage_error("document ID exceeds format limit", None))?;

        let mut record = vec![0u8; total_len];
        record[0..4].copy_from_slice(&RECORD_MAGIC);
        put_u16(&mut record, 4, RECORD_VERSION);
        put_u16(&mut record, 6, HEADER_LEN as u16);
        put_u32(&mut record, 8, schema.collection_id as u32);
        put_u32(&mut record, 12, schema.version);
        put_u16(&mut record, 16, object_ref.collection_id());
        put_u16(&mut record, 18, object_ref.generation());
        put_u32(&mut record, 20, object_ref.object_id());
        put_u64(&mut record, 24, document.version);
        put_i64(&mut record, 32, document.created_at.timestamp_micros());
        put_i64(&mut record, 40, document.updated_at.timestamp_micros());
        put_u16(&mut record, 48, schema.fields.len() as u16);
        put_u16(&mut record, 50, null_bitmap_len as u16);
        put_u32(&mut record, 52, fixed_len as u32);
        put_u32(&mut record, 56, variable_len_u32);
        put_u32(&mut record, 60, external_id_len);
        put_u32(&mut record, 68, presence_bitmap_len as u32);
        put_u32(&mut record, 72, total_len_u32);

        let mut cursor = HEADER_LEN;
        record[cursor..cursor + presence_bitmap_len].copy_from_slice(&presence_bitmap);
        cursor += presence_bitmap_len;
        record[cursor..cursor + null_bitmap_len].copy_from_slice(&null_bitmap);
        cursor += null_bitmap_len;
        record[cursor..cursor + fixed_len].copy_from_slice(&slots);
        cursor += fixed_len;
        record[cursor..].copy_from_slice(&variable);
        let checksum = record_checksum(&record);
        put_u32(&mut record, 64, checksum);
        Ok(record)
    }

    pub(crate) fn header(record: &[u8]) -> OQLResult<RecordHeader> {
        validate_header(record)
    }

    pub(crate) fn decode(record: &[u8], schema: &CompiledSchema) -> OQLResult<Document> {
        Self::decode_projection(record, schema, None)
    }

    pub(crate) fn decode_projection(
        record: &[u8],
        schema: &CompiledSchema,
        projection: Option<&HashSet<String>>,
    ) -> OQLResult<Document> {
        let header = validate_header(record)?;
        if header.schema_id != schema.collection_id as u32
            || header.schema_version != schema.version
            || header.field_count as usize != schema.fields.len()
        {
            return Err(OQLError::storage_error(
                "compiled schema does not match record header",
                None,
            ));
        }
        let null_len = get_u16(record, 50)? as usize;
        let presence_len = if header.format_version >= RECORD_VERSION {
            get_u32(record, 68)? as usize
        } else {
            0
        };
        let fixed_len = get_u32(record, 52)? as usize;
        let variable_len = get_u32(record, 56)? as usize;
        let external_id_len = get_u32(record, 60)? as usize;
        if null_len != schema.fields.len().div_ceil(8)
            || (header.format_version >= RECORD_VERSION
                && presence_len != schema.fields.len().div_ceil(8))
            || fixed_len != schema.fields.len() * SLOT_LEN
        {
            return Err(OQLError::storage_error(
                "invalid record region lengths",
                None,
            ));
        }
        let presence_start = HEADER_LEN;
        let null_start = checked_end(presence_start, presence_len, record.len())?;
        let fixed_start = checked_end(null_start, null_len, record.len())?;
        let variable_start = checked_end(fixed_start, fixed_len, record.len())?;
        let variable_end = checked_end(variable_start, variable_len, record.len())?;
        if variable_end != record.len() || external_id_len > variable_len {
            return Err(OQLError::storage_error(
                "invalid record variable region",
                None,
            ));
        }
        let external_id =
            std::str::from_utf8(&record[variable_start..variable_start + external_id_len])
                .map_err(|_| OQLError::storage_error("document ID is not valid UTF-8", None))?
                .to_string();
        let presence = &record[presence_start..null_start];
        let nulls = &record[null_start..fixed_start];
        let fixed = &record[fixed_start..variable_start];
        let variable = &record[variable_start..variable_end];
        let mut fields = HashMap::new();

        for (index, field) in schema.fields.iter().enumerate() {
            if projection.is_some_and(|projection| !projection.contains(&field.name)) {
                continue;
            }
            if header.format_version >= RECORD_VERSION
                && presence[index / 8] & (1 << (index % 8)) == 0
            {
                continue;
            }
            let value = if nulls[index / 8] & (1 << (index % 8)) != 0 {
                Value::Null
            } else {
                decode_slot(&fixed[index * SLOT_LEN..(index + 1) * SLOT_LEN], variable)?
            };
            fields.insert(field.name.clone(), value);
        }

        let created_at = timestamp(get_i64(record, 32)?)?;
        let updated_at = timestamp(get_i64(record, 40)?)?;
        Ok(Document {
            id: external_id,
            schema: schema.name.clone(),
            fields,
            created_at,
            updated_at,
            version: header.document_version,
        })
    }
}

struct RecordRegions<'a> {
    header: RecordHeader,
    presence: &'a [u8],
    nulls: &'a [u8],
    fixed: &'a [u8],
    variable: &'a [u8],
    external_id_len: usize,
}

fn record_regions_with_checksum<'a>(
    record: &'a [u8],
    schema: &CompiledSchema,
    checksum: bool,
) -> OQLResult<RecordRegions<'a>> {
    let header = parse_header(record, checksum)?;
    if header.schema_id != schema.collection_id as u32
        || header.schema_version != schema.version
        || header.field_count as usize != schema.fields.len()
    {
        return Err(OQLError::storage_error(
            "compiled schema does not match record header",
            None,
        ));
    }
    let null_len = get_u16(record, 50)? as usize;
    let presence_len = if header.format_version >= RECORD_VERSION {
        get_u32(record, 68)? as usize
    } else {
        0
    };
    let fixed_len = get_u32(record, 52)? as usize;
    let variable_len = get_u32(record, 56)? as usize;
    let external_id_len = get_u32(record, 60)? as usize;
    if null_len != schema.fields.len().div_ceil(8)
        || (header.format_version >= RECORD_VERSION
            && presence_len != schema.fields.len().div_ceil(8))
        || fixed_len != schema.fields.len() * SLOT_LEN
    {
        return Err(OQLError::storage_error(
            "invalid record region lengths",
            None,
        ));
    }
    let presence_start = HEADER_LEN;
    let null_start = checked_end(presence_start, presence_len, record.len())?;
    let fixed_start = checked_end(null_start, null_len, record.len())?;
    let variable_start = checked_end(fixed_start, fixed_len, record.len())?;
    let variable_end = checked_end(variable_start, variable_len, record.len())?;
    if variable_end != record.len() || external_id_len > variable_len {
        return Err(OQLError::storage_error(
            "invalid record variable region",
            None,
        ));
    }
    Ok(RecordRegions {
        header,
        presence: &record[presence_start..null_start],
        nulls: &record[null_start..fixed_start],
        fixed: &record[fixed_start..variable_start],
        variable: &record[variable_start..variable_end],
        external_id_len,
    })
}

fn dynamic_slot_payload<'a>(slot: &[u8], variable: &'a [u8]) -> OQLResult<&'a [u8]> {
    let len = get_u32(slot, 4)? as usize;
    let offset = usize::try_from(get_u64(slot, 8)?)
        .map_err(|_| OQLError::storage_error("variable offset overflow", None))?;
    let end = checked_end(offset, len, variable.len())?;
    Ok(&variable[offset..end])
}

fn encode_slot(value: &Value, slot: &mut [u8], variable: &mut Vec<u8>) -> OQLResult<()> {
    match value {
        Value::String(_) | Value::GlobalReference(_) | Value::List(_) | Value::Object(_) => {
            let tag = value_tag(value)?;
            let offset = variable.len();
            encode_dynamic(value, variable, 0)?;
            let len = u32::try_from(variable.len() - offset).map_err(|_| {
                OQLError::storage_error("variable value exceeds format limit", None)
            })?;
            slot[0] = tag;
            put_u32(slot, 4, len);
            put_u64(slot, 8, offset as u64);
        }
        Value::Integer(value) => {
            slot[0] = TAG_INTEGER;
            put_i64(slot, 8, *value);
        }
        Value::Float(value) => {
            slot[0] = TAG_FLOAT;
            put_u64(slot, 8, value.to_bits());
        }
        Value::Boolean(value) => {
            slot[0] = TAG_BOOLEAN;
            put_u64(slot, 8, u64::from(*value));
        }
        Value::DateTime(value) => {
            slot[0] = TAG_DATETIME;
            put_i64(slot, 8, value.timestamp_micros());
        }
        Value::Reference(value) => {
            slot[0] = TAG_REFERENCE;
            put_u64(slot, 8, value.packed());
        }
        Value::Null => unreachable!("nulls are represented by the null bitmap"),
    }
    Ok(())
}

fn decode_slot(slot: &[u8], variable: &[u8]) -> OQLResult<Value> {
    match slot[0] {
        TAG_INTEGER => Ok(Value::Integer(get_i64(slot, 8)?)),
        TAG_FLOAT => Ok(Value::Float(f64::from_bits(get_u64(slot, 8)?))),
        TAG_BOOLEAN => match get_u64(slot, 8)? {
            0 => Ok(Value::Boolean(false)),
            1 => Ok(Value::Boolean(true)),
            _ => Err(OQLError::storage_error("invalid encoded boolean", None)),
        },
        TAG_DATETIME => Ok(Value::DateTime(timestamp(get_i64(slot, 8)?)?)),
        TAG_REFERENCE => Ok(Value::Reference(ObjRef::new(get_u64(slot, 8)?))),
        TAG_STRING | TAG_GLOBAL_REFERENCE | TAG_LIST | TAG_OBJECT => {
            let len = get_u32(slot, 4)? as usize;
            let offset = usize::try_from(get_u64(slot, 8)?)
                .map_err(|_| OQLError::storage_error("variable offset overflow", None))?;
            let end = checked_end(offset, len, variable.len())?;
            let mut cursor = 0;
            let value = decode_dynamic(&variable[offset..end], &mut cursor, 0)?;
            if cursor != len || value_tag(&value)? != slot[0] {
                return Err(OQLError::storage_error(
                    "invalid variable slot payload",
                    None,
                ));
            }
            Ok(value)
        }
        _ => Err(OQLError::storage_error("unknown record slot tag", None)),
    }
}

fn encode_dynamic(value: &Value, output: &mut Vec<u8>, depth: usize) -> OQLResult<()> {
    if depth > MAX_NESTING {
        return Err(OQLError::storage_error(
            "value nesting limit exceeded",
            None,
        ));
    }
    output.push(value_tag(value)?);
    match value {
        Value::Null => {}
        Value::String(value) => put_bytes(output, value.as_bytes())?,
        Value::Integer(value) => output.extend_from_slice(&value.to_le_bytes()),
        Value::Float(value) => output.extend_from_slice(&value.to_bits().to_le_bytes()),
        Value::Boolean(value) => output.push(u8::from(*value)),
        Value::DateTime(value) => output.extend_from_slice(&value.timestamp_micros().to_le_bytes()),
        Value::Reference(value) => output.extend_from_slice(&value.packed().to_le_bytes()),
        Value::GlobalReference(value) => output.extend_from_slice(&value.to_bytes()),
        Value::List(values) => {
            put_count(output, values.len())?;
            for value in values {
                encode_dynamic(value, output, depth + 1)?;
            }
        }
        Value::Object(fields) => {
            put_count(output, fields.len())?;
            let mut names = fields.keys().collect::<Vec<_>>();
            names.sort();
            for name in names {
                put_bytes(output, name.as_bytes())?;
                encode_dynamic(&fields[name], output, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn decode_dynamic(input: &[u8], cursor: &mut usize, depth: usize) -> OQLResult<Value> {
    if depth > MAX_NESTING {
        return Err(OQLError::storage_error(
            "value nesting limit exceeded",
            None,
        ));
    }
    let tag = take(input, cursor, 1)?[0];
    match tag {
        0 => Ok(Value::Null),
        TAG_STRING => {
            let bytes = take_sized(input, cursor)?;
            let value = std::str::from_utf8(bytes)
                .map_err(|_| OQLError::storage_error("string is not valid UTF-8", None))?;
            Ok(Value::String(value.to_string()))
        }
        TAG_INTEGER => Ok(Value::Integer(i64::from_le_bytes(
            take(input, cursor, 8)?.try_into().unwrap(),
        ))),
        TAG_FLOAT => Ok(Value::Float(f64::from_bits(u64::from_le_bytes(
            take(input, cursor, 8)?.try_into().unwrap(),
        )))),
        TAG_BOOLEAN => match take(input, cursor, 1)?[0] {
            0 => Ok(Value::Boolean(false)),
            1 => Ok(Value::Boolean(true)),
            _ => Err(OQLError::storage_error("invalid encoded boolean", None)),
        },
        TAG_DATETIME => Ok(Value::DateTime(timestamp(i64::from_le_bytes(
            take(input, cursor, 8)?.try_into().unwrap(),
        ))?)),
        TAG_REFERENCE => Ok(Value::Reference(ObjRef::new(u64::from_le_bytes(
            take(input, cursor, 8)?.try_into().unwrap(),
        )))),
        TAG_GLOBAL_REFERENCE => Ok(Value::GlobalReference(
            crate::distributed::GlobalObjRef::from_bytes(take(input, cursor, 16)?)
                .map_err(|message| OQLError::storage_error(message, None))?,
        )),
        TAG_LIST => {
            let count = take_count(input, cursor)?;
            let mut values = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                values.push(decode_dynamic(input, cursor, depth + 1)?);
            }
            Ok(Value::List(values))
        }
        TAG_OBJECT => {
            let count = take_count(input, cursor)?;
            let mut fields = HashMap::with_capacity(count.min(1024));
            for _ in 0..count {
                let name = std::str::from_utf8(take_sized(input, cursor)?)
                    .map_err(|_| OQLError::storage_error("field name is not UTF-8", None))?
                    .to_string();
                if fields
                    .insert(name, decode_dynamic(input, cursor, depth + 1)?)
                    .is_some()
                {
                    return Err(OQLError::storage_error("duplicate object field", None));
                }
            }
            Ok(Value::Object(fields))
        }
        _ => Err(OQLError::storage_error("unknown dynamic value tag", None)),
    }
}

fn value_tag(value: &Value) -> OQLResult<u8> {
    Ok(match value {
        Value::Null => 0,
        Value::String(_) => TAG_STRING,
        Value::Integer(_) => TAG_INTEGER,
        Value::Float(_) => TAG_FLOAT,
        Value::Boolean(_) => TAG_BOOLEAN,
        Value::DateTime(_) => TAG_DATETIME,
        Value::Reference(_) => TAG_REFERENCE,
        Value::GlobalReference(_) => TAG_GLOBAL_REFERENCE,
        Value::List(_) => TAG_LIST,
        Value::Object(_) => TAG_OBJECT,
    })
}

fn validate_header(record: &[u8]) -> OQLResult<RecordHeader> {
    parse_header(record, true)
}

fn parse_header(record: &[u8], checksum: bool) -> OQLResult<RecordHeader> {
    if record.len() < HEADER_LEN || record[0..4] != RECORD_MAGIC {
        return Err(OQLError::storage_error("invalid record header", None));
    }
    let format_version = get_u16(record, 4)?;
    if !(RECORD_VERSION_V1..=RECORD_VERSION).contains(&format_version)
        || get_u16(record, 6)? as usize != HEADER_LEN
    {
        return Err(OQLError::storage_error("unsupported record format", None));
    }
    let total_len = get_u32(record, 72)?;
    if total_len as usize != record.len() {
        return Err(OQLError::storage_error(
            "record length does not match header",
            None,
        ));
    }
    if checksum && record_checksum(record) != get_u32(record, 64)? {
        return Err(OQLError::storage_error("record checksum mismatch", None));
    }
    let object_ref = ObjRef::from_parts(
        get_u16(record, 16)?,
        get_u32(record, 20)?,
        get_u16(record, 18)?,
    );
    Ok(RecordHeader {
        format_version,
        schema_id: get_u32(record, 8)?,
        schema_version: get_u32(record, 12)?,
        object_ref,
        document_version: get_u64(record, 24)?,
        field_count: get_u16(record, 48)?,
        total_len,
    })
}

fn record_checksum(record: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&record[..64.min(record.len())]);
    if record.len() > 64 {
        hasher.update(&[0; 4]);
        if record.len() > 68 {
            hasher.update(&record[68..]);
        }
    }
    hasher.finalize()
}

fn timestamp(micros: i64) -> OQLResult<DateTime<Utc>> {
    DateTime::from_timestamp_micros(micros)
        .ok_or_else(|| OQLError::storage_error("timestamp is outside supported range", None))
}

fn put_count(output: &mut Vec<u8>, count: usize) -> OQLResult<()> {
    let count = u32::try_from(count)
        .map_err(|_| OQLError::storage_error("value collection exceeds format limit", None))?;
    output.extend_from_slice(&count.to_le_bytes());
    Ok(())
}

fn put_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> OQLResult<()> {
    put_count(output, bytes.len())?;
    output.extend_from_slice(bytes);
    Ok(())
}

fn take_count(input: &[u8], cursor: &mut usize) -> OQLResult<usize> {
    Ok(u32::from_le_bytes(take(input, cursor, 4)?.try_into().unwrap()) as usize)
}

fn take_sized<'a>(input: &'a [u8], cursor: &mut usize) -> OQLResult<&'a [u8]> {
    let len = take_count(input, cursor)?;
    take(input, cursor, len)
}

fn take<'a>(input: &'a [u8], cursor: &mut usize, len: usize) -> OQLResult<&'a [u8]> {
    let end = checked_end(*cursor, len, input.len())?;
    let value = &input[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn checked_end(start: usize, len: usize, limit: usize) -> OQLResult<usize> {
    let end = start
        .checked_add(len)
        .ok_or_else(|| OQLError::storage_error("encoded length overflow", None))?;
    if end > limit {
        return Err(OQLError::storage_error("encoded value is truncated", None));
    }
    Ok(end)
}

fn put_u16(output: &mut [u8], offset: usize, value: u16) {
    output[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(output: &mut [u8], offset: usize, value: u32) {
    output[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(output: &mut [u8], offset: usize, value: u64) {
    output[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn put_i64(output: &mut [u8], offset: usize, value: i64) {
    output[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn get_u16(input: &[u8], offset: usize) -> OQLResult<u16> {
    Ok(u16::from_le_bytes(
        input
            .get(offset..offset + 2)
            .ok_or_else(|| OQLError::storage_error("truncated integer", None))?
            .try_into()
            .unwrap(),
    ))
}
fn get_u32(input: &[u8], offset: usize) -> OQLResult<u32> {
    Ok(u32::from_le_bytes(
        input
            .get(offset..offset + 4)
            .ok_or_else(|| OQLError::storage_error("truncated integer", None))?
            .try_into()
            .unwrap(),
    ))
}
fn get_u64(input: &[u8], offset: usize) -> OQLResult<u64> {
    Ok(u64::from_le_bytes(
        input
            .get(offset..offset + 8)
            .ok_or_else(|| OQLError::storage_error("truncated integer", None))?
            .try_into()
            .unwrap(),
    ))
}
fn get_i64(input: &[u8], offset: usize) -> OQLResult<i64> {
    Ok(i64::from_le_bytes(
        input
            .get(offset..offset + 8)
            .ok_or_else(|| OQLError::storage_error("truncated integer", None))?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distributed::GlobalObjRef;
    use crate::schema::FieldType;

    fn schema() -> Schema {
        Schema::new(
            "events".to_string(),
            HashMap::from([
                ("message".to_string(), FieldType::String),
                ("count".to_string(), FieldType::Integer),
                ("ratio".to_string(), FieldType::Float),
                ("active".to_string(), FieldType::Boolean),
                ("when".to_string(), FieldType::DateTime),
                ("target".to_string(), FieldType::Object),
                ("global".to_string(), FieldType::Object),
                ("items".to_string(), FieldType::Object),
                ("attrs".to_string(), FieldType::Object),
                (
                    "missing".to_string(),
                    FieldType::Optional(Box::new(FieldType::String)),
                ),
            ]),
        )
    }

    fn document() -> Document {
        let now = Utc::now();
        Document {
            id: "event-1".to_string(),
            schema: "events".to_string(),
            fields: HashMap::from([
                ("message".to_string(), Value::String("boom".to_string())),
                ("count".to_string(), Value::Integer(-42)),
                ("ratio".to_string(), Value::Float(1.25)),
                ("active".to_string(), Value::Boolean(true)),
                ("when".to_string(), Value::DateTime(now)),
                (
                    "target".to_string(),
                    Value::Reference(ObjRef::from_parts(9, 8, 1)),
                ),
                (
                    "global".to_string(),
                    Value::GlobalReference(GlobalObjRef::new(3, 99, 7)),
                ),
                (
                    "items".to_string(),
                    Value::List(vec![Value::Integer(1), Value::String("two".to_string())]),
                ),
                (
                    "attrs".to_string(),
                    Value::Object(HashMap::from([(
                        "region".to_string(),
                        Value::String("ng".to_string()),
                    )])),
                ),
                ("missing".to_string(), Value::Null),
            ]),
            created_at: now,
            updated_at: now,
            version: 4,
        }
    }

    #[test]
    fn all_value_types_round_trip_without_top_level_field_names() {
        let compiled = CompiledSchema::first(&schema(), 1).unwrap();
        let document = document();
        let object_ref = ObjRef::from_parts(1, 22, 1);
        let encoded = RecordCodec::encode(&document, object_ref, &compiled).unwrap();
        let decoded = RecordCodec::decode(&encoded, &compiled).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(
            RecordCodec::header(&encoded).unwrap().object_ref,
            object_ref
        );
        assert_eq!(
            encoded
                .windows("message".len())
                .filter(|window| *window == b"message")
                .count(),
            0,
            "top-level field names belong to the compiled schema, not each record"
        );
    }

    #[test]
    fn projection_decodes_only_requested_fields() {
        let compiled = CompiledSchema::first(&schema(), 1).unwrap();
        let encoded =
            RecordCodec::encode(&document(), ObjRef::from_parts(1, 22, 1), &compiled).unwrap();
        let projection = HashSet::from(["message".to_string(), "count".to_string()]);
        let decoded =
            RecordCodec::decode_projection(&encoded, &compiled, Some(&projection)).unwrap();
        assert_eq!(decoded.fields.len(), 2);
        assert_eq!(decoded.fields["message"], Value::String("boom".to_string()));
        assert_eq!(decoded.fields["count"], Value::Integer(-42));
    }

    #[test]
    fn trusted_view_borrows_scalar_slots_and_decodes_numeric_projection() {
        let compiled = CompiledSchema::first(&schema(), 1).unwrap();
        let encoded =
            RecordCodec::encode(&document(), ObjRef::from_parts(1, 22, 1), &compiled).unwrap();
        // Authentication happens at the storage ingress boundary.
        RecordCodec::header(&encoded).unwrap();
        let view = RecordCodec::trusted_view(&encoded, &compiled).unwrap();
        let message = compiled
            .fields
            .iter()
            .position(|field| field.name == "message")
            .unwrap();
        let count = compiled
            .fields
            .iter()
            .position(|field| field.name == "count")
            .unwrap();
        assert!(matches!(
            view.field_at(message).unwrap(),
            BorrowedRecordValue::String("boom")
        ));
        assert!(matches!(
            view.field_at(count).unwrap(),
            BorrowedRecordValue::Integer(-42)
        ));
        let projected = view.decode_slots(&[(count, "count".to_string())]).unwrap();
        assert_eq!(
            projected.fields,
            HashMap::from([("count".into(), Value::Integer(-42))])
        );
    }

    #[test]
    fn corruption_is_distinct_from_missing_data() {
        let compiled = CompiledSchema::first(&schema(), 1).unwrap();
        let mut encoded =
            RecordCodec::encode(&document(), ObjRef::from_parts(1, 22, 1), &compiled).unwrap();
        *encoded.last_mut().unwrap() ^= 0xff;
        let error = RecordCodec::decode(&encoded, &compiled).unwrap_err();
        assert!(error.to_string().contains("checksum"));
    }

    #[test]
    fn identity_header_corruption_is_detected() {
        let compiled = CompiledSchema::first(&schema(), 1).unwrap();
        let mut encoded =
            RecordCodec::encode(&document(), ObjRef::from_parts(1, 22, 1), &compiled).unwrap();
        encoded[20] ^= 0x01;
        let error = RecordCodec::decode(&encoded, &compiled).unwrap_err();
        assert!(error.to_string().contains("checksum"));
    }

    #[test]
    fn field_ids_survive_schema_evolution() {
        let first = CompiledSchema::first(&schema(), 1).unwrap();
        let names = HashMap::from([
            ("message".to_string(), Value::String(String::new())),
            ("new_field".to_string(), Value::Integer(1)),
        ]);
        let evolved = CompiledSchema::evolve(&first, names.keys()).unwrap();
        let old_id = first
            .fields
            .iter()
            .find(|field| field.name == "message")
            .unwrap()
            .id;
        let new_id = evolved
            .fields
            .iter()
            .find(|field| field.name == "message")
            .unwrap()
            .id;
        assert_eq!(old_id, new_id);
        assert_eq!(evolved.version, 2);
    }
}
