use crate::distributed::GlobalObjRef;
use crate::document::Value;
use crate::error::{OQLError, OQLResult};
use crate::storage_engine::ObjRef;
use chrono::{TimeZone, Utc};

const KEY_VERSION: u8 = 1;
const NULL: u8 = 0x10;
const BOOLEAN: u8 = 0x20;
const INTEGER: u8 = 0x30;
const FLOAT: u8 = 0x40;
const DATETIME: u8 = 0x50;
const STRING: u8 = 0x60;
const REFERENCE: u8 = 0x70;
const GLOBAL_REFERENCE: u8 = 0x71;

/// Versioned byte representation whose lexicographic order matches Owl's
/// scalar value order within each type. Composite keys are encoded by
/// concatenating self-delimiting components.
pub struct TypedKey;

impl TypedKey {
    pub fn encode(values: &[Value]) -> OQLResult<Vec<u8>> {
        let mut output = Vec::with_capacity(1 + values.len() * 9);
        output.push(KEY_VERSION);
        for value in values {
            encode_component(value, &mut output)?;
        }
        Ok(output)
    }

    pub fn encode_one(value: &Value) -> OQLResult<Vec<u8>> {
        Self::encode(std::slice::from_ref(value))
    }

    pub fn decode(key: &[u8]) -> OQLResult<Vec<Value>> {
        if key.first().copied() != Some(KEY_VERSION) {
            return Err(OQLError::storage_error(
                "ordered index key has an unsupported version",
                None,
            ));
        }
        let mut cursor = 1;
        let mut values = Vec::new();
        while cursor < key.len() {
            let tag = key[cursor];
            cursor += 1;
            let value = match tag {
                NULL => Value::Null,
                BOOLEAN => Value::Boolean(read_bytes::<1>(key, &mut cursor)?[0] != 0),
                INTEGER => {
                    let ordered = u64::from_be_bytes(read_bytes::<8>(key, &mut cursor)?);
                    Value::Integer((ordered ^ (1u64 << 63)) as i64)
                }
                FLOAT => {
                    let ordered = u64::from_be_bytes(read_bytes::<8>(key, &mut cursor)?);
                    let bits = if ordered & (1u64 << 63) != 0 {
                        ordered ^ (1u64 << 63)
                    } else {
                        !ordered
                    };
                    Value::Float(f64::from_bits(bits))
                }
                DATETIME => {
                    let seconds =
                        u64::from_be_bytes(read_bytes::<8>(key, &mut cursor)?) ^ (1u64 << 63);
                    let nanos = u32::from_be_bytes(read_bytes::<4>(key, &mut cursor)?);
                    Value::DateTime(
                        Utc.timestamp_opt(seconds as i64, nanos)
                            .single()
                            .ok_or_else(|| {
                                OQLError::storage_error(
                                    "ordered index key contains an invalid timestamp",
                                    None,
                                )
                            })?,
                    )
                }
                STRING => {
                    let mut bytes = Vec::new();
                    loop {
                        let byte = *key.get(cursor).ok_or_else(|| {
                            OQLError::storage_error("ordered index string key is truncated", None)
                        })?;
                        cursor += 1;
                        if byte != 0 {
                            bytes.push(byte);
                            continue;
                        }
                        let escape = *key.get(cursor).ok_or_else(|| {
                            OQLError::storage_error(
                                "ordered index string terminator is truncated",
                                None,
                            )
                        })?;
                        cursor += 1;
                        match escape {
                            0 => break,
                            0xff => bytes.push(0),
                            _ => {
                                return Err(OQLError::storage_error(
                                    "ordered index string contains an invalid escape",
                                    None,
                                ))
                            }
                        }
                    }
                    Value::String(String::from_utf8(bytes).map_err(|_| {
                        OQLError::storage_error("ordered index string is not UTF-8", None)
                    })?)
                }
                REFERENCE => Value::Reference(ObjRef::new(u64::from_be_bytes(read_bytes::<8>(
                    key,
                    &mut cursor,
                )?))),
                GLOBAL_REFERENCE => Value::GlobalReference(GlobalObjRef {
                    shard_id: u32::from_be_bytes(read_bytes::<4>(key, &mut cursor)?),
                    local_offset: u64::from_be_bytes(read_bytes::<8>(key, &mut cursor)?),
                    flags: u32::from_be_bytes(read_bytes::<4>(key, &mut cursor)?),
                }),
                _ => {
                    return Err(OQLError::storage_error(
                        "ordered index key contains an unknown component tag",
                        None,
                    ))
                }
            };
            values.push(value);
        }
        Ok(values)
    }

    /// Decode one component without materializing the components before it.
    ///
    /// Ordered scans use this for residual predicates on covering indexes. A
    /// query such as `(application, environment, occurred_at, level_rank)`
    /// can therefore inspect `level_rank` without allocating the leading
    /// strings or constructing a timestamp for every candidate key.
    pub fn decode_component(key: &[u8], target: usize) -> OQLResult<Value> {
        if key.first().copied() != Some(KEY_VERSION) {
            return Err(OQLError::storage_error(
                "ordered index key has an unsupported version",
                None,
            ));
        }
        let mut cursor = 1;
        let mut position = 0;
        while cursor < key.len() {
            if position == target {
                return decode_component(key, &mut cursor);
            }
            skip_component(key, &mut cursor)?;
            position += 1;
        }
        Err(OQLError::storage_error(
            "ordered index key has fewer components than its definition",
            None,
        ))
    }

    pub fn prefix_successor(mut key: Vec<u8>) -> Option<Vec<u8>> {
        for index in (0..key.len()).rev() {
            if key[index] != u8::MAX {
                key[index] += 1;
                key.truncate(index + 1);
                return Some(key);
            }
        }
        None
    }
}

fn read_bytes<const N: usize>(input: &[u8], cursor: &mut usize) -> OQLResult<[u8; N]> {
    let end = cursor.saturating_add(N);
    let bytes = input
        .get(*cursor..end)
        .ok_or_else(|| OQLError::storage_error("ordered index key component is truncated", None))?;
    *cursor = end;
    Ok(bytes.try_into().unwrap())
}

fn decode_component(input: &[u8], cursor: &mut usize) -> OQLResult<Value> {
    let tag = *input
        .get(*cursor)
        .ok_or_else(|| OQLError::storage_error("ordered index key component is truncated", None))?;
    *cursor += 1;
    match tag {
        NULL => Ok(Value::Null),
        BOOLEAN => Ok(Value::Boolean(read_bytes::<1>(input, cursor)?[0] != 0)),
        INTEGER => {
            let ordered = u64::from_be_bytes(read_bytes::<8>(input, cursor)?);
            Ok(Value::Integer((ordered ^ (1u64 << 63)) as i64))
        }
        FLOAT => {
            let ordered = u64::from_be_bytes(read_bytes::<8>(input, cursor)?);
            let bits = if ordered & (1u64 << 63) != 0 {
                ordered ^ (1u64 << 63)
            } else {
                !ordered
            };
            Ok(Value::Float(f64::from_bits(bits)))
        }
        DATETIME => {
            let seconds = u64::from_be_bytes(read_bytes::<8>(input, cursor)?) ^ (1u64 << 63);
            let nanos = u32::from_be_bytes(read_bytes::<4>(input, cursor)?);
            Ok(Value::DateTime(
                Utc.timestamp_opt(seconds as i64, nanos)
                    .single()
                    .ok_or_else(|| {
                        OQLError::storage_error(
                            "ordered index key contains an invalid timestamp",
                            None,
                        )
                    })?,
            ))
        }
        STRING => {
            let mut bytes = Vec::new();
            loop {
                let byte = *input.get(*cursor).ok_or_else(|| {
                    OQLError::storage_error("ordered index string key is truncated", None)
                })?;
                *cursor += 1;
                if byte != 0 {
                    bytes.push(byte);
                    continue;
                }
                let escape = *input.get(*cursor).ok_or_else(|| {
                    OQLError::storage_error("ordered index string terminator is truncated", None)
                })?;
                *cursor += 1;
                match escape {
                    0 => break,
                    0xff => bytes.push(0),
                    _ => {
                        return Err(OQLError::storage_error(
                            "ordered index string contains an invalid escape",
                            None,
                        ))
                    }
                }
            }
            Ok(Value::String(String::from_utf8(bytes).map_err(|_| {
                OQLError::storage_error("ordered index string is not UTF-8", None)
            })?))
        }
        REFERENCE => Ok(Value::Reference(ObjRef::new(u64::from_be_bytes(
            read_bytes::<8>(input, cursor)?,
        )))),
        GLOBAL_REFERENCE => Ok(Value::GlobalReference(GlobalObjRef {
            shard_id: u32::from_be_bytes(read_bytes::<4>(input, cursor)?),
            local_offset: u64::from_be_bytes(read_bytes::<8>(input, cursor)?),
            flags: u32::from_be_bytes(read_bytes::<4>(input, cursor)?),
        })),
        _ => Err(OQLError::storage_error(
            "ordered index key contains an unknown component tag",
            None,
        )),
    }
}

fn skip_component(input: &[u8], cursor: &mut usize) -> OQLResult<()> {
    let tag = *input
        .get(*cursor)
        .ok_or_else(|| OQLError::storage_error("ordered index key component is truncated", None))?;
    *cursor += 1;
    match tag {
        NULL => {}
        BOOLEAN => *cursor = cursor.saturating_add(1),
        INTEGER | FLOAT | REFERENCE => *cursor = cursor.saturating_add(8),
        DATETIME => *cursor = cursor.saturating_add(12),
        GLOBAL_REFERENCE => *cursor = cursor.saturating_add(16),
        STRING => loop {
            let byte = *input.get(*cursor).ok_or_else(|| {
                OQLError::storage_error("ordered index string key is truncated", None)
            })?;
            *cursor += 1;
            if byte != 0 {
                continue;
            }
            let escape = *input.get(*cursor).ok_or_else(|| {
                OQLError::storage_error("ordered index string terminator is truncated", None)
            })?;
            *cursor += 1;
            match escape {
                0 => break,
                0xff => {}
                _ => {
                    return Err(OQLError::storage_error(
                        "ordered index string contains an invalid escape",
                        None,
                    ))
                }
            }
        },
        _ => {
            return Err(OQLError::storage_error(
                "ordered index key contains an unknown component tag",
                None,
            ))
        }
    }
    if *cursor > input.len() {
        return Err(OQLError::storage_error(
            "ordered index key component is truncated",
            None,
        ));
    }
    Ok(())
}

fn encode_component(value: &Value, output: &mut Vec<u8>) -> OQLResult<()> {
    match value {
        Value::Null => output.push(NULL),
        Value::Boolean(value) => {
            output.push(BOOLEAN);
            output.push(u8::from(*value));
        }
        Value::Integer(value) => {
            output.push(INTEGER);
            output.extend_from_slice(&((*value as u64) ^ (1u64 << 63)).to_be_bytes());
        }
        Value::Float(value) => {
            if value.is_nan() {
                return Err(OQLError::storage_error(
                    "NaN cannot be used in an ordered index",
                    None,
                ));
            }
            output.push(FLOAT);
            let normalized = if *value == 0.0 { 0.0 } else { *value };
            let bits = normalized.to_bits();
            let ordered = if bits & (1u64 << 63) != 0 {
                !bits
            } else {
                bits ^ (1u64 << 63)
            };
            output.extend_from_slice(&ordered.to_be_bytes());
        }
        Value::DateTime(value) => {
            output.push(DATETIME);
            output.extend_from_slice(&((value.timestamp() as u64) ^ (1u64 << 63)).to_be_bytes());
            output.extend_from_slice(&value.timestamp_subsec_nanos().to_be_bytes());
        }
        Value::String(value) => {
            output.push(STRING);
            for byte in value.as_bytes() {
                if *byte == 0 {
                    output.extend_from_slice(&[0, 0xff]);
                } else {
                    output.push(*byte);
                }
            }
            output.extend_from_slice(&[0, 0]);
        }
        Value::Reference(value) => {
            output.push(REFERENCE);
            output.extend_from_slice(&value.packed().to_be_bytes());
        }
        Value::GlobalReference(value) => {
            output.push(GLOBAL_REFERENCE);
            output.extend_from_slice(&value.shard_id.to_be_bytes());
            output.extend_from_slice(&value.local_offset.to_be_bytes());
            output.extend_from_slice(&value.flags.to_be_bytes());
        }
        Value::List(_) | Value::Object(_) => {
            return Err(OQLError::storage_error(
                "lists and objects cannot be used as ordered index keys",
                None,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn assert_order(values: Vec<Value>) {
        let encoded = values
            .iter()
            .map(TypedKey::encode_one)
            .collect::<OQLResult<Vec<_>>>()
            .unwrap();
        assert!(encoded.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn signed_integer_and_float_order_is_memcomparable() {
        assert_order(vec![
            Value::Integer(i64::MIN),
            Value::Integer(-1),
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(i64::MAX),
        ]);
        assert_order(vec![
            Value::Float(f64::NEG_INFINITY),
            Value::Float(-10.5),
            Value::Float(-0.0),
            Value::Float(0.5),
            Value::Float(f64::INFINITY),
        ]);
        assert_eq!(
            TypedKey::encode_one(&Value::Float(-0.0)).unwrap(),
            TypedKey::encode_one(&Value::Float(0.0)).unwrap()
        );
        assert!(TypedKey::encode_one(&Value::Float(f64::NAN)).is_err());
    }

    #[test]
    fn strings_preserve_byte_order_and_component_boundaries() {
        assert_order(vec![
            Value::String(String::new()),
            Value::String("a".into()),
            Value::String("a\0".into()),
            Value::String("aa".into()),
            Value::String("b".into()),
        ]);
        assert_ne!(
            TypedKey::encode(&[Value::String("a".into()), Value::String("bc".into())]).unwrap(),
            TypedKey::encode(&[Value::String("ab".into()), Value::String("c".into())]).unwrap()
        );
    }

    #[test]
    fn timestamps_and_composite_prefixes_are_ordered() {
        let old = Utc.timestamp_opt(-1, 999_999_999).unwrap();
        let epoch = Utc.timestamp_opt(0, 0).unwrap();
        let later = Utc.timestamp_opt(0, 1).unwrap();
        assert_order(vec![
            Value::DateTime(old),
            Value::DateTime(epoch),
            Value::DateTime(later),
        ]);

        let first = TypedKey::encode(&[Value::String("a".into()), Value::Integer(1)]).unwrap();
        let second = TypedKey::encode(&[Value::String("a".into()), Value::Integer(2)]).unwrap();
        assert!(first < second);
    }

    #[test]
    fn composite_scalar_keys_round_trip_for_index_only_filtering() {
        let values = vec![
            Value::Null,
            Value::Boolean(true),
            Value::Integer(-42),
            Value::Float(-10.5),
            Value::DateTime(Utc.timestamp_opt(1_700_000_000, 123).unwrap()),
            Value::String("zero\0byte".into()),
            Value::Reference(ObjRef::from_parts(2, 3, 4)),
            Value::GlobalReference(GlobalObjRef {
                shard_id: 5,
                local_offset: 6,
                flags: 7,
            }),
        ];
        let encoded = TypedKey::encode(&values).unwrap();
        assert_eq!(TypedKey::decode(&encoded).unwrap(), values);
        for (position, expected) in values.into_iter().enumerate() {
            assert_eq!(
                TypedKey::decode_component(&encoded, position).unwrap(),
                expected
            );
        }
        assert!(TypedKey::decode_component(&encoded, 8).is_err());
    }
}
