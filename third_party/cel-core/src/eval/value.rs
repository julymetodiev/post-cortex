//! Runtime values for CEL evaluation.
//!
//! `Value` represents all CEL values at runtime, including primitive types,
//! collections, timestamps, durations, and special values like errors and optionals.
//!
//! # Creating Values
//!
//! Use Rust's standard `Into` trait to create values from native types:
//!
//! ```rust
//! use cel_core::Value;
//!
//! // Primitives - integer types automatically widen to i64/u64
//! let v: Value = 42.into();      // i32 -> Value::Int(i64)
//! let v: Value = 42i64.into();   // i64 -> Value::Int(i64)
//! let v: Value = 42u32.into();   // u32 -> Value::UInt(u64)
//! let v: Value = true.into();
//! let v: Value = "hello".into();
//!
//! // Collections
//! let list: Value = vec![Value::Int(1), Value::Int(2)].into();
//!
//! // From compile-time constants
//! use cel_core::CelValue;
//! let cv = CelValue::Int(42);
//! let v: Value = cv.into();
//! ```
//!
//! # Extracting Values
//!
//! Use `TryFrom` to extract native types from values:
//!
//! ```rust
//! use cel_core::{Value, Timestamp, Duration, ValueMap, OptionalValue, MapKey};
//! use std::convert::TryFrom;
//!
//! // Primitive types
//! let v = Value::Int(42);
//! let i: i64 = i64::try_from(&v).unwrap();
//!
//! // Timestamp and Duration (Copy types - both owned and borrowed work)
//! let v = Value::Timestamp(Timestamp::new(1234567890, 0));
//! let t: Timestamp = Timestamp::try_from(&v).unwrap();
//!
//! let v = Value::Duration(Duration::new(60, 0));
//! let d: Duration = Duration::try_from(&v).unwrap();
//!
//! // Collections (borrowed references)
//! let v: Value = vec![Value::Int(1), Value::Int(2)].into();
//! let list: &[Value] = <&[Value]>::try_from(&v).unwrap();
//!
//! // Maps
//! let v = Value::map([(MapKey::String("key".into()), Value::Int(42))]);
//! let map: &ValueMap = <&ValueMap>::try_from(&v).unwrap();
//!
//! // Optionals
//! let v = Value::optional_some(Value::Int(42));
//! let opt: &OptionalValue = <&OptionalValue>::try_from(&v).unwrap();
//! ```

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use super::message::MessageValue;
use super::EvalError;
use crate::types::{CelType, CelValue};

/// Error returned when converting from Value to a specific type fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError {
    /// The expected type name.
    pub expected: &'static str,
    /// The actual type name found.
    pub found: String,
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "expected {}, found {}", self.expected, self.found)
    }
}

impl std::error::Error for ValueError {}

/// A CEL runtime value.
///
/// This enum represents all possible values that can exist during CEL evaluation.
/// Unlike `CelValue` (which represents compile-time constants), `Value` supports
/// the full range of CEL types including collections, timestamps, and errors.
#[derive(Debug, Clone)]
pub enum Value {
    /// Null value.
    Null,
    /// Boolean value.
    Bool(bool),
    /// Signed 64-bit integer.
    Int(i64),
    /// Unsigned 64-bit integer.
    UInt(u64),
    /// 64-bit floating point.
    Double(f64),
    /// Unicode string (Arc for cheap cloning).
    String(Arc<str>),
    /// Byte sequence (Arc for cheap cloning).
    Bytes(Arc<[u8]>),
    /// Homogeneous list.
    List(Arc<[Value]>),
    /// Key-value map (uses BTreeMap for deterministic iteration).
    Map(Arc<ValueMap>),
    /// Timestamp (seconds and nanos since Unix epoch).
    Timestamp(Timestamp),
    /// Duration (seconds and nanos).
    Duration(Duration),
    /// Type value (represents a CEL type at runtime).
    Type(TypeValue),
    /// Optional value (present or absent).
    Optional(OptionalValue),
    /// Protobuf message value.
    Message(Box<dyn MessageValue>),
    /// Enum value with type information (strong enum typing).
    Enum(EnumValue),
    /// Error value (evaluation errors propagate as values).
    Error(Arc<EvalError>),
}

/// A CEL enum value with type information.
///
/// Used for strong enum typing where enum values carry their
/// fully qualified type name alongside the numeric value.
#[derive(Debug, Clone)]
pub struct EnumValue {
    /// The fully qualified enum type name (e.g., "cel.expr.conformance.proto3.GlobalEnum").
    pub type_name: Arc<str>,
    /// The numeric enum value.
    pub value: i32,
}

impl EnumValue {
    /// Create a new enum value.
    pub fn new(type_name: impl Into<Arc<str>>, value: i32) -> Self {
        Self {
            type_name: type_name.into(),
            value,
        }
    }
}

impl PartialEq for EnumValue {
    fn eq(&self, other: &Self) -> bool {
        self.type_name == other.type_name && self.value == other.value
    }
}

/// A CEL timestamp value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    /// Seconds since Unix epoch.
    pub seconds: i64,
    /// Nanoseconds (0..999_999_999).
    pub nanos: i32,
}

impl Timestamp {
    /// Minimum valid timestamp: Year 0001-01-01 00:00:00 UTC
    /// This is approximately -62135596800 seconds from Unix epoch.
    pub const MIN_SECONDS: i64 = -62135596800;

    /// Maximum valid timestamp: Year 9999-12-31 23:59:59 UTC
    /// This is approximately 253402300799 seconds from Unix epoch.
    pub const MAX_SECONDS: i64 = 253402300799;

    /// Create a new timestamp.
    pub fn new(seconds: i64, nanos: i32) -> Self {
        Self { seconds, nanos }
    }

    /// Create a timestamp from seconds since Unix epoch.
    pub fn from_seconds(seconds: i64) -> Self {
        Self { seconds, nanos: 0 }
    }

    /// Returns true if this timestamp is before another.
    pub fn is_before(&self, other: &Timestamp) -> bool {
        (self.seconds, self.nanos) < (other.seconds, other.nanos)
    }

    /// Returns true if this timestamp is after another.
    pub fn is_after(&self, other: &Timestamp) -> bool {
        (self.seconds, self.nanos) > (other.seconds, other.nanos)
    }

    /// Check if this timestamp is within the valid CEL range (Year 0001 to 9999).
    pub fn is_valid(&self) -> bool {
        self.seconds >= Self::MIN_SECONDS && self.seconds <= Self::MAX_SECONDS
    }

    /// Convert to a chrono DateTime<Utc>.
    pub fn to_datetime_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::from_timestamp(self.seconds, self.nanos as u32)
    }

    /// Create from a chrono DateTime<Utc>.
    pub fn from_datetime<Tz: chrono::TimeZone>(dt: &chrono::DateTime<Tz>) -> Self {
        Self {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        }
    }

    /// Convert to total nanoseconds since Unix epoch.
    pub fn to_nanos(&self) -> Option<i128> {
        let secs = self.seconds as i128;
        let nanos = self.nanos as i128;
        secs.checked_mul(1_000_000_000).map(|s| s + nanos)
    }
}

/// A CEL duration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duration {
    /// Seconds component.
    pub seconds: i64,
    /// Nanoseconds component (0..999_999_999 for positive durations,
    /// -999_999_999..0 for negative durations).
    pub nanos: i32,
}

impl Duration {
    /// Maximum valid duration in seconds.
    /// This is set to one less than the span of the valid timestamp range,
    /// ensuring that subtracting the min timestamp from the max timestamp
    /// (or vice versa) produces a range error.
    pub const MAX_SECONDS: i64 = 315_537_897_598;

    /// Minimum valid duration in seconds (negative max).
    pub const MIN_SECONDS: i64 = -315_537_897_598;

    /// Create a new duration.
    pub fn new(seconds: i64, nanos: i32) -> Self {
        Self { seconds, nanos }
    }

    /// Create a duration from seconds.
    pub fn from_seconds(seconds: i64) -> Self {
        Self { seconds, nanos: 0 }
    }

    /// Create a duration from nanoseconds.
    pub fn from_nanos(nanos: i64) -> Self {
        let seconds = nanos / 1_000_000_000;
        let nanos = (nanos % 1_000_000_000) as i32;
        Self { seconds, nanos }
    }

    /// Convert to total nanoseconds.
    pub fn to_nanos(&self) -> i64 {
        self.seconds
            .saturating_mul(1_000_000_000)
            .saturating_add(self.nanos as i64)
    }

    /// Returns true if this duration is negative.
    pub fn is_negative(&self) -> bool {
        self.seconds < 0 || (self.seconds == 0 && self.nanos < 0)
    }

    /// Check if this duration is within the valid CEL range (~10000 years).
    pub fn is_valid(&self) -> bool {
        self.seconds >= Self::MIN_SECONDS && self.seconds <= Self::MAX_SECONDS
    }

    /// Convert to a chrono Duration.
    pub fn to_chrono(&self) -> chrono::Duration {
        chrono::Duration::seconds(self.seconds) + chrono::Duration::nanoseconds(self.nanos as i64)
    }

    /// Create from a chrono Duration.
    pub fn from_chrono(d: chrono::Duration) -> Self {
        let total_nanos = d.num_nanoseconds().unwrap_or(0);
        Self::from_nanos(total_nanos)
    }

    /// Get total hours (truncated).
    pub fn get_hours(&self) -> i64 {
        self.total_seconds() / 3600
    }

    /// Get total minutes (truncated).
    pub fn get_minutes(&self) -> i64 {
        self.total_seconds() / 60
    }

    /// Get total seconds.
    pub fn total_seconds(&self) -> i64 {
        self.seconds
    }

    /// Get milliseconds component (0-999).
    pub fn get_milliseconds(&self) -> i64 {
        // For negative durations, nanos is also negative
        let ms = if self.nanos >= 0 {
            self.nanos / 1_000_000
        } else {
            -(-self.nanos / 1_000_000)
        };
        ms as i64
    }
}

/// A CEL type value (runtime representation of types).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeValue {
    /// The type name as it appears in CEL.
    pub name: Arc<str>,
}

impl TypeValue {
    /// Create a new type value.
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self { name: name.into() }
    }

    /// Common type values.
    pub fn null_type() -> Self {
        Self::new("null_type")
    }
    pub fn bool_type() -> Self {
        Self::new("bool")
    }
    pub fn int_type() -> Self {
        Self::new("int")
    }
    pub fn uint_type() -> Self {
        Self::new("uint")
    }
    pub fn double_type() -> Self {
        Self::new("double")
    }
    pub fn string_type() -> Self {
        Self::new("string")
    }
    pub fn bytes_type() -> Self {
        Self::new("bytes")
    }
    pub fn list_type() -> Self {
        Self::new("list")
    }
    pub fn map_type() -> Self {
        Self::new("map")
    }
    pub fn timestamp_type() -> Self {
        Self::new("google.protobuf.Timestamp")
    }
    pub fn duration_type() -> Self {
        Self::new("google.protobuf.Duration")
    }
    pub fn type_type() -> Self {
        Self::new("type")
    }
    pub fn optional_type() -> Self {
        Self::new("optional")
    }
}

/// A CEL optional value.
#[derive(Debug, Clone)]
pub enum OptionalValue {
    /// An absent optional value.
    None,
    /// A present optional value.
    Some(Box<Value>),
}

impl OptionalValue {
    /// Create an absent optional.
    pub fn none() -> Self {
        OptionalValue::None
    }

    /// Create a present optional.
    pub fn some(value: Value) -> Self {
        OptionalValue::Some(Box::new(value))
    }

    /// Returns true if the optional is present.
    pub fn is_present(&self) -> bool {
        matches!(self, OptionalValue::Some(_))
    }

    /// Get the inner value, or None if absent.
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            OptionalValue::None => None,
            OptionalValue::Some(v) => Some(v),
        }
    }

    /// Unwrap the value or return a default.
    pub fn unwrap_or(self, default: Value) -> Value {
        match self {
            OptionalValue::None => default,
            OptionalValue::Some(v) => *v,
        }
    }
}

/// A CEL map with heterogeneous keys.
///
/// Uses a BTreeMap with a custom key type for deterministic iteration order.
#[derive(Debug, Clone, Default)]
pub struct ValueMap {
    entries: BTreeMap<MapKey, Value>,
}

/// A map key that supports CEL's key types.
///
/// CEL allows bool, int, uint, and string as map keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapKey {
    Bool(bool),
    Int(i64),
    UInt(u64),
    String(Arc<str>),
}

impl MapKey {
    /// Create a map key from a Value.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Bool(b) => Some(MapKey::Bool(*b)),
            Value::Int(i) => Some(MapKey::Int(*i)),
            Value::UInt(u) => Some(MapKey::UInt(*u)),
            Value::String(s) => Some(MapKey::String(s.clone())),
            _ => None,
        }
    }

    /// Convert back to a Value.
    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Bool(b) => Value::Bool(*b),
            MapKey::Int(i) => Value::Int(*i),
            MapKey::UInt(u) => Value::UInt(*u),
            MapKey::String(s) => Value::String(s.clone()),
        }
    }
}

/// A map key borrowed for a lookup (pcx patch), ordered as [`MapKey`] is:
/// the variants in the same order, text by its characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KeyRef<'a> {
    Bool(bool),
    Int(i64),
    UInt(u64),
    String(&'a str),
}

/// A key as a lookup compares it: an owned [`MapKey`] or a borrowed
/// [`KeyRef`] (pcx patch).
trait Keyed {
    fn key_ref(&self) -> KeyRef<'_>;
}

impl Keyed for MapKey {
    fn key_ref(&self) -> KeyRef<'_> {
        match self {
            MapKey::Bool(b) => KeyRef::Bool(*b),
            MapKey::Int(i) => KeyRef::Int(*i),
            MapKey::UInt(u) => KeyRef::UInt(*u),
            MapKey::String(s) => KeyRef::String(s),
        }
    }
}

impl Keyed for KeyRef<'_> {
    fn key_ref(&self) -> KeyRef<'_> {
        *self
    }
}

impl<'a> Borrow<dyn Keyed + 'a> for MapKey {
    fn borrow(&self) -> &(dyn Keyed + 'a) {
        self
    }
}

impl PartialEq for dyn Keyed + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.key_ref() == other.key_ref()
    }
}

impl Eq for dyn Keyed + '_ {}

impl PartialOrd for dyn Keyed + '_ {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for dyn Keyed + '_ {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key_ref().cmp(&other.key_ref())
    }
}

impl From<&str> for MapKey {
    fn from(s: &str) -> Self {
        MapKey::String(Arc::from(s))
    }
}

impl From<String> for MapKey {
    fn from(s: String) -> Self {
        MapKey::String(Arc::from(s))
    }
}

impl From<Arc<str>> for MapKey {
    fn from(s: Arc<str>) -> Self {
        MapKey::String(s)
    }
}

impl From<bool> for MapKey {
    fn from(b: bool) -> Self {
        MapKey::Bool(b)
    }
}

impl From<i64> for MapKey {
    fn from(i: i64) -> Self {
        MapKey::Int(i)
    }
}

impl From<u64> for MapKey {
    fn from(u: u64) -> Self {
        MapKey::UInt(u)
    }
}

impl From<i32> for MapKey {
    fn from(i: i32) -> Self {
        MapKey::Int(i as i64)
    }
}

impl From<u32> for MapKey {
    fn from(u: u32) -> Self {
        MapKey::UInt(u as u64)
    }
}

impl ValueMap {
    /// Create a new empty map.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Create a map from an iterator of key-value pairs.
    pub fn from_entries(entries: impl IntoIterator<Item = (MapKey, Value)>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    /// Get a value by key.
    pub fn get(&self, key: &MapKey) -> Option<&Value> {
        self.entries.get(key)
    }

    /// The value of the text key `key`, looked up without making a key
    /// (pcx patch: a member read `m.name` allocated an `Arc<str>` for each
    /// lookup).
    pub fn get_str(&self, key: &str) -> Option<&Value> {
        self.entries.get(&KeyRef::String(key) as &dyn Keyed)
    }

    /// Whether the text key `key` is there, without making a key (pcx
    /// patch).
    pub fn contains_str(&self, key: &str) -> bool {
        self.entries.contains_key(&KeyRef::String(key) as &dyn Keyed)
    }

    /// Insert a key-value pair.
    pub fn insert(&mut self, key: MapKey, value: Value) {
        self.entries.insert(key, value);
    }

    /// Check if a key exists.
    pub fn contains_key(&self, key: &MapKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Get a value by key with cross-type numeric coercion.
    /// Tries exact match first, then Int↔UInt coercion for in-range values.
    pub fn get_with_numeric_coercion(&self, key: &MapKey) -> Option<&Value> {
        if let Some(v) = self.entries.get(key) {
            return Some(v);
        }
        match key {
            MapKey::Int(i) => {
                if *i >= 0 {
                    self.entries.get(&MapKey::UInt(*i as u64))
                } else {
                    None
                }
            }
            MapKey::UInt(u) => {
                if *u <= i64::MAX as u64 {
                    self.entries.get(&MapKey::Int(*u as i64))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check if a key exists with cross-type numeric coercion.
    pub fn contains_key_with_numeric_coercion(&self, key: &MapKey) -> bool {
        self.get_with_numeric_coercion(key).is_some()
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the map is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over entries.
    pub fn iter(&self) -> impl Iterator<Item = (&MapKey, &Value)> {
        self.entries.iter()
    }

    /// Iterate over keys.
    pub fn keys(&self) -> impl Iterator<Item = &MapKey> {
        self.entries.keys()
    }

    /// Iterate over values.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.values()
    }
}

// ==================== Value Constructors ====================

impl Value {
    /// Create a map value from key-value pairs.
    ///
    /// Keys and values are automatically converted using `Into<MapKey>` and `Into<Value>`.
    ///
    /// ```
    /// use cel_core::Value;
    ///
    /// // String keys and values
    /// let map = Value::map([("host", "localhost"), ("port", "8080")]);
    ///
    /// // Integer keys
    /// let map = Value::map([(1i64, "one"), (2i64, "two")]);
    ///
    /// // Mixed value types require explicit Value construction
    /// let map = Value::map([
    ///     ("name", Value::from("Alice")),
    ///     ("age", Value::from(30i64)),
    /// ]);
    /// ```
    pub fn map<K, V>(entries: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<MapKey>,
        V: Into<Value>,
    {
        Value::Map(Arc::new(ValueMap::from_entries(
            entries.into_iter().map(|(k, v)| (k.into(), v.into())),
        )))
    }

    /// Create a list value from items.
    ///
    /// Items are automatically converted using `Into<Value>`.
    ///
    /// ```
    /// use cel_core::Value;
    ///
    /// // Integer list (no i64 suffix needed)
    /// let list = Value::list([1, 2, 3]);
    ///
    /// // String list
    /// let list = Value::list(["a", "b", "c"]);
    ///
    /// // Mixed types require explicit Value construction
    /// let list = Value::list([Value::from(1), Value::from("two")]);
    /// ```
    pub fn list<T>(items: impl IntoIterator<Item = T>) -> Self
    where
        T: Into<Value>,
    {
        Value::List(Arc::from(
            items.into_iter().map(Into::into).collect::<Vec<_>>(),
        ))
    }

    /// Create a timestamp value.
    pub fn timestamp(seconds: i64, nanos: i32) -> Self {
        Value::Timestamp(Timestamp::new(seconds, nanos))
    }

    /// Create a duration value.
    pub fn duration(seconds: i64, nanos: i32) -> Self {
        Value::Duration(Duration::new(seconds, nanos))
    }

    /// Create a type value.
    pub fn new_type(name: impl Into<Arc<str>>) -> Self {
        Value::Type(TypeValue::new(name))
    }

    /// Create an optional none value.
    pub fn optional_none() -> Self {
        Value::Optional(OptionalValue::None)
    }

    /// Create an optional some value.
    pub fn optional_some(value: Value) -> Self {
        Value::Optional(OptionalValue::some(value))
    }

    /// Convert an Option<T> to a CEL optional value.
    ///
    /// Unlike `From<Option<T>>` which maps `None` to `Value::Null`,
    /// this preserves CEL optional semantics.
    ///
    /// ```
    /// use cel_core::Value;
    ///
    /// let some_val = Value::from_option(Some(42));  // optional.of(42)
    /// let none_val = Value::from_option(None::<i32>);  // optional.none()
    /// ```
    pub fn from_option<T: Into<Value>>(opt: Option<T>) -> Self {
        match opt {
            Some(v) => Value::optional_some(v.into()),
            None => Value::optional_none(),
        }
    }

    /// Create an error value.
    pub fn error(err: impl Into<EvalError>) -> Self {
        Value::Error(Arc::new(err.into()))
    }
}

// ==================== From Implementations ====================

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<u64> for Value {
    fn from(u: u64) -> Self {
        Value::UInt(u)
    }
}

// Signed integer widening conversions
impl From<i8> for Value {
    fn from(i: i8) -> Self {
        Value::Int(i as i64)
    }
}

impl From<i16> for Value {
    fn from(i: i16) -> Self {
        Value::Int(i as i64)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Value::Int(i as i64)
    }
}

impl From<isize> for Value {
    fn from(i: isize) -> Self {
        Value::Int(i as i64)
    }
}

// Unsigned integer widening conversions
impl From<u8> for Value {
    fn from(u: u8) -> Self {
        Value::UInt(u as u64)
    }
}

impl From<u16> for Value {
    fn from(u: u16) -> Self {
        Value::UInt(u as u64)
    }
}

impl From<u32> for Value {
    fn from(u: u32) -> Self {
        Value::UInt(u as u64)
    }
}

impl From<usize> for Value {
    fn from(u: usize) -> Self {
        Value::UInt(u as u64)
    }
}

impl From<f64> for Value {
    fn from(d: f64) -> Self {
        Value::Double(d)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(Arc::from(s))
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(Arc::from(s))
    }
}

impl From<Arc<str>> for Value {
    fn from(s: Arc<str>) -> Self {
        Value::String(s)
    }
}

impl From<&[u8]> for Value {
    fn from(b: &[u8]) -> Self {
        Value::Bytes(Arc::from(b))
    }
}

impl From<Vec<u8>> for Value {
    fn from(b: Vec<u8>) -> Self {
        Value::Bytes(Arc::from(b))
    }
}

impl From<Arc<[u8]>> for Value {
    fn from(b: Arc<[u8]>) -> Self {
        Value::Bytes(b)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(Arc::from(v))
    }
}

impl From<Timestamp> for Value {
    fn from(t: Timestamp) -> Self {
        Value::Timestamp(t)
    }
}

impl From<Duration> for Value {
    fn from(d: Duration) -> Self {
        Value::Duration(d)
    }
}

impl From<EvalError> for Value {
    fn from(e: EvalError) -> Self {
        Value::Error(Arc::new(e))
    }
}

impl From<CelValue> for Value {
    fn from(cv: CelValue) -> Self {
        match cv {
            CelValue::Null => Value::Null,
            CelValue::Bool(b) => Value::Bool(b),
            CelValue::Int(i) => Value::Int(i),
            CelValue::UInt(u) => Value::UInt(u),
            CelValue::Double(d) => Value::Double(d),
            CelValue::String(s) => Value::String(Arc::from(s)),
            CelValue::Bytes(b) => Value::Bytes(Arc::from(b)),
        }
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

// ==================== TryFrom Implementations ====================

impl TryFrom<Value> for bool {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Bool(b) => Ok(b),
            other => Err(ValueError {
                expected: "bool",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<&Value> for bool {
    type Error = ValueError;
    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        match v {
            Value::Bool(b) => Ok(*b),
            other => Err(ValueError {
                expected: "bool",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for i64 {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Int(i) => Ok(i),
            other => Err(ValueError {
                expected: "int",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<&Value> for i64 {
    type Error = ValueError;
    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        match v {
            Value::Int(i) => Ok(*i),
            other => Err(ValueError {
                expected: "int",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for u64 {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::UInt(u) => Ok(u),
            other => Err(ValueError {
                expected: "uint",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<&Value> for u64 {
    type Error = ValueError;
    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        match v {
            Value::UInt(u) => Ok(*u),
            other => Err(ValueError {
                expected: "uint",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for f64 {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Double(d) => Ok(d),
            other => Err(ValueError {
                expected: "double",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<&Value> for f64 {
    type Error = ValueError;
    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        match v {
            Value::Double(d) => Ok(*d),
            other => Err(ValueError {
                expected: "double",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for String {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::String(s) => Ok(s.to_string()),
            other => Err(ValueError {
                expected: "string",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl<'a> TryFrom<&'a Value> for &'a str {
    type Error = ValueError;
    fn try_from(v: &'a Value) -> Result<Self, Self::Error> {
        match v {
            Value::String(s) => Ok(s),
            other => Err(ValueError {
                expected: "string",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl<'a> TryFrom<&'a Value> for &'a [u8] {
    type Error = ValueError;
    fn try_from(v: &'a Value) -> Result<Self, Self::Error> {
        match v {
            Value::Bytes(b) => Ok(b),
            other => Err(ValueError {
                expected: "bytes",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for Timestamp {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Timestamp(t) => Ok(t),
            other => Err(ValueError {
                expected: "timestamp",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<&Value> for Timestamp {
    type Error = ValueError;
    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        match v {
            Value::Timestamp(t) => Ok(*t),
            other => Err(ValueError {
                expected: "timestamp",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<Value> for Duration {
    type Error = ValueError;
    fn try_from(v: Value) -> Result<Self, Self::Error> {
        match v {
            Value::Duration(d) => Ok(d),
            other => Err(ValueError {
                expected: "duration",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl TryFrom<&Value> for Duration {
    type Error = ValueError;
    fn try_from(v: &Value) -> Result<Self, Self::Error> {
        match v {
            Value::Duration(d) => Ok(*d),
            other => Err(ValueError {
                expected: "duration",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl<'a> TryFrom<&'a Value> for &'a [Value] {
    type Error = ValueError;
    fn try_from(v: &'a Value) -> Result<Self, Self::Error> {
        match v {
            Value::List(l) => Ok(l),
            other => Err(ValueError {
                expected: "list",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl<'a> TryFrom<&'a Value> for &'a ValueMap {
    type Error = ValueError;
    fn try_from(v: &'a Value) -> Result<Self, Self::Error> {
        match v {
            Value::Map(m) => Ok(m.as_ref()),
            other => Err(ValueError {
                expected: "map",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl<'a> TryFrom<&'a Value> for &'a OptionalValue {
    type Error = ValueError;
    fn try_from(v: &'a Value) -> Result<Self, Self::Error> {
        match v {
            Value::Optional(o) => Ok(o),
            other => Err(ValueError {
                expected: "optional",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

impl<'a> TryFrom<&'a Value> for &'a EvalError {
    type Error = ValueError;
    fn try_from(v: &'a Value) -> Result<Self, Self::Error> {
        match v {
            Value::Error(e) => Ok(e.as_ref()),
            other => Err(ValueError {
                expected: "error",
                found: other.type_value().name.to_string(),
            }),
        }
    }
}

// ==================== Type Information ====================

impl Value {
    /// Get the CEL type of this value.
    pub fn cel_type(&self) -> CelType {
        match self {
            Value::Null => CelType::Null,
            Value::Bool(_) => CelType::Bool,
            Value::Int(_) => CelType::Int,
            Value::UInt(_) => CelType::UInt,
            Value::Double(_) => CelType::Double,
            Value::String(_) => CelType::String,
            Value::Bytes(_) => CelType::Bytes,
            Value::List(_) => CelType::list(CelType::Dyn),
            Value::Map(_) => CelType::map(CelType::Dyn, CelType::Dyn),
            Value::Timestamp(_) => CelType::Timestamp,
            Value::Duration(_) => CelType::Duration,
            Value::Type(_) => CelType::type_of(CelType::Dyn),
            Value::Optional(opt) => match opt {
                OptionalValue::None => CelType::optional(CelType::Dyn),
                OptionalValue::Some(v) => CelType::optional(v.cel_type()),
            },
            Value::Message(msg) => CelType::message(msg.type_name()),
            Value::Enum(ev) => CelType::enum_type(&ev.type_name),
            Value::Error(_) => CelType::Error,
        }
    }

    /// Get the CEL type value for this value (for the `type()` function).
    pub fn type_value(&self) -> TypeValue {
        match self {
            Value::Null => TypeValue::null_type(),
            Value::Bool(_) => TypeValue::bool_type(),
            Value::Int(_) => TypeValue::int_type(),
            Value::UInt(_) => TypeValue::uint_type(),
            Value::Double(_) => TypeValue::double_type(),
            Value::String(_) => TypeValue::string_type(),
            Value::Bytes(_) => TypeValue::bytes_type(),
            Value::List(_) => TypeValue::list_type(),
            Value::Map(_) => TypeValue::map_type(),
            Value::Timestamp(_) => TypeValue::timestamp_type(),
            Value::Duration(_) => TypeValue::duration_type(),
            Value::Type(_) => TypeValue::type_type(),
            Value::Optional(_) => TypeValue::new("optional"),
            Value::Message(msg) => TypeValue::new(msg.type_name()),
            Value::Enum(ev) => TypeValue::new(ev.type_name.as_ref()),
            Value::Error(_) => TypeValue::new("error"),
        }
    }

    /// Check if this value is an error.
    pub fn is_error(&self) -> bool {
        matches!(self, Value::Error(_))
    }

    /// Check if this value is null.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Check if this value is truthy (for bool values).
    ///
    /// Returns Some(true) for proto messages since they are always truthy.
    pub fn is_truthy(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Message(_) => Some(true), // Proto messages are always truthy
            _ => None,
        }
    }
}

// ==================== Equality ====================

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::UInt(a), Value::UInt(b)) => a == b,
            (Value::Double(a), Value::Double(b)) => {
                // CEL follows IEEE 754 semantics: NaN != NaN
                a == b
            }
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                for (key, val_a) in a.iter() {
                    match b.get_with_numeric_coercion(key) {
                        Some(val_b) if val_a == val_b => continue,
                        _ => return false,
                    }
                }
                true
            }
            (Value::Timestamp(a), Value::Timestamp(b)) => a == b,
            (Value::Duration(a), Value::Duration(b)) => a == b,
            (Value::Type(a), Value::Type(b)) => a == b,
            (Value::Optional(a), Value::Optional(b)) => match (a, b) {
                (OptionalValue::None, OptionalValue::None) => true,
                (OptionalValue::Some(va), OptionalValue::Some(vb)) => va == vb,
                _ => false,
            },
            (Value::Message(a), Value::Message(b)) => a.eq_message(b.as_ref()),
            (Value::Enum(a), Value::Enum(b)) => a == b,
            // Cross-type numeric equality (CEL spec: 42 == 42u == 42.0)
            (Value::Int(a), Value::UInt(b)) => {
                if *a < 0 {
                    false
                } else {
                    *a as u64 == *b
                }
            }
            (Value::UInt(a), Value::Int(b)) => {
                if *b < 0 {
                    false
                } else {
                    *a == *b as u64
                }
            }
            (Value::Int(a), Value::Double(b)) => {
                if b.is_nan() {
                    return false;
                }
                let a_f64 = *a as f64;
                a_f64 == *b && a_f64 as i64 == *a
            }
            (Value::Double(a), Value::Int(b)) => {
                if a.is_nan() {
                    return false;
                }
                let b_f64 = *b as f64;
                *a == b_f64 && b_f64 as i64 == *b
            }
            (Value::UInt(a), Value::Double(b)) => {
                if b.is_nan() {
                    return false;
                }
                let a_f64 = *a as f64;
                a_f64 == *b && a_f64 as u64 == *a
            }
            (Value::Double(a), Value::UInt(b)) => {
                if a.is_nan() {
                    return false;
                }
                let b_f64 = *b as f64;
                *a == b_f64 && b_f64 as u64 == *b
            }
            // Enum-numeric cross-type equality (CEL spec: enums compare as their int value)
            (Value::Enum(e), Value::Int(i)) | (Value::Int(i), Value::Enum(e)) => {
                e.value as i64 == *i
            }
            (Value::Enum(e), Value::UInt(u)) | (Value::UInt(u), Value::Enum(e)) => {
                if e.value < 0 {
                    false
                } else {
                    e.value as u64 == *u
                }
            }
            (Value::Enum(e), Value::Double(d)) | (Value::Double(d), Value::Enum(e)) => {
                if d.is_nan() {
                    return false;
                }
                let e_f64 = e.value as f64;
                e_f64 == *d && e_f64 as i32 == e.value
            }
            _ => false,
        }
    }
}

// ==================== Comparison ====================

impl Value {
    /// Compare two values, returning an ordering if comparable.
    ///
    /// CEL supports comparison between values of the same type,
    /// and between numeric types (int, uint, double).
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::UInt(a), Value::UInt(b)) => Some(a.cmp(b)),
            (Value::Double(a), Value::Double(b)) => a.partial_cmp(b),
            (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
            (Value::Bytes(a), Value::Bytes(b)) => Some(a.cmp(b)),
            (Value::Timestamp(a), Value::Timestamp(b)) => {
                Some((a.seconds, a.nanos).cmp(&(b.seconds, b.nanos)))
            }
            (Value::Duration(a), Value::Duration(b)) => {
                Some((a.seconds, a.nanos).cmp(&(b.seconds, b.nanos)))
            }
            // Cross-numeric comparisons
            (Value::Int(a), Value::UInt(b)) => {
                if *a < 0 {
                    Some(Ordering::Less)
                } else {
                    (*a as u64).partial_cmp(b)
                }
            }
            (Value::UInt(a), Value::Int(b)) => {
                if *b < 0 {
                    Some(Ordering::Greater)
                } else {
                    a.partial_cmp(&(*b as u64))
                }
            }
            (Value::Int(a), Value::Double(b)) => (*a as f64).partial_cmp(b),
            (Value::Double(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::UInt(a), Value::Double(b)) => (*a as f64).partial_cmp(b),
            (Value::Double(a), Value::UInt(b)) => a.partial_cmp(&(*b as f64)),
            // Enum-numeric cross-type ordering
            (Value::Enum(e), Value::Int(i)) => (e.value as i64).partial_cmp(i),
            (Value::Int(i), Value::Enum(e)) => i.partial_cmp(&(e.value as i64)),
            (Value::Enum(e), Value::UInt(u)) => {
                if e.value < 0 {
                    Some(Ordering::Less)
                } else {
                    (e.value as u64).partial_cmp(u)
                }
            }
            (Value::UInt(u), Value::Enum(e)) => {
                if e.value < 0 {
                    Some(Ordering::Greater)
                } else {
                    u.partial_cmp(&(e.value as u64))
                }
            }
            (Value::Enum(e), Value::Double(d)) => (e.value as f64).partial_cmp(d),
            (Value::Double(d), Value::Enum(e)) => d.partial_cmp(&(e.value as f64)),
            _ => None,
        }
    }
}

// ==================== Display ====================

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(v) => write!(f, "{}", v),
            Value::Int(v) => write!(f, "{}", v),
            Value::UInt(v) => write!(f, "{}u", v),
            Value::Double(v) => {
                if v.is_nan() {
                    write!(f, "NaN")
                } else if v.is_infinite() {
                    if v.is_sign_positive() {
                        write!(f, "+infinity")
                    } else {
                        write!(f, "-infinity")
                    }
                } else if v.fract() == 0.0 {
                    write!(f, "{}.0", v)
                } else {
                    write!(f, "{}", v)
                }
            }
            Value::String(v) => write!(f, "\"{}\"", v),
            Value::Bytes(v) => write!(f, "b\"{}\"", String::from_utf8_lossy(v)),
            Value::List(v) => {
                write!(f, "[")?;
                for (i, elem) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", elem)?;
                }
                write!(f, "]")
            }
            Value::Map(m) => {
                write!(f, "{{")?;
                for (i, (key, value)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", key.to_value(), value)?;
                }
                write!(f, "}}")
            }
            Value::Timestamp(t) => write!(f, "timestamp({})", t.seconds),
            Value::Duration(d) => write!(f, "duration({}s)", d.seconds),
            Value::Type(t) => write!(f, "type({})", t.name),
            Value::Optional(o) => match o {
                OptionalValue::None => write!(f, "optional.none()"),
                OptionalValue::Some(v) => write!(f, "optional.of({})", v),
            },
            Value::Message(m) => write!(f, "{}{{...}}", m.type_name()),
            Value::Enum(e) => write!(f, "{}({})", e.type_name, e.value),
            Value::Error(e) => write!(f, "error({})", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_equality() {
        assert_eq!(Value::Int(42), Value::Int(42));
        assert_ne!(Value::Int(42), Value::Int(43));
        // Cross-type numeric equality: 42 == 42u
        assert_eq!(Value::Int(42), Value::UInt(42));
        assert_ne!(Value::Int(-1), Value::UInt(1));
        let hello: Value = "hello".into();
        assert_eq!(hello, "hello".into());
    }

    #[test]
    fn test_value_comparison() {
        assert_eq!(Value::Int(1).compare(&Value::Int(2)), Some(Ordering::Less));
        assert_eq!(
            Value::Int(2).compare(&Value::Int(1)),
            Some(Ordering::Greater)
        );
        assert_eq!(Value::Int(1).compare(&Value::Int(1)), Some(Ordering::Equal));

        // Cross-numeric comparison
        assert_eq!(
            Value::Int(-1).compare(&Value::UInt(1)),
            Some(Ordering::Less)
        );
        assert_eq!(
            Value::Int(1).compare(&Value::Double(1.5)),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn test_map_operations() {
        let mut map = ValueMap::new();
        map.insert(MapKey::String(Arc::from("key")), Value::Int(42));

        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&MapKey::String(Arc::from("key"))),
            Some(&Value::Int(42))
        );
        assert!(map.contains_key(&MapKey::String(Arc::from("key"))));
        assert!(!map.contains_key(&MapKey::String(Arc::from("other"))));
    }

    #[test]
    fn a_text_key_is_found_without_making_one() {
        // pcx patch: get_str and contains_str agree with get over keys of
        // every kind, so the borrowed key orders as MapKey does.
        let map = ValueMap::from_entries([
            (MapKey::Bool(true), Value::Int(1)),
            (MapKey::Int(-3), Value::Int(2)),
            (MapKey::UInt(9), Value::Int(3)),
            (MapKey::String(Arc::from("b")), Value::Int(4)),
            (MapKey::String(Arc::from("a")), Value::Int(5)),
            (MapKey::String(Arc::from("")), Value::Int(6)),
        ]);
        for key in ["a", "b", ""] {
            assert_eq!(map.get_str(key), map.get(&MapKey::String(Arc::from(key))));
            assert!(map.contains_str(key));
        }
        assert_eq!(map.get_str("a"), Some(&Value::Int(5)));
        assert!(map.get_str("c").is_none());
        assert!(!map.contains_str("true"));
    }

    #[test]
    fn test_map_key_from() {
        // From &str
        let key: MapKey = "hello".into();
        assert_eq!(key, MapKey::String(Arc::from("hello")));

        // From String
        let key: MapKey = String::from("world").into();
        assert_eq!(key, MapKey::String(Arc::from("world")));

        // From i64
        let key: MapKey = 42i64.into();
        assert_eq!(key, MapKey::Int(42));

        // From u64
        let key: MapKey = 42u64.into();
        assert_eq!(key, MapKey::UInt(42));

        // From bool
        let key: MapKey = true.into();
        assert_eq!(key, MapKey::Bool(true));
    }

    #[test]
    fn test_value_map_ergonomic() {
        // Homogeneous string keys and values
        let map = Value::map([("a", "1"), ("b", "2")]);
        if let Value::Map(m) = &map {
            assert_eq!(m.len(), 2);
            assert_eq!(
                m.get(&MapKey::String(Arc::from("a"))),
                Some(&Value::String(Arc::from("1")))
            );
        } else {
            panic!("expected map");
        }

        // Integer keys
        let map = Value::map([(1i64, "one"), (2i64, "two")]);
        if let Value::Map(m) = &map {
            assert_eq!(
                m.get(&MapKey::Int(1)),
                Some(&Value::String(Arc::from("one")))
            );
        } else {
            panic!("expected map");
        }

        // Mixed value types with explicit Value::from
        let map = Value::map([("name", Value::from("Alice")), ("age", Value::from(30i64))]);
        if let Value::Map(m) = &map {
            assert_eq!(m.len(), 2);
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn test_optional_value() {
        let none = OptionalValue::none();
        assert!(!none.is_present());
        assert!(none.as_value().is_none());

        let some = OptionalValue::some(Value::Int(42));
        assert!(some.is_present());
        assert_eq!(some.as_value(), Some(&Value::Int(42)));
    }

    #[test]
    fn test_timestamp_comparison() {
        let t1 = Timestamp::new(100, 0);
        let t2 = Timestamp::new(200, 0);
        let t3 = Timestamp::new(100, 500);

        assert!(t1.is_before(&t2));
        assert!(t2.is_after(&t1));
        assert!(t1.is_before(&t3));
    }

    #[test]
    fn test_duration_nanos() {
        let d = Duration::from_nanos(1_500_000_000);
        assert_eq!(d.seconds, 1);
        assert_eq!(d.nanos, 500_000_000);
        assert_eq!(d.to_nanos(), 1_500_000_000);
    }

    #[test]
    fn test_cel_type() {
        assert_eq!(Value::Int(42).cel_type(), CelType::Int);
        let hello: Value = "hello".into();
        assert_eq!(hello.cel_type(), CelType::String);
        let list: Value = vec![Value::Int(1)].into();
        assert_eq!(list.cel_type(), CelType::list(CelType::Dyn));
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", Value::Null), "null");
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::UInt(42)), "42u");
        let hello: Value = "hello".into();
        assert_eq!(format!("{}", hello), "\"hello\"");
        assert_eq!(format!("{}", Value::Bool(true)), "true");
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_from_primitives() {
        let v: Value = true.into();
        assert_eq!(v, Value::Bool(true));

        let v: Value = 42i64.into();
        assert_eq!(v, Value::Int(42));

        let v: Value = 42u64.into();
        assert_eq!(v, Value::UInt(42));

        let v: Value = 3.14f64.into();
        assert_eq!(v, Value::Double(3.14));

        let v: Value = "hello".into();
        assert_eq!(v, Value::String(Arc::from("hello")));

        let v: Value = String::from("world").into();
        assert_eq!(v, Value::String(Arc::from("world")));
    }

    #[test]
    fn test_from_collections() {
        let v: Value = vec![Value::Int(1), Value::Int(2)].into();
        assert!(matches!(v, Value::List(_)));

        let v: Value = vec![1u8, 2, 3].into();
        assert!(matches!(v, Value::Bytes(_)));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_try_from_success() {
        let v = Value::Int(42);
        let i: i64 = (&v).try_into().unwrap();
        assert_eq!(i, 42);

        let v: Value = "hello".into();
        let s: &str = (&v).try_into().unwrap();
        assert_eq!(s, "hello");

        let v = Value::Bool(true);
        let b: bool = (&v).try_into().unwrap();
        assert!(b);

        let v = Value::UInt(100);
        let u: u64 = (&v).try_into().unwrap();
        assert_eq!(u, 100);

        let v = Value::Double(3.14);
        let d: f64 = (&v).try_into().unwrap();
        assert_eq!(d, 3.14);
    }

    #[test]
    fn test_try_from_error() {
        let v: Value = "hello".into();
        let result: Result<i64, ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "int");
        assert_eq!(err.found, "string");
    }

    #[test]
    fn test_cel_value_to_value() {
        use crate::types::CelValue;

        let cv = CelValue::String("test".to_string());
        let v: Value = cv.into();
        let s: &str = (&v).try_into().unwrap();
        assert_eq!(s, "test");

        let cv = CelValue::Int(42);
        let v: Value = cv.into();
        assert_eq!(v, Value::Int(42));

        let cv = CelValue::Null;
        let v: Value = cv.into();
        assert_eq!(v, Value::Null);
    }

    #[test]
    fn test_try_from_timestamp() {
        let v = Value::Timestamp(Timestamp::new(1234567890, 0));
        let t: Timestamp = (&v).try_into().unwrap();
        assert_eq!(t.seconds, 1234567890);

        // Owned conversion
        let v = Value::Timestamp(Timestamp::new(1234567890, 500));
        let t: Timestamp = v.try_into().unwrap();
        assert_eq!(t.nanos, 500);

        let v: Value = "not a timestamp".into();
        let result: Result<Timestamp, ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "timestamp");
        assert_eq!(err.found, "string");
    }

    #[test]
    fn test_try_from_duration() {
        let v = Value::Duration(Duration::new(60, 0));
        let d: Duration = (&v).try_into().unwrap();
        assert_eq!(d.seconds, 60);

        // Owned conversion
        let v = Value::Duration(Duration::new(120, 500));
        let d: Duration = v.try_into().unwrap();
        assert_eq!(d.seconds, 120);
        assert_eq!(d.nanos, 500);

        let v = Value::Int(42);
        let result: Result<Duration, ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "duration");
        assert_eq!(err.found, "int");
    }

    #[test]
    fn test_try_from_list() {
        let v: Value = vec![Value::Int(1), Value::Int(2)].into();
        let list: &[Value] = (&v).try_into().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], Value::Int(1));
        assert_eq!(list[1], Value::Int(2));

        let v: Value = "not a list".into();
        let result: Result<&[Value], ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "list");
        assert_eq!(err.found, "string");
    }

    #[test]
    fn test_try_from_map() {
        let v = Value::map([(MapKey::String("key".into()), Value::Int(42))]);
        let map: &ValueMap = (&v).try_into().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get(&MapKey::String("key".into())),
            Some(&Value::Int(42))
        );

        let v: Value = "not a map".into();
        let result: Result<&ValueMap, ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "map");
        assert_eq!(err.found, "string");
    }

    #[test]
    fn test_try_from_optional() {
        let v = Value::optional_some(Value::Int(42));
        let opt: &OptionalValue = (&v).try_into().unwrap();
        assert!(opt.is_present());
        assert_eq!(opt.as_value(), Some(&Value::Int(42)));

        let v = Value::optional_none();
        let opt: &OptionalValue = (&v).try_into().unwrap();
        assert!(!opt.is_present());

        let v = Value::Int(42);
        let result: Result<&OptionalValue, ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "optional");
        assert_eq!(err.found, "int");
    }

    #[test]
    fn test_try_from_eval_error() {
        let v = Value::error(EvalError::division_by_zero());
        let err: &EvalError = (&v).try_into().unwrap();
        assert_eq!(err.message, "division by zero");

        let v = Value::Int(42);
        let result: Result<&EvalError, ValueError> = (&v).try_into();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.expected, "error");
        assert_eq!(err.found, "int");
    }

    #[test]
    fn test_from_integer_widening() {
        // Signed integers widen to Value::Int
        let v: Value = 42i8.into();
        assert_eq!(v, Value::Int(42));

        let v: Value = 42i16.into();
        assert_eq!(v, Value::Int(42));

        let v: Value = 42i32.into();
        assert_eq!(v, Value::Int(42));

        let v: Value = (42isize).into();
        assert_eq!(v, Value::Int(42));

        // Unsigned integers widen to Value::UInt
        let v: Value = 42u8.into();
        assert_eq!(v, Value::UInt(42));

        let v: Value = 42u16.into();
        assert_eq!(v, Value::UInt(42));

        let v: Value = 42u32.into();
        assert_eq!(v, Value::UInt(42));

        let v: Value = (42usize).into();
        assert_eq!(v, Value::UInt(42));
    }

    #[test]
    fn test_map_key_from_i32_u32() {
        // From i32
        let key: MapKey = 42i32.into();
        assert_eq!(key, MapKey::Int(42));

        // From u32
        let key: MapKey = 42u32.into();
        assert_eq!(key, MapKey::UInt(42));
    }

    #[test]
    fn test_from_option() {
        // Some values convert to inner value
        let v: Value = Some(42).into();
        assert_eq!(v, Value::Int(42));

        let v: Value = Some("hello").into();
        assert_eq!(v, Value::String(Arc::from("hello")));

        // None converts to Null
        let v: Value = None::<i32>.into();
        assert_eq!(v, Value::Null);

        let v: Value = None::<String>.into();
        assert_eq!(v, Value::Null);

        // Nested options
        let v: Value = Some(Some(42)).into();
        assert_eq!(v, Value::Int(42));
    }

    #[test]
    fn test_from_option_in_map() {
        let email: Option<String> = None;
        let name: Option<&str> = Some("Alice");

        let user = Value::map([("email", Value::from(email)), ("name", Value::from(name))]);

        if let Value::Map(m) = user {
            assert_eq!(m.get(&MapKey::from("email")), Some(&Value::Null));
            assert_eq!(
                m.get(&MapKey::from("name")),
                Some(&Value::String(Arc::from("Alice")))
            );
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn test_enum_numeric_equality() {
        let enum_val = Value::Enum(EnumValue::new("test.MyEnum", 1));

        // Enum == Int
        assert_eq!(enum_val, Value::Int(1));
        assert_eq!(Value::Int(1), enum_val);
        assert_ne!(enum_val, Value::Int(2));

        // Enum == UInt
        assert_eq!(enum_val, Value::UInt(1));
        assert_eq!(Value::UInt(1), enum_val);

        // Negative enum value != UInt
        let neg_enum = Value::Enum(EnumValue::new("test.MyEnum", -1));
        assert_ne!(neg_enum, Value::UInt(1));

        // Enum == Double
        assert_eq!(enum_val, Value::Double(1.0));
        assert_eq!(Value::Double(1.0), enum_val);
        assert_ne!(enum_val, Value::Double(1.5));

        // NaN != Enum
        assert_ne!(Value::Double(f64::NAN), enum_val);

        // List membership: Int in [Enum] uses PartialEq
        let list = Value::List(Arc::from(vec![Value::Enum(EnumValue::new(
            "test.MyEnum",
            1,
        ))]));
        if let Value::List(items) = &list {
            assert!(items.contains(&Value::Int(1)));
        }
    }

    #[test]
    fn test_enum_numeric_comparison() {
        let enum_val = Value::Enum(EnumValue::new("test.MyEnum", 5));

        assert_eq!(enum_val.compare(&Value::Int(3)), Some(Ordering::Greater));
        assert_eq!(enum_val.compare(&Value::Int(5)), Some(Ordering::Equal));
        assert_eq!(enum_val.compare(&Value::Int(10)), Some(Ordering::Less));

        assert_eq!(enum_val.compare(&Value::UInt(3)), Some(Ordering::Greater));
        assert_eq!(enum_val.compare(&Value::Double(5.0)), Some(Ordering::Equal));

        // Negative enum vs UInt
        let neg_enum = Value::Enum(EnumValue::new("test.MyEnum", -1));
        assert_eq!(neg_enum.compare(&Value::UInt(0)), Some(Ordering::Less));
    }

    #[test]
    fn test_value_from_option_cel_optional() {
        // from_option preserves CEL optional semantics
        let v = Value::from_option(Some(42));
        assert!(matches!(v, Value::Optional(OptionalValue::Some(_))));

        let v = Value::from_option(None::<i32>);
        assert!(matches!(v, Value::Optional(OptionalValue::None)));
    }
}
