//! Common types for CEL: type system, values, AST, and declarations.
//!
//! This module provides the foundational types shared across CEL components:
//!
//! - **Type System**: `CelType` for representing CEL types with parameterized
//!   types like `List<T>`, `Map<K, V>`, and `type(T)`.
//! - **Values**: `CelValue` for representing constant values.
//! - **AST**: Expression types (`Expr`, `SpannedExpr`, `BinaryOp`, etc.) for
//!   representing parsed CEL syntax.
//! - **Declarations**: `FunctionDecl`, `OverloadDecl`, `VariableDecl` for
//!   defining the type environment.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// AST module
mod ast;
pub use ast::{
    BinaryOp, ComprehensionData, Expr, ListElement, MapEntry, Span, Spanned, SpannedExpr,
    StructField, UnaryOp,
};

// Declarations module
mod decls;
pub use decls::{FunctionDecl, OverloadDecl, VariableDecl};

// Proto types module
pub mod proto;
pub use proto::{proto_message_to_cel_type, ResolvedProtoType};

// ==================== CelValue ====================

/// A CEL constant value.
///
/// This represents compile-time constant values that can appear in CEL expressions,
/// such as literals and enum constants.
#[derive(Debug, Clone, PartialEq)]
pub enum CelValue {
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
    /// Unicode string.
    String(String),
    /// Byte sequence.
    Bytes(Vec<u8>),
}

impl CelValue {
    /// Get the CEL type of this value.
    pub fn cel_type(&self) -> CelType {
        match self {
            CelValue::Null => CelType::Null,
            CelValue::Bool(_) => CelType::Bool,
            CelValue::Int(_) => CelType::Int,
            CelValue::UInt(_) => CelType::UInt,
            CelValue::Double(_) => CelType::Double,
            CelValue::String(_) => CelType::String,
            CelValue::Bytes(_) => CelType::Bytes,
        }
    }
}

impl fmt::Display for CelValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CelValue::Null => write!(f, "null"),
            CelValue::Bool(v) => write!(f, "{}", v),
            CelValue::Int(v) => write!(f, "{}", v),
            CelValue::UInt(v) => write!(f, "{}u", v),
            CelValue::Double(v) => write!(f, "{}", v),
            CelValue::String(v) => write!(f, "\"{}\"", v),
            CelValue::Bytes(v) => write!(f, "b\"{}\"", String::from_utf8_lossy(v)),
        }
    }
}

// ==================== CelType ====================

static TYPE_VAR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// CEL types that can be inferred from expressions.
///
/// This enum represents CEL's type system, including primitive types,
/// parameterized collections, and special types for type checking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CelType {
    // ==================== Primitives ====================
    /// Boolean type
    Bool,
    /// Signed 64-bit integer
    Int,
    /// Unsigned 64-bit integer
    UInt,
    /// 64-bit floating point
    Double,
    /// Unicode string
    String,
    /// Byte sequence
    Bytes,

    // ==================== Parameterized Collections ====================
    /// Homogeneous list with element type: `list<T>`
    List(Arc<CelType>),
    /// Key-value map with key and value types: `map<K, V>`
    Map(Arc<CelType>, Arc<CelType>),

    // ==================== Well-known Types ====================
    /// google.protobuf.Timestamp
    Timestamp,
    /// google.protobuf.Duration
    Duration,

    // ==================== Special Types ====================
    /// Null value
    Null,
    /// Dynamic/unknown type - compatible with any type
    Dyn,
    /// Type value: `type(T)` - represents the type of a type
    Type(Arc<CelType>),

    // ==================== Proto Types ====================
    /// Protobuf message type with fully qualified name
    Message(Arc<str>),
    /// Protobuf enum type with fully qualified name
    Enum(Arc<str>),
    /// Abstract, application-defined type with optional type parameters.
    /// Used for custom container types or domain-specific types.
    Abstract {
        name: Arc<str>,
        params: Arc<[CelType]>,
    },

    // ==================== Type Checking Types ====================
    /// Function type with parameter types and return type
    Function {
        params: Arc<[CelType]>,
        result: Arc<CelType>,
    },
    /// Type parameter for generic type checking (e.g., `T` in `list<T>`)
    TypeParam(Arc<str>),
    /// Type variable for inference - can be unified with concrete types.
    /// Used for empty collections where element type comes from context.
    TypeVar(u64),
    /// Wrapper type for proto wrapper types (e.g., `google.protobuf.Int64Value`)
    Wrapper(Arc<CelType>),
    /// Optional type: `optional<T>` - represents a value that may or may not be present.
    /// Used by the CEL optionals extension.
    Optional(Arc<CelType>),
    /// Error type - used when type inference fails
    Error,
}

// ==================== Constructors ====================

impl CelType {
    /// Create a list type with the given element type.
    ///
    /// # Example
    /// ```
    /// use cel_core::types::CelType;
    /// let list_of_int = CelType::list(CelType::Int);
    /// assert_eq!(list_of_int.display_name(), "list<int>");
    /// ```
    pub fn list(elem: CelType) -> Self {
        CelType::List(Arc::new(elem))
    }

    /// Create a map type with the given key and value types.
    ///
    /// # Example
    /// ```
    /// use cel_core::types::CelType;
    /// let map_str_int = CelType::map(CelType::String, CelType::Int);
    /// assert_eq!(map_str_int.display_name(), "map<string, int>");
    /// ```
    pub fn map(key: CelType, value: CelType) -> Self {
        CelType::Map(Arc::new(key), Arc::new(value))
    }

    /// Create a type value representing `type(T)`.
    ///
    /// # Example
    /// ```
    /// use cel_core::types::CelType;
    /// let type_of_int = CelType::type_of(CelType::Int);
    /// assert_eq!(type_of_int.display_name(), "type(int)");
    /// ```
    pub fn type_of(inner: CelType) -> Self {
        CelType::Type(Arc::new(inner))
    }

    /// Create a message type with the given fully qualified name.
    ///
    /// # Example
    /// ```
    /// use cel_core::types::CelType;
    /// let msg = CelType::message("google.protobuf.Timestamp");
    /// assert_eq!(msg.display_name(), "google.protobuf.Timestamp");
    /// ```
    pub fn message(name: &str) -> Self {
        CelType::Message(Arc::from(name))
    }

    /// Create an enum type with the given fully qualified name.
    pub fn enum_type(name: &str) -> Self {
        CelType::Enum(Arc::from(name))
    }

    /// Create an abstract type with the given name and parameter types.
    pub fn abstract_type(name: &str, params: &[CelType]) -> Self {
        CelType::Abstract {
            name: Arc::from(name),
            params: Arc::from(params),
        }
    }

    /// Create a function type with the given parameter and result types.
    ///
    /// # Example
    /// ```
    /// use cel_core::types::CelType;
    /// let func = CelType::function(&[CelType::String], CelType::Int);
    /// assert_eq!(func.display_name(), "(string) -> int");
    /// ```
    pub fn function(params: &[CelType], result: CelType) -> Self {
        CelType::Function {
            params: Arc::from(params),
            result: Arc::new(result),
        }
    }

    /// Create a type parameter with the given name.
    pub fn type_param(name: &str) -> Self {
        CelType::TypeParam(Arc::from(name))
    }

    /// Create a fresh type variable for inference.
    pub fn fresh_type_var() -> Self {
        CelType::TypeVar(TYPE_VAR_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Create a wrapper type.
    pub fn wrapper(inner: CelType) -> Self {
        CelType::Wrapper(Arc::new(inner))
    }

    /// Create an optional type: `optional<T>`.
    ///
    /// # Example
    /// ```
    /// use cel_core::types::CelType;
    /// let opt_int = CelType::optional(CelType::Int);
    /// assert_eq!(opt_int.display_name(), "optional<int>");
    /// ```
    pub fn optional(inner: CelType) -> Self {
        CelType::Optional(Arc::new(inner))
    }
}

// ==================== Type Properties ====================

impl CelType {
    /// Returns true if this is a primitive type.
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            CelType::Bool
                | CelType::Int
                | CelType::UInt
                | CelType::Double
                | CelType::String
                | CelType::Bytes
        )
    }

    /// Returns true if this is a collection type (list or map).
    pub fn is_collection(&self) -> bool {
        matches!(self, CelType::List(_) | CelType::Map(_, _))
    }

    /// Returns true if this is a numeric type (int, uint, or double).
    pub fn is_numeric(&self) -> bool {
        matches!(self, CelType::Int | CelType::UInt | CelType::Double)
    }

    /// Returns true if this type is assignable from another type.
    ///
    /// Type `a` is assignable from type `b` if a value of type `b` can be
    /// used where a value of type `a` is expected.
    ///
    /// # Rules
    /// - Any type is assignable from itself
    /// - `Dyn` is assignable from and to any type
    /// - `List<Dyn>` is assignable from `List<T>` for any `T`
    /// - `Map<Dyn, Dyn>` is assignable from `Map<K, V>` for any `K`, `V`
    /// - `Null` is assignable to wrapper types
    pub fn is_assignable_from(&self, other: &CelType) -> bool {
        // Same type is always assignable
        if self == other {
            return true;
        }

        // Dyn is compatible with everything
        if matches!(self, CelType::Dyn) || matches!(other, CelType::Dyn) {
            return true;
        }

        // Error type is compatible with everything (for error recovery)
        if matches!(self, CelType::Error) || matches!(other, CelType::Error) {
            return true;
        }

        // Type variables unify with anything (they're placeholders)
        if matches!(self, CelType::TypeVar(_)) || matches!(other, CelType::TypeVar(_)) {
            return true;
        }

        match (self, other) {
            // List<Dyn> accepts any List<T>
            (CelType::List(self_elem), CelType::List(other_elem)) => {
                self_elem.is_assignable_from(other_elem)
            }

            // Map<Dyn, Dyn> accepts any Map<K, V>
            (CelType::Map(self_key, self_val), CelType::Map(other_key, other_val)) => {
                self_key.is_assignable_from(other_key) && self_val.is_assignable_from(other_val)
            }

            // Null is assignable to wrapper, message, duration, timestamp, optional, abstract types
            (CelType::Wrapper(_), CelType::Null) => true,
            (CelType::Message(_), CelType::Null) => true,
            (CelType::Timestamp, CelType::Null) => true,
            (CelType::Duration, CelType::Null) => true,
            (CelType::Optional(_), CelType::Null) => true,
            (CelType::Abstract { .. }, CelType::Null) => true,
            // Wrapper accepts its underlying type (boxing)
            (CelType::Wrapper(inner), other) => inner.is_assignable_from(other),
            // Underlying type accepts Wrapper (unboxing)
            (other, CelType::Wrapper(inner)) => other.is_assignable_from(inner.as_ref()),

            // Enum types are assignable to/from Int (backward compat)
            (CelType::Int, CelType::Enum(_)) => true,
            (CelType::Enum(_), CelType::Int) => true,

            // Optional type assignability
            (CelType::Optional(self_inner), CelType::Optional(other_inner)) => {
                self_inner.is_assignable_from(other_inner)
            }

            // Type values need compatible inner types
            (CelType::Type(self_inner), CelType::Type(other_inner)) => {
                self_inner.is_assignable_from(other_inner)
            }

            _ => false,
        }
    }

    /// Attempt to unify this type with another, collecting type parameter bindings.
    ///
    /// Unlike `is_assignable_from`, this method properly handles type parameters
    /// by binding them to concrete types during unification.
    ///
    /// Returns `true` if unification succeeds, `false` if there's a conflict.
    pub fn unify_with_substitution(
        &self,
        other: &CelType,
        substitutions: &mut std::collections::HashMap<Arc<str>, CelType>,
    ) -> bool {
        match (self, other) {
            // Bind type parameter to concrete type
            (CelType::TypeParam(name), concrete) => {
                if let Some(bound) = substitutions.get(name) {
                    bound.is_assignable_from(concrete)
                } else {
                    substitutions.insert(name.clone(), concrete.clone());
                    true
                }
            }
            // Bind type parameter from the other side
            (concrete, CelType::TypeParam(name)) => {
                if let Some(bound) = substitutions.get(name) {
                    concrete.is_assignable_from(bound)
                } else {
                    substitutions.insert(name.clone(), concrete.clone());
                    true
                }
            }
            // Recurse into parameterized types
            (CelType::List(a), CelType::List(b)) => a.unify_with_substitution(b, substitutions),
            (CelType::Map(ak, av), CelType::Map(bk, bv)) => {
                ak.unify_with_substitution(bk, substitutions)
                    && av.unify_with_substitution(bv, substitutions)
            }
            (CelType::Optional(a), CelType::Optional(b)) => {
                a.unify_with_substitution(b, substitutions)
            }
            // Fall back to regular assignability for other cases
            _ => self.is_assignable_from(other),
        }
    }

    /// Get the element type of a list, or None if not a list.
    pub fn list_elem(&self) -> Option<&CelType> {
        match self {
            CelType::List(elem) => Some(elem),
            _ => None,
        }
    }

    /// Get the key and value types of a map, or None if not a map.
    pub fn map_types(&self) -> Option<(&CelType, &CelType)> {
        match self {
            CelType::Map(key, val) => Some((key, val)),
            _ => None,
        }
    }

    /// Get the inner type of a type value, or None if not a type.
    pub fn type_inner(&self) -> Option<&CelType> {
        match self {
            CelType::Type(inner) => Some(inner),
            _ => None,
        }
    }

    /// Get the message name if this is a message type.
    pub fn message_name(&self) -> Option<&str> {
        match self {
            CelType::Message(name) => Some(name),
            _ => None,
        }
    }

    /// Get the inner type of an optional, or None if not an optional.
    pub fn optional_inner(&self) -> Option<&CelType> {
        match self {
            CelType::Optional(inner) => Some(inner),
            _ => None,
        }
    }
}

// ==================== Display ====================

impl CelType {
    /// Returns the display name of this type as used in CEL.
    ///
    /// This is the canonical string representation of the type that would
    /// be shown in error messages or hover information.
    pub fn display_name(&self) -> String {
        match self {
            CelType::Bool => "bool".to_string(),
            CelType::Int => "int".to_string(),
            CelType::UInt => "uint".to_string(),
            CelType::Double => "double".to_string(),
            CelType::String => "string".to_string(),
            CelType::Bytes => "bytes".to_string(),
            CelType::List(elem) => format!("list<{}>", elem.display_name()),
            CelType::Map(key, val) => {
                format!("map<{}, {}>", key.display_name(), val.display_name())
            }
            CelType::Timestamp => "timestamp".to_string(),
            CelType::Duration => "duration".to_string(),
            CelType::Null => "null".to_string(),
            CelType::Dyn => "dyn".to_string(),
            CelType::Type(inner) => format!("type({})", inner.display_name()),
            CelType::Message(name) => name.to_string(),
            CelType::Enum(name) => name.to_string(),
            CelType::Abstract { name, params } => {
                if params.is_empty() {
                    name.to_string()
                } else {
                    let params_str: Vec<_> = params.iter().map(|p| p.display_name()).collect();
                    format!("{}<{}>", name, params_str.join(", "))
                }
            }
            CelType::Function { params, result } => {
                let params_str: Vec<_> = params.iter().map(|p| p.display_name()).collect();
                format!("({}) -> {}", params_str.join(", "), result.display_name())
            }
            CelType::TypeParam(name) => name.to_string(),
            CelType::TypeVar(id) => format!("?T{}", id),
            CelType::Wrapper(inner) => format!("wrapper<{}>", inner.display_name()),
            CelType::Optional(inner) => format!("optional<{}>", inner.display_name()),
            CelType::Error => "error".to_string(),
        }
    }

    /// Returns a simple type name without parameters.
    ///
    /// This is useful for backwards compatibility where only the base
    /// type name is needed (e.g., "list" instead of "list<int>").
    ///
    /// Note: For `Message` and `Enum` types, this returns "message" and "enum"
    /// respectively, NOT the actual type names. Use `display_name()` for that.
    pub fn as_str(&self) -> &'static str {
        match self {
            CelType::Bool => "bool",
            CelType::Int => "int",
            CelType::UInt => "uint",
            CelType::Double => "double",
            CelType::String => "string",
            CelType::Bytes => "bytes",
            CelType::List(_) => "list",
            CelType::Map(_, _) => "map",
            CelType::Timestamp => "timestamp",
            CelType::Duration => "duration",
            CelType::Null => "null",
            CelType::Dyn => "dyn",
            CelType::Type(_) => "type",
            CelType::Message(_) => "message",
            CelType::Enum(_) => "enum",
            CelType::Abstract { .. } => "abstract",
            CelType::Function { .. } => "function",
            CelType::TypeParam(_) => "type_param",
            CelType::TypeVar(_) => "type_var",
            CelType::Wrapper(_) => "wrapper",
            CelType::Optional(_) => "optional",
            CelType::Error => "error",
        }
    }
}

impl fmt::Display for CelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ==================== Unification ====================

/// Context for tracking type variable bindings during unification.
#[derive(Default)]
#[cfg(test)]
struct UnificationContext {
    bindings: std::collections::HashMap<u64, CelType>,
}

#[cfg(test)]
impl UnificationContext {
    /// Create a new unification context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the resolved type for a type variable, if bound.
    pub fn resolve(&self, var_id: u64) -> Option<&CelType> {
        self.bindings.get(&var_id)
    }

    /// Bind a type variable to a concrete type.
    pub fn bind(&mut self, var_id: u64, ty: CelType) {
        self.bindings.insert(var_id, ty);
    }

    /// Fully resolve a type, substituting all bound type variables.
    pub fn fully_resolve(&self, ty: &CelType) -> CelType {
        match ty {
            CelType::TypeVar(id) => self.bindings.get(id).cloned().unwrap_or_else(|| ty.clone()),
            CelType::List(elem) => CelType::list(self.fully_resolve(elem)),
            CelType::Map(k, v) => CelType::map(self.fully_resolve(k), self.fully_resolve(v)),
            CelType::Optional(inner) => CelType::optional(self.fully_resolve(inner)),
            _ => ty.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_types_display() {
        assert_eq!(CelType::Bool.display_name(), "bool");
        assert_eq!(CelType::Int.display_name(), "int");
        assert_eq!(CelType::UInt.display_name(), "uint");
        assert_eq!(CelType::Double.display_name(), "double");
        assert_eq!(CelType::String.display_name(), "string");
        assert_eq!(CelType::Bytes.display_name(), "bytes");
    }

    #[test]
    fn list_type_display() {
        let list_int = CelType::list(CelType::Int);
        assert_eq!(list_int.display_name(), "list<int>");

        let list_string = CelType::list(CelType::String);
        assert_eq!(list_string.display_name(), "list<string>");

        let list_dyn = CelType::list(CelType::Dyn);
        assert_eq!(list_dyn.display_name(), "list<dyn>");
    }

    #[test]
    fn map_type_display() {
        let map_str_int = CelType::map(CelType::String, CelType::Int);
        assert_eq!(map_str_int.display_name(), "map<string, int>");

        let map_int_bool = CelType::map(CelType::Int, CelType::Bool);
        assert_eq!(map_int_bool.display_name(), "map<int, bool>");
    }

    #[test]
    fn nested_type_display() {
        let nested = CelType::list(CelType::list(CelType::Int));
        assert_eq!(nested.display_name(), "list<list<int>>");

        let map_of_lists = CelType::map(CelType::String, CelType::list(CelType::Int));
        assert_eq!(map_of_lists.display_name(), "map<string, list<int>>");
    }

    #[test]
    fn type_of_display() {
        let type_of_int = CelType::type_of(CelType::Int);
        assert_eq!(type_of_int.display_name(), "type(int)");
    }

    #[test]
    fn function_type_display() {
        let func = CelType::function(&[CelType::String], CelType::Int);
        assert_eq!(func.display_name(), "(string) -> int");

        let func_multi = CelType::function(&[CelType::String, CelType::Int], CelType::Bool);
        assert_eq!(func_multi.display_name(), "(string, int) -> bool");
    }

    #[test]
    fn is_primitive() {
        assert!(CelType::Bool.is_primitive());
        assert!(CelType::Int.is_primitive());
        assert!(CelType::String.is_primitive());
        assert!(!CelType::list(CelType::Int).is_primitive());
        assert!(!CelType::Timestamp.is_primitive());
    }

    #[test]
    fn is_collection() {
        assert!(CelType::list(CelType::Int).is_collection());
        assert!(CelType::map(CelType::String, CelType::Int).is_collection());
        assert!(!CelType::Int.is_collection());
    }

    #[test]
    fn is_numeric() {
        assert!(CelType::Int.is_numeric());
        assert!(CelType::UInt.is_numeric());
        assert!(CelType::Double.is_numeric());
        assert!(!CelType::String.is_numeric());
        assert!(!CelType::Bool.is_numeric());
    }

    #[test]
    fn assignability_same_type() {
        assert!(CelType::Int.is_assignable_from(&CelType::Int));
        assert!(CelType::String.is_assignable_from(&CelType::String));
    }

    #[test]
    fn assignability_dyn() {
        assert!(CelType::Dyn.is_assignable_from(&CelType::Int));
        assert!(CelType::Int.is_assignable_from(&CelType::Dyn));
        assert!(CelType::list(CelType::Int).is_assignable_from(&CelType::Dyn));
    }

    #[test]
    fn assignability_list() {
        let list_int = CelType::list(CelType::Int);
        let list_dyn = CelType::list(CelType::Dyn);

        assert!(list_dyn.is_assignable_from(&list_int));
        assert!(list_int.is_assignable_from(&list_dyn));
    }

    #[test]
    fn assignability_map() {
        let map_str_int = CelType::map(CelType::String, CelType::Int);
        let map_dyn_dyn = CelType::map(CelType::Dyn, CelType::Dyn);

        assert!(map_dyn_dyn.is_assignable_from(&map_str_int));
        assert!(map_str_int.is_assignable_from(&map_dyn_dyn));
    }

    #[test]
    fn list_elem() {
        let list_int = CelType::list(CelType::Int);
        assert_eq!(list_int.list_elem(), Some(&CelType::Int));
        assert_eq!(CelType::Int.list_elem(), None);
    }

    #[test]
    fn map_types() {
        let map_str_int = CelType::map(CelType::String, CelType::Int);
        assert_eq!(
            map_str_int.map_types(),
            Some((&CelType::String, &CelType::Int))
        );
        assert_eq!(CelType::Int.map_types(), None);
    }

    #[test]
    fn as_str_backwards_compat() {
        assert_eq!(CelType::Bool.as_str(), "bool");
        assert_eq!(CelType::list(CelType::Int).as_str(), "list");
        assert_eq!(CelType::map(CelType::String, CelType::Int).as_str(), "map");
    }

    #[test]
    fn equality() {
        let list1 = CelType::list(CelType::Int);
        let list2 = CelType::list(CelType::Int);
        let list3 = CelType::list(CelType::String);

        assert_eq!(list1, list2);
        assert_ne!(list1, list3);
    }

    #[test]
    fn hash_consistency() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(CelType::list(CelType::Int));
        set.insert(CelType::list(CelType::Int));

        assert_eq!(set.len(), 1);
    }

    #[test]
    fn type_param_does_not_match_arbitrary_types() {
        let param = CelType::type_param("T");
        // TypeParam should NOT be assignable from/to concrete types
        assert!(!param.is_assignable_from(&CelType::Int));
        assert!(!CelType::Int.is_assignable_from(&param));
    }

    #[test]
    fn type_param_unification_works() {
        use std::collections::HashMap;
        let mut subs = HashMap::new();
        let param = CelType::type_param("T");

        // First binding succeeds
        assert!(param.unify_with_substitution(&CelType::Int, &mut subs));
        assert_eq!(subs.get(&Arc::from("T")), Some(&CelType::Int));

        // Same type unifies again
        assert!(param.unify_with_substitution(&CelType::Int, &mut subs));

        // Different type fails
        assert!(!param.unify_with_substitution(&CelType::String, &mut subs));
    }

    #[test]
    fn type_param_unification_in_list() {
        use std::collections::HashMap;
        let mut subs = HashMap::new();
        let list_param = CelType::list(CelType::type_param("T"));
        let list_int = CelType::list(CelType::Int);

        assert!(list_param.unify_with_substitution(&list_int, &mut subs));
        assert_eq!(subs.get(&Arc::from("T")), Some(&CelType::Int));
    }

    #[test]
    fn wrapper_bidirectional_assignability() {
        let wrapper_int = CelType::wrapper(CelType::Int);

        // Null -> Wrapper (existing)
        assert!(wrapper_int.is_assignable_from(&CelType::Null));
        // Int -> Wrapper<Int> (boxing)
        assert!(wrapper_int.is_assignable_from(&CelType::Int));
        // Wrapper<Int> -> Int (unboxing)
        assert!(CelType::Int.is_assignable_from(&wrapper_int));
        // Wrapper<Int> should NOT be assignable from String
        assert!(!wrapper_int.is_assignable_from(&CelType::String));
    }

    #[test]
    fn type_var_assignable_from_anything() {
        let tv = CelType::fresh_type_var();
        // TypeVar should be assignable from/to any type
        assert!(tv.is_assignable_from(&CelType::Int));
        assert!(tv.is_assignable_from(&CelType::String));
        assert!(CelType::Int.is_assignable_from(&tv));
        assert!(CelType::list(CelType::Int).is_assignable_from(&tv));
    }

    #[test]
    fn type_var_display() {
        let tv1 = CelType::fresh_type_var();
        let tv2 = CelType::fresh_type_var();
        // Each TypeVar should have a unique display name
        assert!(tv1.display_name().starts_with("?T"));
        assert!(tv2.display_name().starts_with("?T"));
        assert_ne!(tv1.display_name(), tv2.display_name());
    }

    #[test]
    fn unification_context_binds_and_resolves() {
        let mut ctx = UnificationContext::new();
        let tv = CelType::fresh_type_var();
        let CelType::TypeVar(var_id) = tv else {
            panic!("expected TypeVar")
        };

        // Initially unbound
        assert!(ctx.resolve(var_id).is_none());

        // Bind it
        ctx.bind(var_id, CelType::Int);
        assert_eq!(ctx.resolve(var_id), Some(&CelType::Int));

        // fully_resolve substitutes the binding
        let resolved = ctx.fully_resolve(&tv);
        assert_eq!(resolved, CelType::Int);
    }

    #[test]
    fn unification_context_resolves_nested() {
        let mut ctx = UnificationContext::new();
        let tv = CelType::fresh_type_var();
        let CelType::TypeVar(var_id) = tv.clone() else {
            panic!("expected TypeVar")
        };

        ctx.bind(var_id, CelType::String);

        let list_tv = CelType::list(tv);
        let resolved = ctx.fully_resolve(&list_tv);
        assert_eq!(resolved, CelType::list(CelType::String));
    }

    #[test]
    fn abstract_type_display() {
        let abs = CelType::abstract_type("custom.Container", &[CelType::Int, CelType::String]);
        assert_eq!(abs.display_name(), "custom.Container<int, string>");

        let abs_no_params = CelType::abstract_type("custom.Type", &[]);
        assert_eq!(abs_no_params.display_name(), "custom.Type");
    }
}
