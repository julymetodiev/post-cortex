//! Trait abstraction for protobuf message values.
//!
//! `MessageValue` defines the interface for proto message values,
//! decoupling cel-core from any specific protobuf implementation.

use std::any::Any;
use std::fmt;

/// A trait representing a protobuf message value at runtime.
///
/// This abstraction allows cel-core to work with proto messages without
/// depending on a specific protobuf library (e.g., prost-reflect).
///
/// # Object Safety
///
/// This trait is object-safe and can be used with `dyn MessageValue`.
///
/// # Downcasting
///
/// The `as_any()` method enables downcasting to concrete types. Note that
/// downcasting only works with the exact concrete type - wrapped or newtype
/// implementations will not match. Equality comparisons via `eq_message()`
/// will return `false` for different concrete types even if they represent
/// the same message content.
///
/// # Map Keys
///
/// `MessageValue` does not support hashing, so proto messages cannot be used
/// as map keys in CEL. This matches the CEL specification.
pub trait MessageValue: fmt::Debug + Send + Sync {
    /// Get the fully qualified type name of this message.
    fn type_name(&self) -> &str;

    /// Check equality with another message value.
    fn eq_message(&self, other: &dyn MessageValue) -> bool;

    /// Downcast to a concrete type via `Any`.
    fn as_any(&self) -> &dyn Any;

    /// Clone this message value into a boxed trait object.
    fn clone_boxed(&self) -> Box<dyn MessageValue>;

    /// Returns true if this message is a "zero value" (all fields are default/unset).
    ///
    /// Used by optionals extension for `hasValue()` / `or()` semantics.
    /// Returns `None` if the implementation cannot determine default status.
    fn is_default(&self) -> Option<bool> {
        None // Unknown - implementations should override
    }
}

impl Clone for Box<dyn MessageValue> {
    fn clone(&self) -> Self {
        self.clone_boxed()
    }
}
