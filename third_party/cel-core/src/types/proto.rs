//! Proto type definitions shared across cel-core and cel-core-proto.
//!
//! This module provides type definitions that describe protobuf types
//! in the CEL type system, without any dependency on a specific protobuf runtime.

use crate::types::CelType;

/// Result of resolving a qualified proto name.
#[derive(Debug, Clone)]
pub enum ResolvedProtoType {
    /// Resolved to a message type.
    Message {
        /// Fully qualified message name.
        name: String,
        /// CEL type representation.
        cel_type: CelType,
    },
    /// Resolved to an enum type.
    Enum {
        /// Fully qualified enum name.
        name: String,
        /// CEL type representation.
        cel_type: CelType,
    },
    /// Resolved to an enum value.
    EnumValue {
        /// Fully qualified enum name.
        enum_name: String,
        /// Numeric value of the enum constant.
        value: i32,
    },
}

/// Convert a proto message type name to its CEL type representation.
///
/// This handles well-known types specially:
/// - `google.protobuf.Timestamp` -> `CelType::Timestamp`
/// - `google.protobuf.Duration` -> `CelType::Duration`
/// - Wrapper types -> `CelType::Wrapper(inner)`
/// - Other messages -> `CelType::Message(name)`
pub fn proto_message_to_cel_type(full_name: &str) -> CelType {
    match full_name {
        // Well-known types
        "google.protobuf.Timestamp" => CelType::Timestamp,
        "google.protobuf.Duration" => CelType::Duration,

        // Wrapper types
        "google.protobuf.BoolValue" => CelType::wrapper(CelType::Bool),
        "google.protobuf.Int32Value" | "google.protobuf.Int64Value" => {
            CelType::wrapper(CelType::Int)
        }
        "google.protobuf.UInt32Value" | "google.protobuf.UInt64Value" => {
            CelType::wrapper(CelType::UInt)
        }
        "google.protobuf.FloatValue" | "google.protobuf.DoubleValue" => {
            CelType::wrapper(CelType::Double)
        }
        "google.protobuf.StringValue" => CelType::wrapper(CelType::String),
        "google.protobuf.BytesValue" => CelType::wrapper(CelType::Bytes),

        // Any, Struct, Value, ListValue
        "google.protobuf.Any"
        | "google.protobuf.Struct"
        | "google.protobuf.Value"
        | "google.protobuf.ListValue" => CelType::Dyn,

        // Regular message types
        _ => CelType::message(full_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_well_known_type_mapping() {
        assert_eq!(
            proto_message_to_cel_type("google.protobuf.Timestamp"),
            CelType::Timestamp
        );
        assert_eq!(
            proto_message_to_cel_type("google.protobuf.Duration"),
            CelType::Duration
        );
        assert_eq!(
            proto_message_to_cel_type("google.protobuf.Int64Value"),
            CelType::wrapper(CelType::Int)
        );
    }

    #[test]
    fn test_regular_message_type() {
        assert_eq!(
            proto_message_to_cel_type("my.package.MyMessage"),
            CelType::message("my.package.MyMessage")
        );
    }
}
