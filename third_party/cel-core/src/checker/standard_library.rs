//! CEL standard library function declarations.
//!
//! This module defines all CEL operators and built-in functions with their
//! type signatures and overload IDs matching cel-go conventions.

use std::sync::LazyLock;

use crate::types::{CelType, FunctionDecl, OverloadDecl};

/// The CEL standard library containing all built-in operators and functions.
pub static STANDARD_LIBRARY: LazyLock<Vec<FunctionDecl>> = LazyLock::new(build_standard_library);

fn build_standard_library() -> Vec<FunctionDecl> {
    let mut funcs = vec![
        // ==================== Operators ====================

        // Arithmetic: _+_
        FunctionDecl::new("_+_")
            .with_overload(OverloadDecl::function(
                "add_int64_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "add_uint64_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "add_double_double",
                vec![CelType::Double, CelType::Double],
                CelType::Double,
            ))
            .with_overload(OverloadDecl::function(
                "add_string_string",
                vec![CelType::String, CelType::String],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "add_bytes_bytes",
                vec![CelType::Bytes, CelType::Bytes],
                CelType::Bytes,
            ))
            .with_overload(
                OverloadDecl::function(
                    "add_list_list",
                    vec![
                        CelType::list(CelType::type_param("T")),
                        CelType::list(CelType::type_param("T")),
                    ],
                    CelType::list(CelType::type_param("T")),
                )
                .with_type_params(vec!["T".to_string()]),
            )
            .with_overload(OverloadDecl::function(
                "add_timestamp_duration",
                vec![CelType::Timestamp, CelType::Duration],
                CelType::Timestamp,
            ))
            .with_overload(OverloadDecl::function(
                "add_duration_timestamp",
                vec![CelType::Duration, CelType::Timestamp],
                CelType::Timestamp,
            ))
            .with_overload(OverloadDecl::function(
                "add_duration_duration",
                vec![CelType::Duration, CelType::Duration],
                CelType::Duration,
            ))
            .with_overload(
                OverloadDecl::function(
                    "add_map_map",
                    vec![
                        CelType::map(CelType::type_param("K"), CelType::type_param("V")),
                        CelType::map(CelType::type_param("K"), CelType::type_param("V")),
                    ],
                    CelType::map(CelType::type_param("K"), CelType::type_param("V")),
                )
                .with_type_params(vec!["K".to_string(), "V".to_string()]),
            ),
        // Arithmetic: _-_
        FunctionDecl::new("_-_")
            .with_overload(OverloadDecl::function(
                "subtract_int64_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "subtract_uint64_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "subtract_double_double",
                vec![CelType::Double, CelType::Double],
                CelType::Double,
            ))
            .with_overload(OverloadDecl::function(
                "subtract_timestamp_timestamp",
                vec![CelType::Timestamp, CelType::Timestamp],
                CelType::Duration,
            ))
            .with_overload(OverloadDecl::function(
                "subtract_timestamp_duration",
                vec![CelType::Timestamp, CelType::Duration],
                CelType::Timestamp,
            ))
            .with_overload(OverloadDecl::function(
                "subtract_duration_duration",
                vec![CelType::Duration, CelType::Duration],
                CelType::Duration,
            )),
        // Arithmetic: _*_
        FunctionDecl::new("_*_")
            .with_overload(OverloadDecl::function(
                "multiply_int64_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "multiply_uint64_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "multiply_double_double",
                vec![CelType::Double, CelType::Double],
                CelType::Double,
            )),
        // Arithmetic: _/_
        FunctionDecl::new("_/_")
            .with_overload(OverloadDecl::function(
                "divide_int64_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "divide_uint64_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "divide_double_double",
                vec![CelType::Double, CelType::Double],
                CelType::Double,
            )),
        // Arithmetic: _%_
        FunctionDecl::new("_%_")
            .with_overload(OverloadDecl::function(
                "modulo_int64_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "modulo_uint64_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::UInt,
            )),
        // Unary: -_
        FunctionDecl::new("-_")
            .with_overload(OverloadDecl::function(
                "negate_int64",
                vec![CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "negate_double",
                vec![CelType::Double],
                CelType::Double,
            )),
        // Comparison: _==_
        FunctionDecl::new("_==_").with_overload(
            OverloadDecl::function(
                "equals",
                vec![CelType::type_param("T"), CelType::type_param("T")],
                CelType::Bool,
            )
            .with_type_params(vec!["T".to_string()]),
        ),
        // Comparison: _!=_
        FunctionDecl::new("_!=_").with_overload(
            OverloadDecl::function(
                "not_equals",
                vec![CelType::type_param("T"), CelType::type_param("T")],
                CelType::Bool,
            )
            .with_type_params(vec!["T".to_string()]),
        ),
        // Comparison: _<_
        FunctionDecl::new("_<_")
            .with_overload(OverloadDecl::function(
                "less_bool",
                vec![CelType::Bool, CelType::Bool],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_double",
                vec![CelType::Double, CelType::Double],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_string",
                vec![CelType::String, CelType::String],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_bytes",
                vec![CelType::Bytes, CelType::Bytes],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_timestamp",
                vec![CelType::Timestamp, CelType::Timestamp],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_duration",
                vec![CelType::Duration, CelType::Duration],
                CelType::Bool,
            )),
        // Comparison: _<=_
        FunctionDecl::new("_<=_")
            .with_overload(OverloadDecl::function(
                "less_equals_bool",
                vec![CelType::Bool, CelType::Bool],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_double",
                vec![CelType::Double, CelType::Double],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_string",
                vec![CelType::String, CelType::String],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_bytes",
                vec![CelType::Bytes, CelType::Bytes],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_timestamp",
                vec![CelType::Timestamp, CelType::Timestamp],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "less_equals_duration",
                vec![CelType::Duration, CelType::Duration],
                CelType::Bool,
            )),
        // Comparison: _>_
        FunctionDecl::new("_>_")
            .with_overload(OverloadDecl::function(
                "greater_bool",
                vec![CelType::Bool, CelType::Bool],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_double",
                vec![CelType::Double, CelType::Double],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_string",
                vec![CelType::String, CelType::String],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_bytes",
                vec![CelType::Bytes, CelType::Bytes],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_timestamp",
                vec![CelType::Timestamp, CelType::Timestamp],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_duration",
                vec![CelType::Duration, CelType::Duration],
                CelType::Bool,
            )),
        // Comparison: _>=_
        FunctionDecl::new("_>=_")
            .with_overload(OverloadDecl::function(
                "greater_equals_bool",
                vec![CelType::Bool, CelType::Bool],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_uint64",
                vec![CelType::UInt, CelType::UInt],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_double",
                vec![CelType::Double, CelType::Double],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_string",
                vec![CelType::String, CelType::String],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_bytes",
                vec![CelType::Bytes, CelType::Bytes],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_timestamp",
                vec![CelType::Timestamp, CelType::Timestamp],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "greater_equals_duration",
                vec![CelType::Duration, CelType::Duration],
                CelType::Bool,
            )),
        // Logical: _&&_
        FunctionDecl::new("_&&_").with_overload(OverloadDecl::function(
            "logical_and",
            vec![CelType::Bool, CelType::Bool],
            CelType::Bool,
        )),
        // Logical: _||_
        FunctionDecl::new("_||_").with_overload(OverloadDecl::function(
            "logical_or",
            vec![CelType::Bool, CelType::Bool],
            CelType::Bool,
        )),
        // Logical: !_
        FunctionDecl::new("!_").with_overload(OverloadDecl::function(
            "logical_not",
            vec![CelType::Bool],
            CelType::Bool,
        )),
        // Ternary: _?_:_
        FunctionDecl::new("_?_:_").with_overload(
            OverloadDecl::function(
                "conditional",
                vec![
                    CelType::Bool,
                    CelType::type_param("T"),
                    CelType::type_param("T"),
                ],
                CelType::type_param("T"),
            )
            .with_type_params(vec!["T".to_string()]),
        ),
        // Membership: _in_
        FunctionDecl::new("@in")
            .with_overload(
                OverloadDecl::function(
                    "in_list",
                    vec![
                        CelType::type_param("T"),
                        CelType::list(CelType::type_param("T")),
                    ],
                    CelType::Bool,
                )
                .with_type_params(vec!["T".to_string()]),
            )
            .with_overload(
                OverloadDecl::function(
                    "in_map",
                    vec![
                        CelType::type_param("K"),
                        CelType::map(CelType::type_param("K"), CelType::type_param("V")),
                    ],
                    CelType::Bool,
                )
                .with_type_params(vec!["K".to_string(), "V".to_string()]),
            ),
        // Index: _[_]
        FunctionDecl::new("_[_]")
            .with_overload(
                OverloadDecl::function(
                    "index_list",
                    vec![CelType::list(CelType::type_param("T")), CelType::Int],
                    CelType::type_param("T"),
                )
                .with_type_params(vec!["T".to_string()]),
            )
            .with_overload(
                OverloadDecl::function(
                    "index_map",
                    vec![
                        CelType::map(CelType::type_param("K"), CelType::type_param("V")),
                        CelType::type_param("K"),
                    ],
                    CelType::type_param("V"),
                )
                .with_type_params(vec!["K".to_string(), "V".to_string()]),
            ),
        // ==================== Type Conversions ====================
        FunctionDecl::new("bool")
            .with_overload(OverloadDecl::function(
                "bool_to_bool",
                vec![CelType::Bool],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_bool",
                vec![CelType::String],
                CelType::Bool,
            )),
        FunctionDecl::new("bytes")
            .with_overload(OverloadDecl::function(
                "bytes_to_bytes",
                vec![CelType::Bytes],
                CelType::Bytes,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_bytes",
                vec![CelType::String],
                CelType::Bytes,
            )),
        FunctionDecl::new("double")
            .with_overload(OverloadDecl::function(
                "double_to_double",
                vec![CelType::Double],
                CelType::Double,
            ))
            .with_overload(OverloadDecl::function(
                "int_to_double",
                vec![CelType::Int],
                CelType::Double,
            ))
            .with_overload(OverloadDecl::function(
                "uint_to_double",
                vec![CelType::UInt],
                CelType::Double,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_double",
                vec![CelType::String],
                CelType::Double,
            )),
        FunctionDecl::new("duration")
            .with_overload(OverloadDecl::function(
                "duration_to_duration",
                vec![CelType::Duration],
                CelType::Duration,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_duration",
                vec![CelType::String],
                CelType::Duration,
            )),
        FunctionDecl::new("dyn").with_overload(
            OverloadDecl::function("to_dyn", vec![CelType::type_param("T")], CelType::Dyn)
                .with_type_params(vec!["T".to_string()]),
        ),
        FunctionDecl::new("int")
            .with_overload(OverloadDecl::function(
                "int_to_int",
                vec![CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "uint_to_int",
                vec![CelType::UInt],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "double_to_int",
                vec![CelType::Double],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_int",
                vec![CelType::String],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "timestamp_to_int",
                vec![CelType::Timestamp],
                CelType::Int,
            )),
        FunctionDecl::new("string")
            .with_overload(OverloadDecl::function(
                "string_to_string",
                vec![CelType::String],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "bool_to_string",
                vec![CelType::Bool],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "int_to_string",
                vec![CelType::Int],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "uint_to_string",
                vec![CelType::UInt],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "double_to_string",
                vec![CelType::Double],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "bytes_to_string",
                vec![CelType::Bytes],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "timestamp_to_string",
                vec![CelType::Timestamp],
                CelType::String,
            ))
            .with_overload(OverloadDecl::function(
                "duration_to_string",
                vec![CelType::Duration],
                CelType::String,
            )),
        FunctionDecl::new("timestamp")
            .with_overload(OverloadDecl::function(
                "timestamp_to_timestamp",
                vec![CelType::Timestamp],
                CelType::Timestamp,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_timestamp",
                vec![CelType::String],
                CelType::Timestamp,
            ))
            .with_overload(OverloadDecl::function(
                "int_to_timestamp",
                vec![CelType::Int],
                CelType::Timestamp,
            )),
        FunctionDecl::new("type").with_overload(
            OverloadDecl::function(
                "type",
                vec![CelType::type_param("T")],
                CelType::type_of(CelType::type_param("T")),
            )
            .with_type_params(vec!["T".to_string()]),
        ),
        FunctionDecl::new("uint")
            .with_overload(OverloadDecl::function(
                "uint_to_uint",
                vec![CelType::UInt],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "int_to_uint",
                vec![CelType::Int],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "double_to_uint",
                vec![CelType::Double],
                CelType::UInt,
            ))
            .with_overload(OverloadDecl::function(
                "string_to_uint",
                vec![CelType::String],
                CelType::UInt,
            )),
        // ==================== Size ====================
        FunctionDecl::new("size")
            .with_overload(OverloadDecl::function(
                "size_string",
                vec![CelType::String],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "size_bytes",
                vec![CelType::Bytes],
                CelType::Int,
            ))
            .with_overload(
                OverloadDecl::function(
                    "size_list",
                    vec![CelType::list(CelType::type_param("T"))],
                    CelType::Int,
                )
                .with_type_params(vec!["T".to_string()]),
            )
            .with_overload(
                OverloadDecl::function(
                    "size_map",
                    vec![CelType::map(
                        CelType::type_param("K"),
                        CelType::type_param("V"),
                    )],
                    CelType::Int,
                )
                .with_type_params(vec!["K".to_string(), "V".to_string()]),
            )
            // Method overloads
            .with_overload(OverloadDecl::method(
                "string_size",
                vec![CelType::String],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::method(
                "bytes_size",
                vec![CelType::Bytes],
                CelType::Int,
            ))
            .with_overload(
                OverloadDecl::method(
                    "list_size",
                    vec![CelType::list(CelType::type_param("T"))],
                    CelType::Int,
                )
                .with_type_params(vec!["T".to_string()]),
            )
            .with_overload(
                OverloadDecl::method(
                    "map_size",
                    vec![CelType::map(
                        CelType::type_param("K"),
                        CelType::type_param("V"),
                    )],
                    CelType::Int,
                )
                .with_type_params(vec!["K".to_string(), "V".to_string()]),
            ),
        // ==================== String Functions ====================
        FunctionDecl::new("contains").with_overload(OverloadDecl::method(
            "string_contains_string",
            vec![CelType::String, CelType::String],
            CelType::Bool,
        )),
        FunctionDecl::new("endsWith").with_overload(OverloadDecl::method(
            "string_ends_with_string",
            vec![CelType::String, CelType::String],
            CelType::Bool,
        )),
        FunctionDecl::new("startsWith").with_overload(OverloadDecl::method(
            "string_starts_with_string",
            vec![CelType::String, CelType::String],
            CelType::Bool,
        )),
        FunctionDecl::new("matches")
            .with_overload(OverloadDecl::function(
                "matches_string_re",
                vec![CelType::String, CelType::String],
                CelType::Bool,
            ))
            .with_overload(OverloadDecl::method(
                "string_matches_re",
                vec![CelType::String, CelType::String],
                CelType::Bool,
            )),
    ];

    // ==================== Timestamp/Duration Accessors ====================

    let timestamp_accessors = [
        "getDate",
        "getDayOfMonth",
        "getDayOfWeek",
        "getDayOfYear",
        "getFullYear",
        "getMonth",
    ];

    for name in timestamp_accessors {
        let base_id = name.to_lowercase().replace("get", "").to_lowercase();
        funcs.push(
            FunctionDecl::new(name)
                .with_overload(OverloadDecl::method(
                    format!("timestamp_to_{}", base_id),
                    vec![CelType::Timestamp],
                    CelType::Int,
                ))
                .with_overload(OverloadDecl::method(
                    format!("timestamp_to_{}_with_tz", base_id),
                    vec![CelType::Timestamp, CelType::String],
                    CelType::Int,
                )),
        );
    }

    // Accessors for both timestamp and duration
    let time_accessors = ["getHours", "getMinutes", "getSeconds", "getMilliseconds"];

    for name in time_accessors {
        let base_id = name.to_lowercase().replace("get", "").to_lowercase();
        funcs.push(
            FunctionDecl::new(name)
                .with_overload(OverloadDecl::method(
                    format!("timestamp_to_{}", base_id),
                    vec![CelType::Timestamp],
                    CelType::Int,
                ))
                .with_overload(OverloadDecl::method(
                    format!("timestamp_to_{}_with_tz", base_id),
                    vec![CelType::Timestamp, CelType::String],
                    CelType::Int,
                ))
                .with_overload(OverloadDecl::method(
                    format!("duration_to_{}", base_id),
                    vec![CelType::Duration],
                    CelType::Int,
                )),
        );
    }

    funcs
}

/// Get a function declaration by name from the standard library.
#[allow(dead_code)]
pub fn get_standard_function(name: &str) -> Option<&'static FunctionDecl> {
    STANDARD_LIBRARY.iter().find(|f| f.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_library_loads() {
        assert!(!STANDARD_LIBRARY.is_empty());
    }

    #[test]
    fn test_add_operator() {
        let add = get_standard_function("_+_").unwrap();
        assert!(add.overloads.len() >= 4);
        assert!(add.overloads.iter().any(|o| o.id == "add_int64_int64"));
        assert!(add.overloads.iter().any(|o| o.id == "add_string_string"));
    }

    #[test]
    fn test_comparison_operators() {
        assert!(get_standard_function("_==_").is_some());
        assert!(get_standard_function("_!=_").is_some());
        assert!(get_standard_function("_<_").is_some());
        assert!(get_standard_function("_<=_").is_some());
        assert!(get_standard_function("_>_").is_some());
        assert!(get_standard_function("_>=_").is_some());
    }

    #[test]
    fn test_logical_operators() {
        assert!(get_standard_function("_&&_").is_some());
        assert!(get_standard_function("_||_").is_some());
        assert!(get_standard_function("!_").is_some());
    }

    #[test]
    fn test_size_has_both_standalone_and_method() {
        let size = get_standard_function("size").unwrap();
        assert!(size.has_standalone_overloads());
        assert!(size.has_member_overloads());
    }

    #[test]
    fn test_contains_is_method_only() {
        let contains = get_standard_function("contains").unwrap();
        assert!(!contains.has_standalone_overloads());
        assert!(contains.has_member_overloads());
    }

    #[test]
    fn test_type_conversions() {
        assert!(get_standard_function("bool").is_some());
        assert!(get_standard_function("int").is_some());
        assert!(get_standard_function("uint").is_some());
        assert!(get_standard_function("double").is_some());
        assert!(get_standard_function("string").is_some());
        assert!(get_standard_function("bytes").is_some());
        assert!(get_standard_function("timestamp").is_some());
        assert!(get_standard_function("duration").is_some());
        assert!(get_standard_function("dyn").is_some());
        assert!(get_standard_function("type").is_some());
    }

    #[test]
    fn test_timestamp_accessors() {
        assert!(get_standard_function("getDate").is_some());
        assert!(get_standard_function("getHours").is_some());
        assert!(get_standard_function("getMinutes").is_some());
    }
}
