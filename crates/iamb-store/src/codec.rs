//! Conversions between model values and `SQLite` values.

use std::fmt;

use iamb_core::model::{
    CaseMode, Id, Permission, ProfileKind, Role, SnippetKind, SyncMeta, Timestamp, Trigger,
};
use rusqlite::Row;
use rusqlite::types::{Type, Value};

use crate::error::StoreError;

/// A value read from the database broke a model rule.
#[derive(Debug)]
pub(crate) struct DecodeError(pub(crate) String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DecodeError {}

/// An enum stored as TEXT.
pub(crate) trait TextEnum: Sized + Copy {
    fn as_text(self) -> &'static str;
    fn from_text(s: &str) -> Option<Self>;
}

macro_rules! text_enum {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl TextEnum for $ty {
            fn as_text(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
            fn from_text(s: &str) -> Option<Self> {
                match s {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

text_enum!(ProfileKind { Personal => "personal", Family => "family", Company => "company" });
text_enum!(Role { Owner => "owner", Admin => "admin", Member => "member" });
text_enum!(Permission { View => "view", Edit => "edit" });
text_enum!(SnippetKind { Plain => "plain", Rich => "rich" });
text_enum!(Trigger { Immediate => "immediate", Delimiter => "delimiter" });
text_enum!(CaseMode { Exact => "exact", Insensitive => "insensitive", Adaptive => "adaptive" });

// ---- writing --------------------------------------------------------------------------------

pub(crate) fn id_val(id: Id) -> Value {
    Value::Blob(id.as_bytes().to_vec())
}

pub(crate) fn opt_id_val(id: Option<Id>) -> Value {
    id.map_or(Value::Null, id_val)
}

pub(crate) fn text_val(s: &str) -> Value {
    Value::Text(s.to_owned())
}

pub(crate) fn opt_text_val(s: Option<&str>) -> Value {
    s.map_or(Value::Null, text_val)
}

pub(crate) fn enum_val<T: TextEnum>(v: T) -> Value {
    Value::Text(v.as_text().to_owned())
}

pub(crate) fn opt_enum_val<T: TextEnum>(v: Option<T>) -> Value {
    v.map_or(Value::Null, enum_val)
}

pub(crate) fn int_val(v: i64) -> Value {
    Value::Integer(v)
}

pub(crate) fn bool_val(v: bool) -> Value {
    Value::Integer(i64::from(v))
}

pub(crate) fn ts_val(t: Timestamp) -> Value {
    Value::Integer(t.0)
}

pub(crate) fn opt_ts_val(t: Option<Timestamp>) -> Value {
    t.map_or(Value::Null, ts_val)
}

/// A `u64` as a signed `SQLite` integer; fails above `i64::MAX`.
pub(crate) fn u64_val(v: u64, what: &str) -> Result<Value, StoreError> {
    i64::try_from(v).map(Value::Integer).map_err(|_| {
        StoreError::InvalidData(format!(
            "{what} {v} does not fit in a signed 64-bit integer"
        ))
    })
}

/// Same as [`u64_val`] but returns the integer.
pub(crate) fn u64_int(v: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(v).map_err(|_| {
        StoreError::InvalidData(format!(
            "{what} {v} does not fit in a signed 64-bit integer"
        ))
    })
}

// ---- reading --------------------------------------------------------------------------------

pub(crate) fn decode_err(idx: usize, ty: Type, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, ty, Box::new(DecodeError(msg)))
}

pub(crate) fn id_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Id> {
    row.get::<_, [u8; 16]>(idx).map(Id::from_bytes)
}

pub(crate) fn opt_id_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Id>> {
    row.get::<_, Option<[u8; 16]>>(idx)
        .map(|o| o.map(Id::from_bytes))
}

pub(crate) fn ts_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Timestamp> {
    row.get::<_, i64>(idx).map(Timestamp)
}

pub(crate) fn opt_ts_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<Timestamp>> {
    row.get::<_, Option<i64>>(idx).map(|o| o.map(Timestamp))
}

pub(crate) fn enum_at<T: TextEnum>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s = row.get_ref(idx)?.as_str()?;
    T::from_text(s).ok_or_else(|| decode_err(idx, Type::Text, format!("unknown value {s:?}")))
}

pub(crate) fn opt_enum_at<T: TextEnum>(row: &Row<'_>, idx: usize) -> rusqlite::Result<Option<T>> {
    match row.get_ref(idx)?.as_str_or_null()? {
        None => Ok(None),
        Some(s) => T::from_text(s)
            .map(Some)
            .ok_or_else(|| decode_err(idx, Type::Text, format!("unknown value {s:?}"))),
    }
}

/// Reads a non-negative integer as `u64`.
pub(crate) fn u64_at(row: &Row<'_>, idx: usize) -> rusqlite::Result<u64> {
    let v: i64 = row.get(idx)?;
    u64::try_from(v).map_err(|_| decode_err(idx, Type::Integer, format!("negative value {v}")))
}

/// Reads the six common columns, which every entity query selects first.
pub(crate) fn meta_at(row: &Row<'_>) -> rusqlite::Result<SyncMeta> {
    Ok(SyncMeta {
        id: id_at(row, 0)?,
        created_at: ts_at(row, 1)?,
        updated_at: ts_at(row, 2)?,
        updated_by: opt_id_at(row, 3)?,
        rev: u64_at(row, 4)?,
        deleted_at: opt_ts_at(row, 5)?,
    })
}
