//! Trait abstractions for protobuf type registries.
//!
//! This module defines two focused traits for working with protobuf types:
//!
//! - `ProtoTypeResolver`: Provides type lookup capabilities used by the checker
//! - `ProtoRegistry`: Extends ProtoTypeResolver with runtime operations for the evaluator
//!
//! These traits decouple cel-core from any specific protobuf implementation.

use std::any::Any;
use std::fmt;

use super::message::MessageValue;
use super::Value;
use crate::types::{CelType, ResolvedProtoType};

/// An evaluated struct field ready for message construction.
#[derive(Debug, Clone)]
pub struct StructFieldValue {
    /// The field name.
    pub name: String,
    /// The evaluated value.
    pub value: Value,
    /// Whether this is an optional field entry.
    pub optional: bool,
}

/// Trait for protobuf descriptor pool access (checker operations).
///
/// This trait provides type information needed during type checking,
/// without requiring the ability to construct or manipulate proto messages.
///
/// # Object Safety
///
/// This trait is object-safe and can be used with `dyn ProtoTypeResolver`.
///
/// # Downcasting
///
/// The `as_any()` method enables downcasting to concrete types. Note that
/// downcasting only works with the exact concrete type - wrapped or newtype
/// implementations will not match.
pub trait ProtoTypeResolver: fmt::Debug + Send + Sync {
    /// Get the CEL type of a message field.
    fn get_field_type(&self, message: &str, field: &str) -> Option<CelType>;

    /// Check whether a message type is known to this registry.
    fn has_message(&self, message: &str) -> bool;

    /// Check whether `ext_name` is a known extension field on `message`.
    fn is_extension(&self, message: &str, ext_name: &str) -> bool;

    /// Get the numeric value of an enum constant by name.
    fn get_enum_value(&self, enum_name: &str, value_name: &str) -> Option<i32>;

    /// Resolve a qualified name (dot-separated parts) within a container namespace.
    ///
    /// Returns the resolved proto type (Message, Enum, or EnumValue).
    fn resolve_qualified(&self, parts: &[&str], container: &str) -> Option<ResolvedProtoType>;

    /// Resolve a message name within a container namespace using C++ namespace rules.
    ///
    /// Returns the fully qualified message name if found.
    fn resolve_message_name(&self, name: &str, container: &str) -> Option<String>;

    /// List all field names of a message type.
    ///
    /// Returns `None` if the message type is unknown to this registry.
    /// Default implementation returns `None`.
    fn message_field_names(&self, _message: &str) -> Option<Vec<String>> {
        None
    }

    /// Downcast to a concrete type via `Any`.
    fn as_any(&self) -> &dyn Any;
}

/// Trait for protobuf runtime operations (evaluator operations).
///
/// This trait extends `ProtoTypeResolver` with the ability to construct messages,
/// access fields at runtime, and work with extensions. It is used by the evaluator.
///
/// # Object Safety
///
/// This trait is object-safe and can be used with `dyn ProtoRegistry`.
pub trait ProtoRegistry: ProtoTypeResolver {
    /// Construct a protobuf message from evaluated field values.
    ///
    /// Returns the constructed message as a Value (may be unwrapped to a
    /// native CEL value for well-known types like Timestamp, Duration, etc.).
    fn construct_message(
        &self,
        type_name: &str,
        fields: &[StructFieldValue],
        strong_enums: bool,
    ) -> Value;

    /// Access a field on a message value.
    ///
    /// Returns the field value, or an error/optional-none for missing fields.
    fn message_field_access(
        &self,
        msg: &dyn MessageValue,
        field: &str,
        optional: bool,
        strong_enums: bool,
    ) -> Value;

    /// Test whether a message has a field set (for `has()` macro).
    fn message_has_field(&self, msg: &dyn MessageValue, field: &str) -> Value;

    /// Get an extension field value from a message.
    ///
    /// Returns `None` if the extension is not applicable to this message type.
    fn get_extension_value(
        &self,
        msg: &dyn MessageValue,
        ext_name: &str,
        optional: bool,
        strong_enums: bool,
    ) -> Option<Value>;

    /// Test whether a message has an extension field set.
    ///
    /// Returns `None` if the extension is not applicable to this message type.
    fn has_extension(&self, msg: &dyn MessageValue, ext_name: &str) -> Option<bool>;
}
