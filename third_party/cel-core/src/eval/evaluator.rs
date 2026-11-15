//! Tree-walking evaluator for CEL expressions.
//!
//! The evaluator performs depth-first traversal of the AST, evaluating
//! each node and returning a `Value`. It supports:
//!
//! - Arithmetic, comparison, and logical operators
//! - Short-circuit evaluation for `&&`, `||`, and ternary
//! - Function calls via the function registry
//! - Comprehension evaluation for macros like `all`, `exists`, `map`, `filter`
//! - Error propagation (errors are values in CEL)
//! - Proto message construction and field access

use std::collections::HashMap;
use std::sync::Arc;

use super::proto_registry::ProtoRegistry;
use super::{
    Activation, Budget, EvalError, FunctionRegistry, HierarchicalActivation, MapKey, OptionalValue, Value,
    ValueMap,
};
use super::References;
use crate::types::{ComprehensionData, Expr, SpannedExpr};

/// The CEL expression evaluator.
///
/// Evaluates a CEL AST against an activation (variable bindings) and
/// function registry. Optionally supports proto message construction
/// when configured with a proto type registry.
pub struct Evaluator<'a> {
    pub(super) activation: &'a dyn Activation,
    /// Root activation (the outermost, non-hierarchical activation).
    /// Used for leading-dot (RootIdent) resolution to bypass local scope.
    pub(super) root_activation: &'a dyn Activation,
    pub(super) functions: &'a FunctionRegistry,
    /// References from type checking (for qualified name resolution),
    /// sorted by node id (pcx patch: a HashMap hashed every node's id).
    pub(super) reference_map: Option<&'a References>,
    /// Type registry for message construction and field access.
    pub(super) proto_registry: Option<&'a dyn ProtoRegistry>,
    /// Container namespace for type resolution (C++ namespace rules).
    pub(super) container: String,
    /// Abbreviations for qualified name shortcuts.
    pub(super) abbreviations: Option<&'a HashMap<String, String>>,
    /// Whether to use strong enum typing (default: true).
    /// When false, enum values are returned as plain integers.
    pub(super) strong_enums: bool,
    /// Whether we're inside a local scope (comprehension, cel.bind).
    /// When true, as-is resolution takes priority over container prefixing
    /// because local variables should shadow namespace-resolved names.
    pub(super) in_local_scope: bool,
    /// The steps it may take; none: no limit (pcx patch).
    pub(super) budget: Option<&'a Budget>,
}

impl<'a> Evaluator<'a> {
    /// Create a new evaluator.
    pub fn new(activation: &'a dyn Activation, functions: &'a FunctionRegistry) -> Self {
        Self {
            activation,
            root_activation: activation,
            functions,
            reference_map: None,
            proto_registry: None,
            container: String::new(),
            abbreviations: None,
            strong_enums: true,
            in_local_scope: false,
            budget: None,
        }
    }

    /// Spend at most `budget`'s steps (builder pattern; pcx patch).
    pub fn with_budget(mut self, budget: &'a Budget) -> Self {
        self.budget = Some(budget);
        self
    }


    /// Set the references for qualified name resolution (builder pattern).
    pub fn with_reference_map(mut self, map: &'a References) -> Self {
        self.reference_map = Some(map);
        self
    }

    /// Set the type registry for message construction and field access (builder pattern).
    pub fn with_proto_registry(mut self, registry: &'a dyn ProtoRegistry) -> Self {
        self.proto_registry = Some(registry);
        self
    }

    /// Set the container namespace for type resolution (builder pattern).
    ///
    /// The container is used for resolving unqualified type names following
    /// C++ namespace rules. For example, with container "cel.expr.conformance.proto3"
    /// and type name "TestAllTypes", resolution tries:
    /// 1. cel.expr.conformance.proto3.TestAllTypes
    /// 2. cel.expr.conformance.TestAllTypes
    /// 3. cel.expr.TestAllTypes
    /// 4. cel.TestAllTypes
    /// 5. TestAllTypes
    pub fn with_container(mut self, container: &str) -> Self {
        self.container = container.to_string();
        self
    }

    /// Set abbreviations for qualified name resolution (builder pattern).
    ///
    /// Abbreviations allow short names to be used instead of fully-qualified
    /// type names in expressions.
    pub fn with_abbreviations(mut self, abbreviations: &'a HashMap<String, String>) -> Self {
        self.abbreviations = Some(abbreviations);
        self
    }

    /// Use legacy (weak) enum mode where enum values are returned as plain integers.
    pub fn with_legacy_enums(mut self) -> Self {
        self.strong_enums = false;
        self
    }

    /// Return an enum value or plain int depending on the strong_enums setting.
    pub(super) fn enum_or_int(&self, type_name: &str, value: i32) -> Value {
        if self.strong_enums {
            Value::Enum(super::EnumValue::new(type_name, value))
        } else {
            Value::Int(value as i64)
        }
    }

    /// Evaluate an expression.
    pub fn eval(&self, expr: &SpannedExpr) -> Value {
        self.eval_expr(expr)
    }

    /// Create a child evaluator with a new activation but preserving other settings.
    ///
    /// The root_activation is preserved from the parent, so leading-dot resolution
    /// always refers to the outermost (global) scope.
    pub(super) fn child_evaluator<'b>(&'b self, activation: &'b dyn Activation) -> Evaluator<'b>
    where
        'a: 'b,
    {
        let mut eval = Evaluator::new(activation, self.functions);
        // Preserve root_activation from parent (don't reset to the child activation)
        eval.root_activation = self.root_activation;
        // Child evaluators are always in local scope (comprehension, bind)
        eval.in_local_scope = true;
        if let Some(ref_map) = self.reference_map {
            eval = eval.with_reference_map(ref_map);
        }
        if let Some(proto_registry) = self.proto_registry {
            eval = eval.with_proto_registry(proto_registry);
        }
        if !self.container.is_empty() {
            eval = eval.with_container(&self.container);
        }
        if let Some(abbreviations) = self.abbreviations {
            eval = eval.with_abbreviations(abbreviations);
        }
        if !self.strong_enums {
            eval = eval.with_legacy_enums();
        }
        eval.budget = self.budget;
        eval
    }

    pub(super) fn eval_expr(&self, expr: &SpannedExpr) -> Value {
        if self.budget.is_some_and(|budget| !budget.spend()) {
            return Value::error(EvalError::budget_exceeded());
        }
        // Check reference_map for pre-resolved constant values (enum values, etc.)
        if let Some(ref_map) = self.reference_map {
            if let Some(ref_info) = ref_map.get(&expr.id) {
                if let Some(ref value) = ref_info.value {
                    // If this is an enum value, produce Value::Enum or Value::Int
                    if let Some(ref enum_type) = ref_info.enum_type {
                        if let crate::types::CelValue::Int(i) = value {
                            return self.enum_or_int(enum_type.as_str(), *i as i32);
                        }
                    }
                    return Value::from(value.clone());
                }
            }
        }
        match &expr.node {
            // Literals
            Expr::Null => Value::Null,
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Int(i) => Value::Int(*i),
            Expr::UInt(u) => Value::UInt(*u),
            Expr::Float(f) => Value::Double(*f),
            Expr::String(s) => Value::String(Arc::from(s.as_str())),
            Expr::Bytes(b) => Value::Bytes(Arc::from(b.as_slice())),

            // Identifiers
            Expr::Ident(name) => self.eval_ident(name, expr),
            Expr::RootIdent(name) => self.eval_root_ident(name),

            // Collections
            Expr::List(elements) => self.eval_list(elements),
            Expr::Map(entries) => self.eval_map(entries),

            // Operations
            Expr::Unary { op, expr } => self.eval_unary(*op, expr),
            Expr::Binary { op, left, right } => self.eval_binary(*op, left, right),
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => self.eval_ternary(cond, then_expr, else_expr),

            // Access
            Expr::Member {
                expr: inner,
                field,
                optional,
            } => self.eval_member(inner, field, *optional, expr),
            Expr::Index {
                expr,
                index,
                optional,
            } => self.eval_index(expr, index, *optional),
            Expr::Call { expr: callee, args } => self.eval_call(callee, args, expr),
            Expr::Struct { type_name, fields } => self.eval_struct(type_name, fields),

            // Comprehension
            Expr::Comprehension(comp) => self.eval_comprehension(comp),

            // Member test
            Expr::MemberTestOnly { expr, field } => self.eval_member_test(expr, field),

            // Variable binding
            Expr::Bind {
                var_name,
                init,
                body,
            } => self.eval_bind(var_name, init, body),

            // Error placeholder
            Expr::Error => Value::error(EvalError::internal("evaluated error expression")),
        }
    }

    fn eval_list(&self, elements: &[crate::types::ListElement]) -> Value {
        let mut values = Vec::with_capacity(elements.len());

        for elem in elements {
            let value = self.eval_expr(&elem.expr);

            // Propagate errors
            if value.is_error() {
                return value;
            }

            if elem.optional {
                // Optional list element: only add if present
                match value {
                    Value::Optional(OptionalValue::Some(v)) => values.push(*v),
                    Value::Optional(OptionalValue::None) => {} // Skip absent optionals
                    _ => values.push(value),
                }
            } else {
                values.push(value);
            }
        }

        Value::List(Arc::from(values))
    }

    fn eval_map(&self, entries: &[crate::types::MapEntry]) -> Value {
        let mut map = ValueMap::new();

        for entry in entries {
            let key = self.eval_expr(&entry.key);
            if key.is_error() {
                return key;
            }

            let value = self.eval_expr(&entry.value);
            if value.is_error() {
                return value;
            }

            // Handle optional entries
            if entry.optional {
                match value {
                    Value::Optional(OptionalValue::Some(v)) => {
                        if let Some(map_key) = MapKey::from_value(&key) {
                            if map.contains_key_with_numeric_coercion(&map_key) {
                                return Value::error(EvalError::invalid_argument(
                                    "Failed with repeated key",
                                ));
                            }
                            map.insert(map_key, *v);
                        } else {
                            return Value::error(EvalError::type_mismatch(
                                "valid map key",
                                &key.cel_type().display_name(),
                            ));
                        }
                    }
                    Value::Optional(OptionalValue::None) => {} // Skip absent optionals
                    _ => {
                        if let Some(map_key) = MapKey::from_value(&key) {
                            if map.contains_key_with_numeric_coercion(&map_key) {
                                return Value::error(EvalError::invalid_argument(
                                    "Failed with repeated key",
                                ));
                            }
                            map.insert(map_key, value);
                        } else {
                            return Value::error(EvalError::type_mismatch(
                                "valid map key",
                                &key.cel_type().display_name(),
                            ));
                        }
                    }
                }
            } else if let Some(map_key) = MapKey::from_value(&key) {
                if map.contains_key_with_numeric_coercion(&map_key) {
                    return Value::error(EvalError::invalid_argument("Failed with repeated key"));
                }
                map.insert(map_key, value);
            } else {
                return Value::error(EvalError::type_mismatch(
                    "valid map key",
                    &key.cel_type().display_name(),
                ));
            }
        }

        Value::Map(Arc::new(map))
    }

    pub(super) fn eval_and(&self, left: &SpannedExpr, right: &SpannedExpr) -> Value {
        let left_val = self.eval_expr(left);

        // Short-circuit: if left is false, return false
        match &left_val {
            Value::Bool(false) => return Value::Bool(false),
            Value::Bool(true) => {}
            Value::Error(_) => {
                // CEL semantics: evaluate right side, return error if both error or if right is true
                let right_val = self.eval_expr(right);
                return match right_val {
                    Value::Bool(false) => Value::Bool(false),
                    _ => left_val, // Return the error
                };
            }
            // Non-boolean type: apply commutative logic same as errors
            _ => {
                let right_val = self.eval_expr(right);
                return match right_val {
                    Value::Bool(false) => Value::Bool(false),
                    _ => Value::error(EvalError::no_matching_overload("_&&_")),
                };
            }
        }

        // Left is true, evaluate right
        let right_val = self.eval_expr(right);
        match &right_val {
            Value::Bool(_) | Value::Error(_) => right_val,
            _ => Value::error(EvalError::no_matching_overload("_&&_")),
        }
    }

    pub(super) fn eval_or(&self, left: &SpannedExpr, right: &SpannedExpr) -> Value {
        let left_val = self.eval_expr(left);

        // Short-circuit: if left is true, return true
        match &left_val {
            Value::Bool(true) => return Value::Bool(true),
            Value::Bool(false) => {}
            Value::Error(_) => {
                // CEL semantics: evaluate right side, return error if both error or if right is false
                let right_val = self.eval_expr(right);
                return match right_val {
                    Value::Bool(true) => Value::Bool(true),
                    _ => left_val, // Return the error
                };
            }
            // Non-boolean type: apply commutative logic same as errors
            _ => {
                let right_val = self.eval_expr(right);
                return match right_val {
                    Value::Bool(true) => Value::Bool(true),
                    _ => Value::error(EvalError::no_matching_overload("_||_")),
                };
            }
        }

        // Left is false, evaluate right
        let right_val = self.eval_expr(right);
        match &right_val {
            Value::Bool(_) | Value::Error(_) => right_val,
            _ => Value::error(EvalError::no_matching_overload("_||_")),
        }
    }

    fn eval_ternary(
        &self,
        cond: &SpannedExpr,
        then_expr: &SpannedExpr,
        else_expr: &SpannedExpr,
    ) -> Value {
        let cond_val = self.eval_expr(cond);

        match cond_val {
            Value::Bool(true) => self.eval_expr(then_expr),
            Value::Bool(false) => self.eval_expr(else_expr),
            Value::Error(_) => cond_val,
            _ => Value::error(EvalError::type_mismatch(
                "bool",
                &cond_val.cel_type().display_name(),
            )),
        }
    }

    fn eval_member(
        &self,
        receiver: &SpannedExpr,
        field: &str,
        optional: bool,
        member_expr: &SpannedExpr,
    ) -> Value {
        // First, check reference_map for a pre-resolved qualified name (from checker)
        if let Some(ref_map) = self.reference_map {
            if let Some(ref_info) = ref_map.get(&member_expr.id) {
                // The checker resolved this to a qualified variable name
                if ref_info.overload_ids.is_empty() && ref_info.value.is_none() {
                    if let Some(v) = self.activation.resolve(&ref_info.name) {
                        return v;
                    }
                }
            }
        }

        // Try qualified identifier resolution (e.g., a.b.c as a variable name),
        // but only when the leftmost identifier does NOT resolve in the current scope.
        // This ensures comprehension variables (local scope) shadow qualified names.
        // pcx patch: the qualified name is written only when the leftmost
        // identifier does not resolve (a comprehension's or a bound
        // variable's member is read without writing "item.name" each time).
        // A leading-dot chain's leftmost never resolves, so it took this
        // branch before too (the one for a resolving leftmost with a leading
        // dot could not be reached).
        if !optional && !self.leftmost_ident_resolves(receiver) {
            if let Some(qualified_name) = self.try_qualified_variable_name(receiver, field) {
                if let Some(v) = self.try_longest_prefix_match(&qualified_name, false) {
                    return v;
                }
            }
        }

        // Fall through to normal field-access evaluation
        let value = self.eval_expr(receiver);

        if value.is_error() {
            return value;
        }

        // Handle optional select
        if optional {
            match &value {
                Value::Optional(OptionalValue::None) => {
                    return Value::Optional(OptionalValue::None)
                }
                Value::Optional(OptionalValue::Some(inner)) => {
                    return self.access_field(inner, field, true);
                }
                _ => {}
            }
        }

        self.access_field(&value, field, optional)
    }

    pub(super) fn access_field(&self, value: &Value, field: &str, optional: bool) -> Value {
        match value {
            Value::Map(map) => {
                match map.get_str(field) {
                    Some(v) => {
                        if optional {
                            Value::optional_some(v.clone())
                        } else {
                            v.clone()
                        }
                    }
                    None => {
                        if optional {
                            Value::optional_none()
                        } else {
                            Value::error(EvalError::key_not_found(field))
                        }
                    }
                }
            }
            Value::Message(msg) => {
                if let Some(registry) = self.proto_registry {
                    registry.message_field_access(msg.as_ref(), field, optional, self.strong_enums)
                } else if optional {
                    Value::optional_none()
                } else {
                    Value::error(EvalError::field_not_found(field))
                }
            }
            Value::Optional(opt) => match opt {
                OptionalValue::Some(inner) => {
                    // Only map/proto types support field access in optional-land.
                    // For those types, missing fields become Optional(None).
                    // For other types (null, int, etc.), propagate the error.
                    match inner.as_ref() {
                        Value::Map(_) | Value::Message(_) => {
                            let result = self.access_field(inner, field, false);
                            if result.is_error() {
                                Value::optional_none()
                            } else {
                                Value::optional_some(result)
                            }
                        }
                        _ => self.access_field(inner, field, false),
                    }
                }
                OptionalValue::None => Value::optional_none(),
            },
            _ => {
                if optional {
                    Value::optional_none()
                } else {
                    Value::error(EvalError::field_not_found(field))
                }
            }
        }
    }

    fn eval_index(&self, expr: &SpannedExpr, index: &SpannedExpr, optional: bool) -> Value {
        let value = self.eval_expr(expr);
        if value.is_error() {
            return value;
        }

        let index_val = self.eval_expr(index);
        if index_val.is_error() {
            return index_val;
        }

        // Handle optional index
        if optional {
            match &value {
                Value::Optional(OptionalValue::None) => {
                    return Value::Optional(OptionalValue::None)
                }
                Value::Optional(OptionalValue::Some(inner)) => {
                    return self.access_index(inner, &index_val, true);
                }
                _ => {}
            }
        }

        self.access_index(&value, &index_val, optional)
    }

    fn access_index(&self, value: &Value, index: &Value, optional: bool) -> Value {
        match value {
            Value::List(list) => {
                let idx = match index {
                    Value::Int(i) => *i,
                    Value::UInt(u) => *u as i64,
                    Value::Double(d) if d.is_finite() && d.fract() == 0.0 => *d as i64,
                    _ => {
                        return Value::error(EvalError::type_mismatch(
                            "int",
                            &index.cel_type().display_name(),
                        ))
                    }
                };

                // Handle negative indices
                let len = list.len() as i64;
                let actual_idx = if idx < 0 { idx + len } else { idx };

                if actual_idx < 0 || actual_idx >= len {
                    if optional {
                        Value::optional_none()
                    } else {
                        Value::error(EvalError::index_out_of_bounds(idx, list.len()))
                    }
                } else {
                    let result = list[actual_idx as usize].clone();
                    if optional {
                        Value::optional_some(result)
                    } else {
                        result
                    }
                }
            }
            Value::Map(map) => {
                // Handle double keys: coerce exact integers to int/uint for lookup
                let map_key = if let Value::Double(d) = index {
                    if d.is_finite() && d.fract() == 0.0 {
                        // Try as int key first, then uint
                        let as_int = *d as i64;
                        let int_key = MapKey::Int(as_int);
                        if let Some(v) = map.get_with_numeric_coercion(&int_key) {
                            let result = v.clone();
                            return if optional {
                                Value::optional_some(result)
                            } else {
                                result
                            };
                        }
                        // Key not found
                        return if optional {
                            Value::optional_none()
                        } else {
                            Value::error(EvalError::key_not_found(&format!("{}", index)))
                        };
                    } else {
                        return Value::error(EvalError::key_not_found(&format!("{}", index)));
                    }
                } else {
                    MapKey::from_value(index)
                };
                if let Some(key) = map_key {
                    match map.get_with_numeric_coercion(&key) {
                        Some(v) => {
                            if optional {
                                Value::optional_some(v.clone())
                            } else {
                                v.clone()
                            }
                        }
                        None => {
                            if optional {
                                Value::optional_none()
                            } else {
                                Value::error(EvalError::key_not_found(&format!("{}", index)))
                            }
                        }
                    }
                } else {
                    Value::error(EvalError::type_mismatch(
                        "valid map key",
                        &index.cel_type().display_name(),
                    ))
                }
            }
            Value::String(s) => {
                let idx = match index {
                    Value::Int(i) => *i,
                    _ => {
                        return Value::error(EvalError::type_mismatch(
                            "int",
                            &index.cel_type().display_name(),
                        ))
                    }
                };

                // Convert string to code points for proper indexing
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len() as i64;
                let actual_idx = if idx < 0 { idx + len } else { idx };

                if actual_idx < 0 || actual_idx >= len {
                    if optional {
                        Value::optional_none()
                    } else {
                        Value::error(EvalError::index_out_of_bounds(idx, chars.len()))
                    }
                } else {
                    let result = Value::String(Arc::from(chars[actual_idx as usize].to_string()));
                    if optional {
                        Value::optional_some(result)
                    } else {
                        result
                    }
                }
            }
            Value::Bytes(bytes) => {
                let idx = match index {
                    Value::Int(i) => *i,
                    _ => {
                        return Value::error(EvalError::type_mismatch(
                            "int",
                            &index.cel_type().display_name(),
                        ))
                    }
                };

                let len = bytes.len() as i64;
                let actual_idx = if idx < 0 { idx + len } else { idx };

                if actual_idx < 0 || actual_idx >= len {
                    if optional {
                        Value::optional_none()
                    } else {
                        Value::error(EvalError::index_out_of_bounds(idx, bytes.len()))
                    }
                } else {
                    let result = Value::UInt(bytes[actual_idx as usize] as u64);
                    if optional {
                        Value::optional_some(result)
                    } else {
                        result
                    }
                }
            }
            Value::Optional(opt) => match opt {
                OptionalValue::Some(inner) => match inner.as_ref() {
                    Value::List(_) | Value::Map(_) => {
                        let result = self.access_index(inner, index, false);
                        if result.is_error() {
                            Value::optional_none()
                        } else {
                            Value::optional_some(result)
                        }
                    }
                    _ => self.access_index(inner, index, false),
                },
                OptionalValue::None => Value::optional_none(),
            },
            _ => {
                if optional {
                    Value::optional_none()
                } else {
                    Value::error(EvalError::type_mismatch(
                        "list, map, string, or bytes",
                        &value.cel_type().display_name(),
                    ))
                }
            }
        }
    }

    fn eval_call(
        &self,
        expr: &SpannedExpr,
        args: &[SpannedExpr],
        call_expr: &SpannedExpr,
    ) -> Value {
        // Check reference_map for enum constructor calls
        if let Some(ref_map) = self.reference_map {
            if let Some(ref_info) = ref_map.get(&call_expr.id) {
                if ref_info
                    .overload_ids
                    .iter()
                    .any(|id| id == "enum_constructor")
                {
                    if let Some(ref enum_type) = ref_info.enum_type {
                        return self.eval_enum_constructor(enum_type, args);
                    }
                }
            }
        }

        // Try namespaced function first (e.g., strings.quote, math.greatest)
        if let Expr::Member {
            expr: receiver,
            field,
            ..
        } = &expr.node
        {
            if let Some(qualified_name) = self.try_qualified_function_name(receiver, field) {
                if self.functions.contains(&qualified_name) {
                    let mut arg_values = Vec::with_capacity(args.len());
                    for arg in args {
                        let val = self.eval_expr(arg);
                        if val.is_error() {
                            return val;
                        }
                        arg_values.push(val);
                    }
                    return self.call_function(&qualified_name, &arg_values, false);
                }
            }
        }

        // Determine function name and whether it's a member call
        let (func_name, receiver, is_member) = self.resolve_call_target(expr);

        // Evaluate arguments
        let mut arg_values =
            Vec::with_capacity(args.len() + if receiver.is_some() { 1 } else { 0 });

        if let Some(recv) = receiver {
            let recv_val = self.eval_expr(recv);
            if recv_val.is_error() {
                return recv_val;
            }
            arg_values.push(recv_val);
        }

        for arg in args {
            let val = self.eval_expr(arg);
            if val.is_error() {
                return val;
            }
            arg_values.push(val);
        }

        // Look up function in registry
        self.call_function(&func_name, &arg_values, is_member)
    }

    fn resolve_call_target<'b>(
        &self,
        expr: &'b SpannedExpr,
    ) -> (String, Option<&'b SpannedExpr>, bool) {
        match &expr.node {
            Expr::Ident(name) | Expr::RootIdent(name) => (name.clone(), None, false),
            Expr::Member {
                expr: recv,
                field,
                optional: _,
            } => (field.clone(), Some(recv.as_ref()), true),
            _ => ("".to_string(), None, false),
        }
    }

    fn try_qualified_function_name(&self, obj: &SpannedExpr, field: &str) -> Option<String> {
        match &obj.node {
            Expr::Ident(name) => Some(format!("{}.{}", name, field)),
            Expr::Member {
                expr: inner,
                field: inner_field,
                ..
            } => {
                let prefix = self.try_qualified_function_name(inner, inner_field)?;
                Some(format!("{}.{}", prefix, field))
            }
            _ => None,
        }
    }

    /// Evaluate an enum constructor call (e.g., `TestAllTypes.NestedEnum(1)` or `GlobalEnum("BAZ")`).
    fn eval_enum_constructor(&self, enum_type_name: &str, args: &[SpannedExpr]) -> Value {
        if args.len() != 1 {
            return Value::error(EvalError::invalid_argument(
                "enum constructor expects exactly 1 argument".to_string(),
            ));
        }

        let arg = self.eval_expr(&args[0]);
        if arg.is_error() {
            return arg;
        }

        match &arg {
            Value::Int(i) => {
                // Int → Enum: range check i32
                if *i > i32::MAX as i64 || *i < i32::MIN as i64 {
                    Value::error(EvalError::overflow("int to enum overflow"))
                } else {
                    self.enum_or_int(enum_type_name, *i as i32)
                }
            }
            Value::String(s) => {
                // String → Enum: look up value by name in type registry
                if let Some(registry) = self.proto_registry {
                    if let Some(value) = registry.get_enum_value(enum_type_name, s) {
                        self.enum_or_int(enum_type_name, value)
                    } else {
                        Value::error(EvalError::invalid_argument(format!(
                            "unknown enum value '{}'",
                            s
                        )))
                    }
                } else {
                    Value::error(EvalError::internal(
                        "no type registry available for enum lookup",
                    ))
                }
            }
            _ => Value::error(EvalError::no_matching_overload("enum constructor")),
        }
    }

    fn call_function(&self, name: &str, args: &[Value], is_member: bool) -> Value {
        // Look up function
        let overloads = self.functions.find_overloads(name, args.len(), is_member);

        if overloads.is_empty() {
            // Check if it's a type conversion function
            if let Some(result) = self.try_type_conversion(name, args) {
                return result;
            }

            // Check for built-in functions handled specially
            if let Some(result) = self.try_builtin_function(name, args) {
                return result;
            }

            return Value::error(EvalError::unknown_function(name));
        }

        // Try each overload until one works
        // In a typed system, we'd use type information to select the right overload
        // For now, try them all and return the first non-error result
        for overload in &overloads {
            let result = overload.call(args);
            if !result.is_error() {
                return result;
            }
        }

        // All overloads failed, return the error from the first one
        overloads[0].call(args)
    }

    fn eval_comprehension(&self, comp: &ComprehensionData) -> Value {
        // Evaluate the iteration range
        let range_val = self.eval_expr(&comp.iter_range);
        if range_val.is_error() {
            return range_val;
        }

        // Initialize accumulator
        let mut accu = self.eval_expr(&comp.accu_init);
        if accu.is_error() {
            return accu;
        }

        // Create iterator based on range type
        match &range_val {
            Value::List(list) => {
                for (i, elem) in list.iter().enumerate() {
                    // Create nested activation with iteration variables
                    let mut iter_activation = HierarchicalActivation::new(self.activation)
                        .with_binding(&comp.accu_var, accu.clone());

                    if !comp.iter_var.is_empty() {
                        if !comp.iter_var2.is_empty() {
                            // Two-variable form: iter_var = index, iter_var2 = element
                            iter_activation.insert(&comp.iter_var, Value::Int(i as i64));
                        } else {
                            // Single-variable form: iter_var = element
                            iter_activation.insert(&comp.iter_var, elem.clone());
                        }
                    }
                    if !comp.iter_var2.is_empty() {
                        iter_activation.insert(&comp.iter_var2, elem.clone());
                    }

                    // An iteration costs a step, and a spent budget ends
                    // the loop (pcx patch).
                    if self.budget.is_some_and(|budget| !budget.spend()) {
                        return Value::error(EvalError::budget_exceeded());
                    }
                    let iter_eval = self.child_evaluator(&iter_activation);

                    // Check loop condition
                    let cond = iter_eval.eval_expr(&comp.loop_condition);
                    match &cond {
                        Value::Bool(false) => break,
                        Value::Bool(true) => {}
                        Value::Error(_) => {} // continue iteration (not strictly false)
                        _ => {
                            return Value::error(EvalError::type_mismatch(
                                "bool",
                                &cond.cel_type().display_name(),
                            ))
                        }
                    }

                    // Compute next accumulator value
                    accu = iter_eval.eval_expr(&comp.loop_step);
                }
            }
            Value::Map(map) => {
                for (key, val) in map.iter() {
                    let mut iter_activation = HierarchicalActivation::new(self.activation)
                        .with_binding(&comp.accu_var, accu.clone());

                    if !comp.iter_var.is_empty() {
                        iter_activation.insert(&comp.iter_var, key.to_value());
                    }
                    if !comp.iter_var2.is_empty() {
                        iter_activation.insert(&comp.iter_var2, val.clone());
                    }

                    // An iteration costs a step, and a spent budget ends
                    // the loop (pcx patch).
                    if self.budget.is_some_and(|budget| !budget.spend()) {
                        return Value::error(EvalError::budget_exceeded());
                    }
                    let iter_eval = self.child_evaluator(&iter_activation);

                    // Check loop condition
                    let cond = iter_eval.eval_expr(&comp.loop_condition);
                    match &cond {
                        Value::Bool(false) => break,
                        Value::Bool(true) => {}
                        Value::Error(_) => {} // continue iteration (not strictly false)
                        _ => {
                            return Value::error(EvalError::type_mismatch(
                                "bool",
                                &cond.cel_type().display_name(),
                            ))
                        }
                    }

                    // Compute next accumulator value
                    accu = iter_eval.eval_expr(&comp.loop_step);
                }
            }
            _ => {
                return Value::error(EvalError::type_mismatch(
                    "list or map",
                    &range_val.cel_type().display_name(),
                ))
            }
        }

        // Compute final result
        let result_activation =
            HierarchicalActivation::new(self.activation).with_binding(&comp.accu_var, accu);
        let result_eval = self.child_evaluator(&result_activation);
        result_eval.eval_expr(&comp.result)
    }

    fn eval_member_test(&self, expr: &SpannedExpr, field: &str) -> Value {
        let value = self.eval_expr(expr);

        // For `has()`, we don't propagate errors - we just check presence
        if value.is_error() {
            return Value::Bool(false);
        }

        match &value {
            Value::Map(map) => {
                Value::Bool(map.contains_str(field))
            }
            Value::Message(msg) => {
                if let Some(registry) = self.proto_registry {
                    registry.message_has_field(msg.as_ref(), field)
                } else {
                    Value::Bool(false)
                }
            }
            Value::Optional(opt) => match opt {
                OptionalValue::Some(inner) => match inner.as_ref() {
                    Value::Map(map) => {
                        Value::Bool(map.contains_str(field))
                    }
                    Value::Message(msg) => {
                        if let Some(registry) = self.proto_registry {
                            registry.message_has_field(msg.as_ref(), field)
                        } else {
                            Value::Bool(false)
                        }
                    }
                    _ => Value::Bool(false),
                },
                OptionalValue::None => Value::Bool(false),
            },
            _ => Value::Bool(false),
        }
    }

    /// Evaluate a struct construction expression.
    ///
    /// Resolves the type name, evaluates all field expressions, then delegates
    /// to the type registry for actual message construction.
    pub(super) fn eval_struct(
        &self,
        type_name: &SpannedExpr,
        fields: &[crate::types::StructField],
    ) -> Value {
        use super::proto_registry::StructFieldValue;

        // Get the fully qualified type name
        let extracted_name = self.get_type_name_from_expr(type_name);
        let fq_name = match self.resolve_type_name(type_name) {
            Some(name) => name,
            None => {
                return Value::error(EvalError::internal(format!(
                    "could not resolve type name for struct (extracted: {:?})",
                    extracted_name
                )))
            }
        };

        // Get the type registry
        let registry = match self.proto_registry {
            Some(r) => r,
            None => {
                return Value::error(EvalError::internal(format!(
                    "type registry not available for struct construction (type: {})",
                    fq_name
                )))
            }
        };

        // Evaluate all field expressions
        let mut evaluated_fields = Vec::with_capacity(fields.len());
        for field in fields {
            let value = self.eval_expr(&field.value);
            if value.is_error() {
                return value;
            }
            evaluated_fields.push(StructFieldValue {
                name: field.name.clone(),
                value,
                optional: field.optional,
            });
        }

        // Delegate to the type registry for message construction
        registry.construct_message(&fq_name, &evaluated_fields, self.strong_enums)
    }

    fn eval_bind(&self, var_name: &str, init: &SpannedExpr, body: &SpannedExpr) -> Value {
        let init_val = self.eval_expr(init);
        if init_val.is_error() {
            return init_val;
        }

        let bind_activation =
            HierarchicalActivation::new(self.activation).with_binding(var_name, init_val);
        let bind_eval = self.child_evaluator(&bind_activation);
        bind_eval.eval_expr(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn eval_expr(source: &str) -> Value {
        let result = parse(source);
        assert!(
            result.errors.is_empty(),
            "parse errors: {:?}",
            result.errors
        );
        let ast = result.ast.unwrap();

        let activation = super::super::EmptyActivation;
        let registry = FunctionRegistry::new();
        let evaluator = Evaluator::new(&activation, &registry);
        evaluator.eval(&ast)
    }

    fn eval_expr_with_vars(source: &str, vars: &[(&str, Value)]) -> Value {
        let result = parse(source);
        assert!(
            result.errors.is_empty(),
            "parse errors: {:?}",
            result.errors
        );
        let ast = result.ast.unwrap();

        let mut activation = super::super::MapActivation::new();
        for (name, value) in vars {
            activation.insert(*name, value.clone());
        }

        let registry = FunctionRegistry::new();
        let evaluator = Evaluator::new(&activation, &registry);
        evaluator.eval(&ast)
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_literals() {
        assert_eq!(eval_expr("null"), Value::Null);
        assert_eq!(eval_expr("true"), Value::Bool(true));
        assert_eq!(eval_expr("false"), Value::Bool(false));
        assert_eq!(eval_expr("42"), Value::Int(42));
        assert_eq!(eval_expr("42u"), Value::UInt(42));
        assert_eq!(eval_expr("3.14"), Value::Double(3.14));
        assert_eq!(eval_expr("\"hello\""), "hello".into());
    }

    #[test]
    fn test_arithmetic() {
        assert_eq!(eval_expr("1 + 2"), Value::Int(3));
        assert_eq!(eval_expr("5 - 3"), Value::Int(2));
        assert_eq!(eval_expr("3 * 4"), Value::Int(12));
        assert_eq!(eval_expr("10 / 3"), Value::Int(3));
        assert_eq!(eval_expr("10 % 3"), Value::Int(1));
    }

    #[test]
    fn test_comparison() {
        assert_eq!(eval_expr("1 < 2"), Value::Bool(true));
        assert_eq!(eval_expr("2 <= 2"), Value::Bool(true));
        assert_eq!(eval_expr("3 > 2"), Value::Bool(true));
        assert_eq!(eval_expr("2 >= 2"), Value::Bool(true));
        assert_eq!(eval_expr("1 == 1"), Value::Bool(true));
        assert_eq!(eval_expr("1 != 2"), Value::Bool(true));
    }

    #[test]
    fn test_logical() {
        assert_eq!(eval_expr("true && true"), Value::Bool(true));
        assert_eq!(eval_expr("true && false"), Value::Bool(false));
        assert_eq!(eval_expr("false || true"), Value::Bool(true));
        assert_eq!(eval_expr("false || false"), Value::Bool(false));
        assert_eq!(eval_expr("!true"), Value::Bool(false));
    }

    #[test]
    fn test_short_circuit() {
        // && short-circuits on false
        assert_eq!(eval_expr("false && undefined"), Value::Bool(false));
        // || short-circuits on true
        assert_eq!(eval_expr("true || undefined"), Value::Bool(true));
    }

    #[test]
    fn test_ternary() {
        assert_eq!(eval_expr("true ? 1 : 2"), Value::Int(1));
        assert_eq!(eval_expr("false ? 1 : 2"), Value::Int(2));
    }

    #[test]
    fn test_string_operations() {
        assert_eq!(eval_expr("\"hello\" + \" world\""), "hello world".into());
        assert_eq!(eval_expr("size(\"hello\")"), Value::Int(5));
        assert_eq!(eval_expr("\"hello\".contains(\"ell\")"), Value::Bool(true));
        assert_eq!(eval_expr("\"hello\".startsWith(\"he\")"), Value::Bool(true));
        assert_eq!(eval_expr("\"hello\".endsWith(\"lo\")"), Value::Bool(true));
    }

    #[test]
    fn test_list_operations() {
        assert_eq!(eval_expr("[1, 2, 3][0]"), Value::Int(1));
        assert_eq!(eval_expr("[1, 2, 3][2]"), Value::Int(3));
        assert_eq!(eval_expr("size([1, 2, 3])"), Value::Int(3));
        assert_eq!(eval_expr("2 in [1, 2, 3]"), Value::Bool(true));
        assert_eq!(eval_expr("4 in [1, 2, 3]"), Value::Bool(false));
    }

    #[test]
    fn test_map_operations() {
        assert_eq!(eval_expr("{\"a\": 1, \"b\": 2}[\"a\"]"), Value::Int(1));
        assert_eq!(eval_expr("{\"a\": 1, \"b\": 2}.a"), Value::Int(1));
        assert_eq!(eval_expr("size({\"a\": 1, \"b\": 2})"), Value::Int(2));
        assert_eq!(
            eval_expr("\"a\" in {\"a\": 1, \"b\": 2}"),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_variables() {
        assert_eq!(
            eval_expr_with_vars("x + 1", &[("x", Value::Int(41))]),
            Value::Int(42)
        );
        assert_eq!(
            eval_expr_with_vars(
                "x && y",
                &[("x", Value::Bool(true)), ("y", Value::Bool(false))]
            ),
            Value::Bool(false)
        );
    }

    #[test]
    fn test_type_conversions() {
        assert_eq!(eval_expr("int(3.7)"), Value::Int(3));
        assert_eq!(eval_expr("double(42)"), Value::Double(42.0));
        assert_eq!(eval_expr("string(42)"), "42".into());
        assert_eq!(eval_expr("int(\"42\")"), Value::Int(42));
    }

    #[test]
    fn test_division_by_zero() {
        let result = eval_expr("1 / 0");
        assert!(result.is_error());
    }

    #[test]
    fn test_overflow() {
        let result = eval_expr("9223372036854775807 + 1");
        assert!(result.is_error());
    }

    fn eval_with_extensions(source: &str) -> Value {
        let env = crate::Env::with_standard_library().with_all_extensions();
        let ast = env.compile(source).expect("compile failed");
        let program = env.program(&ast).expect("program failed");
        program.eval_empty()
    }

    #[test]
    fn test_namespaced_function_strings_quote() {
        let result = eval_with_extensions("strings.quote(\"hello\")");
        assert_eq!(result, Value::String("\"hello\"".into()));
    }

    #[test]
    fn test_namespaced_function_strings_quote_escape() {
        let result = eval_with_extensions("strings.quote(\"first\\nsecond\")");
        assert_eq!(result, Value::String(Arc::from("\"first\\nsecond\"")));
    }

    #[test]
    fn test_member_call_still_works() {
        let result = eval_with_extensions("\"hello\".startsWith(\"he\")");
        assert_eq!(result, Value::Bool(true));
    }
}
