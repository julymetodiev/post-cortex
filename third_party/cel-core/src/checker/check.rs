//! Core type checker implementation.
//!
//! This module provides the main `Checker` struct and the `check` function
//! for type-checking CEL expressions.
//!
//! The checker is independent and takes raw data (variables, functions, container)
//! rather than a type environment struct. This allows it to be used as a building
//! block for higher-level APIs.

use std::collections::HashMap;
use std::sync::Arc;

use super::errors::CheckError;
use super::overload::{
    finalize_type, format_expected_args, resolve_overload, substitute_type, OverloadMismatch,
};
use super::scope::ScopeStack;
use crate::eval::proto_registry::ProtoTypeResolver;
use crate::types::{
    BinaryOp, CelType, CelValue, ComprehensionData, Expr, FunctionDecl, ListElement, MapEntry,
    ResolvedProtoType, SpannedExpr, StructField, UnaryOp, VariableDecl,
};

/// Reference information for a resolved identifier or function.
#[derive(Debug, Clone)]
pub struct ReferenceInfo {
    /// The fully qualified name.
    pub name: String,
    /// Matching overload IDs for function calls.
    pub overload_ids: Vec<String>,
    /// Constant value for enum constants.
    pub value: Option<CelValue>,
    /// Enum type name (for strong enum typing).
    pub enum_type: Option<String>,
}

impl ReferenceInfo {
    /// Create a new identifier reference.
    pub fn ident(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            overload_ids: Vec::new(),
            value: None,
            enum_type: None,
        }
    }

    /// Create a new function reference with overload IDs.
    pub fn function(name: impl Into<String>, overload_ids: Vec<String>) -> Self {
        Self {
            name: name.into(),
            overload_ids,
            value: None,
            enum_type: None,
        }
    }
}

/// Result of type checking an expression.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Map from expression ID to inferred type.
    pub type_map: HashMap<i64, CelType>,
    /// Map from expression ID to resolved reference.
    pub reference_map: HashMap<i64, ReferenceInfo>,
    /// Errors encountered during type checking.
    pub errors: Vec<CheckError>,
}

impl CheckResult {
    /// Check if type checking was successful (no errors).
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Get the type for an expression ID.
    pub fn get_type(&self, expr_id: i64) -> Option<&CelType> {
        self.type_map.get(&expr_id)
    }

    /// Get the reference for an expression ID.
    pub fn get_reference(&self, expr_id: i64) -> Option<&ReferenceInfo> {
        self.reference_map.get(&expr_id)
    }
}

/// Type checker for CEL expressions.
///
/// The checker takes raw data (variables, functions, container) rather than
/// a type environment struct, making it independent and reusable.
pub struct Checker<'a> {
    /// Scope stack for variable resolution (managed internally).
    scopes: ScopeStack,
    /// Function declarations indexed by name.
    functions: &'a HashMap<String, FunctionDecl>,
    /// Container namespace for qualified name resolution.
    container: &'a str,
    /// Map from expression ID to inferred type.
    type_map: HashMap<i64, CelType>,
    /// Map from expression ID to resolved reference.
    reference_map: HashMap<i64, ReferenceInfo>,
    /// Type checking errors.
    errors: Vec<CheckError>,
    /// Type parameter substitutions.
    substitutions: HashMap<Arc<str>, CelType>,
    /// Type resolver for resolving protobuf types.
    type_resolver: Option<&'a dyn ProtoTypeResolver>,
    /// Abbreviations for qualified name shortcuts.
    abbreviations: Option<&'a HashMap<String, String>>,
}

impl<'a> Checker<'a> {
    /// Create a new type checker with the given data.
    ///
    /// # Arguments
    /// * `variables` - Variable declarations (name -> type)
    /// * `functions` - Function declarations indexed by name
    /// * `container` - Container namespace for qualified name resolution
    pub fn new(
        variables: &HashMap<String, CelType>,
        functions: &'a HashMap<String, FunctionDecl>,
        container: &'a str,
    ) -> Self {
        let mut scopes = ScopeStack::new();

        // Add variables to the initial scope
        for (name, cel_type) in variables {
            scopes.add_variable(name, cel_type.clone());
        }

        Self {
            scopes,
            functions,
            container,
            type_map: HashMap::new(),
            reference_map: HashMap::new(),
            errors: Vec::new(),
            substitutions: HashMap::new(),
            type_resolver: None,
            abbreviations: None,
        }
    }

    /// Set the type resolver for resolving protobuf types.
    pub fn with_type_resolver(mut self, resolver: &'a dyn ProtoTypeResolver) -> Self {
        self.type_resolver = Some(resolver);
        self
    }

    /// Set abbreviations for qualified name resolution.
    pub fn with_abbreviations(mut self, abbreviations: &'a HashMap<String, String>) -> Self {
        self.abbreviations = Some(abbreviations);
        self
    }

    /// Type check an expression and return the result.
    pub fn check(mut self, expr: &SpannedExpr) -> CheckResult {
        self.check_expr(expr);
        self.finalize_types();

        CheckResult {
            type_map: self.type_map,
            reference_map: self.reference_map,
            errors: self.errors,
        }
    }

    /// Store the type for an expression.
    fn set_type(&mut self, expr_id: i64, cel_type: CelType) {
        self.type_map.insert(expr_id, cel_type);
    }

    /// Store a reference for an expression.
    fn set_reference(&mut self, expr_id: i64, reference: ReferenceInfo) {
        self.reference_map.insert(expr_id, reference);
    }

    /// Report an error.
    fn report_error(&mut self, error: CheckError) {
        self.errors.push(error);
    }

    /// Finalize all types by replacing unbound type variables with Dyn.
    fn finalize_types(&mut self) {
        for ty in self.type_map.values_mut() {
            *ty = finalize_type(ty);
            *ty = substitute_type(ty, &self.substitutions);
            *ty = finalize_type(ty);
        }
    }

    /// Type check an expression and return its type.
    fn check_expr(&mut self, expr: &SpannedExpr) -> CelType {
        let result = match &expr.node {
            Expr::Null => CelType::Null,
            Expr::Bool(_) => CelType::Bool,
            Expr::Int(_) => CelType::Int,
            Expr::UInt(_) => CelType::UInt,
            Expr::Float(_) => CelType::Double,
            Expr::String(_) => CelType::String,
            Expr::Bytes(_) => CelType::Bytes,

            Expr::Ident(name) => self.check_ident(name, expr),
            Expr::RootIdent(name) => self.check_ident(name, expr),

            Expr::List(elements) => self.check_list(elements, expr),
            Expr::Map(entries) => self.check_map(entries, expr),

            Expr::Unary { op, expr: inner } => self.check_unary(*op, inner, expr),
            Expr::Binary { op, left, right } => self.check_binary(*op, left, right, expr),
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => self.check_ternary(cond, then_expr, else_expr, expr),

            Expr::Member {
                expr: obj,
                field,
                optional,
            } => self.check_member(obj, field, *optional, expr),
            Expr::Index {
                expr: obj,
                index,
                optional,
            } => self.check_index(obj, index, *optional, expr),
            Expr::Call { expr: callee, args } => self.check_call(callee, args, expr),
            Expr::Struct { type_name, fields } => self.check_struct(type_name, fields, expr),

            Expr::Comprehension(comp) => self.check_comprehension(comp, expr),

            Expr::MemberTestOnly { expr: obj, field } => self.check_member_test(obj, field, expr),

            Expr::Bind {
                var_name,
                init,
                body,
            } => self.check_bind(var_name, init, body, expr),

            Expr::Error => CelType::Error,
        };

        self.set_type(expr.id, result.clone());
        result
    }

    /// Check an identifier expression.
    ///
    /// Resolution order:
    /// - If the name is a local variable (comprehension, bind), use it directly
    ///   (local variables shadow container-prefixed names)
    /// - Otherwise: container-prefixed first (most qualified → least), then as-is
    fn check_ident(&mut self, name: &str, expr: &SpannedExpr) -> CelType {
        // Local variables (comprehension, bind) always take priority
        if self.scopes.is_local(name) {
            if let Some(decl) = self.scopes.resolve(name) {
                let cel_type = decl.cel_type.clone();
                self.set_reference(expr.id, ReferenceInfo::ident(name));
                return cel_type;
            }
        }

        // Try container-prefixed (namespace shadowing at global level)
        if !self.container.is_empty() {
            let mut container = self.container;
            loop {
                let qualified = format!("{}.{}", container, name);
                if let Some(decl) = self.scopes.resolve(&qualified) {
                    let cel_type = decl.cel_type.clone();
                    self.set_reference(expr.id, ReferenceInfo::ident(&qualified));
                    return cel_type;
                }
                match container.rfind('.') {
                    Some(pos) => container = &container[..pos],
                    None => break,
                }
            }
        }

        // Then try as-is
        if let Some(decl) = self.scopes.resolve(name) {
            let cel_type = decl.cel_type.clone();
            self.set_reference(expr.id, ReferenceInfo::ident(name));
            return cel_type;
        }

        self.report_error(CheckError::undeclared_reference(
            name,
            expr.span.clone(),
            expr.id,
        ));
        CelType::Error
    }

    /// Check a list literal.
    fn check_list(&mut self, elements: &[ListElement], _expr: &SpannedExpr) -> CelType {
        if elements.is_empty() {
            return CelType::list(CelType::fresh_type_var());
        }

        let mut elem_types = Vec::new();
        for elem in elements {
            let elem_type = self.check_expr(&elem.expr);
            elem_types.push(elem_type);
        }

        let joined = self.join_types(&elem_types);
        CelType::list(joined)
    }

    /// Check a map literal.
    fn check_map(&mut self, entries: &[MapEntry], _expr: &SpannedExpr) -> CelType {
        if entries.is_empty() {
            return CelType::map(CelType::fresh_type_var(), CelType::fresh_type_var());
        }

        let mut key_types = Vec::new();
        let mut value_types = Vec::new();

        for entry in entries {
            let key_type = self.check_expr(&entry.key);
            let value_type = self.check_expr(&entry.value);
            // Optional entries have values of type optional<T>, but the map value type is T
            let value_type = if entry.optional {
                match value_type {
                    CelType::Optional(inner) => (*inner).clone(),
                    other => other,
                }
            } else {
                value_type
            };
            key_types.push(key_type);
            value_types.push(value_type);
        }

        let key_joined = self.join_types(&key_types);
        let value_joined = self.join_types(&value_types);

        CelType::map(key_joined, value_joined)
    }

    /// Join multiple types into a common type.
    ///
    /// Prefers the most specific type among compatible candidates.
    /// Specificity: concrete types > Dyn, deeper nesting > shallower.
    fn join_types(&self, types: &[CelType]) -> CelType {
        if types.is_empty() {
            return CelType::fresh_type_var();
        }

        // Find the most specific type that all others are assignable to
        let mut best = &types[0];
        for candidate in types {
            if type_specificity(candidate) > type_specificity(best) {
                // Check if all types are compatible with this more specific candidate
                if types.iter().all(|t| candidate.is_assignable_from(t)) {
                    best = candidate;
                }
            }
        }

        // Verify all types are compatible with the best candidate
        let all_compatible = types
            .iter()
            .all(|t| best.is_assignable_from(t) || t.is_assignable_from(best));

        if all_compatible {
            best.clone()
        } else {
            CelType::Dyn
        }
    }

    /// Check a unary operation.
    fn check_unary(&mut self, op: UnaryOp, inner: &SpannedExpr, expr: &SpannedExpr) -> CelType {
        let inner_type = self.check_expr(inner);

        let func_name = match op {
            UnaryOp::Neg => "-_",
            UnaryOp::Not => "!_",
        };

        self.resolve_function_call(func_name, None, &[inner_type], expr)
    }

    /// Check a binary operation.
    fn check_binary(
        &mut self,
        op: BinaryOp,
        left: &SpannedExpr,
        right: &SpannedExpr,
        expr: &SpannedExpr,
    ) -> CelType {
        let left_type = self.check_expr(left);
        let right_type = self.check_expr(right);

        let func_name = binary_op_to_function(op);

        self.resolve_function_call(func_name, None, &[left_type, right_type], expr)
    }

    /// Check a ternary expression.
    fn check_ternary(
        &mut self,
        cond: &SpannedExpr,
        then_expr: &SpannedExpr,
        else_expr: &SpannedExpr,
        expr: &SpannedExpr,
    ) -> CelType {
        let cond_type = self.check_expr(cond);
        let then_type = self.check_expr(then_expr);
        let else_type = self.check_expr(else_expr);

        // Condition must be bool
        if !matches!(cond_type, CelType::Bool | CelType::Dyn | CelType::Error) {
            self.report_error(CheckError::type_mismatch(
                CelType::Bool,
                cond_type,
                cond.span.clone(),
                cond.id,
            ));
        }

        // Use ternary operator for type resolution
        self.resolve_function_call("_?_:_", None, &[CelType::Bool, then_type, else_type], expr)
    }

    /// Check a member access expression.
    fn check_member(
        &mut self,
        obj: &SpannedExpr,
        field: &str,
        optional: bool,
        expr: &SpannedExpr,
    ) -> CelType {
        // First, try to resolve as qualified identifier (e.g., pkg.Type),
        // but only if the leftmost identifier doesn't resolve locally.
        // This ensures comprehension variables shadow qualified names.
        if !self.leftmost_ident_resolves(obj) {
            if let Some(qualified_name) = self.try_qualified_name(obj, field) {
                // Try variable/type resolution first
                if let Some(decl) = self.resolve_qualified(&qualified_name) {
                    let cel_type = decl.cel_type.clone();
                    self.set_reference(expr.id, ReferenceInfo::ident(&qualified_name));
                    return cel_type;
                }

                // Try proto type resolution (enum values, message types)
                if let Some(resolved) = self.resolve_proto_qualified(&qualified_name, expr) {
                    return resolved;
                }
            }
        }

        // Otherwise, it's a field access
        let obj_type = self.check_expr(obj);

        // Unwrap optional types for field access
        let (inner_type, was_optional) = match &obj_type {
            CelType::Optional(inner) => ((**inner).clone(), true),
            other => (other.clone(), false),
        };

        // Check for well-known types with fields
        let result = match &inner_type {
            CelType::Message(name) => {
                // Try to get field type from proto registry
                if let Some(registry) = self.type_resolver {
                    if let Some(field_type) = registry.get_field_type(name, field) {
                        return self.wrap_optional_if_needed(field_type, optional, was_optional);
                    }
                    // If the message is known but the field doesn't exist, report an error.
                    // Skip extension fields — those are valid but not in the field list.
                    if registry.has_message(name) && !registry.is_extension(name, field) {
                        self.report_error(CheckError::undefined_field(
                            name,
                            field,
                            expr.span.clone(),
                            expr.id,
                        ));
                        return CelType::Error;
                    }
                }
                // Fall back to Dyn if no registry available
                CelType::Dyn
            }
            CelType::Dyn | CelType::TypeVar(_) => {
                // For Dyn and type variables, we can't verify field existence statically
                CelType::Dyn
            }
            CelType::Map(_, value_type) => {
                // Map field access returns the value type
                (**value_type).clone()
            }
            _ => {
                // Other types don't support field access
                self.report_error(CheckError::undefined_field(
                    &inner_type.display_name(),
                    field,
                    expr.span.clone(),
                    expr.id,
                ));
                return CelType::Error;
            }
        };

        self.wrap_optional_if_needed(result, optional, was_optional)
    }

    /// Wrap a type in optional if needed.
    fn wrap_optional_if_needed(
        &self,
        result: CelType,
        optional: bool,
        was_optional: bool,
    ) -> CelType {
        // Wrap in optional if using optional select (.?) or receiver was optional
        // But flatten nested optionals - CEL semantics say chaining doesn't create optional<optional<T>>
        if optional || was_optional {
            match &result {
                CelType::Optional(_) => result, // Already optional, don't double-wrap
                _ => CelType::optional(result),
            }
        } else {
            result
        }
    }

    /// Try to resolve a qualified name as a proto type.
    fn resolve_proto_qualified(
        &mut self,
        qualified_name: &str,
        expr: &SpannedExpr,
    ) -> Option<CelType> {
        let registry = self.type_resolver?;

        // Try to expand abbreviations first
        let expanded_name = self.expand_abbreviation(qualified_name);
        let name_to_resolve = expanded_name.as_deref().unwrap_or(qualified_name);
        let parts: Vec<&str> = name_to_resolve.split('.').collect();

        match registry.resolve_qualified(&parts, self.container)? {
            ResolvedProtoType::EnumValue { enum_name, value } => {
                self.set_reference(
                    expr.id,
                    ReferenceInfo {
                        name: name_to_resolve.to_string(),
                        overload_ids: vec![],
                        value: Some(CelValue::Int(value as i64)),
                        enum_type: Some(enum_name),
                    },
                );
                Some(CelType::Int)
            }
            ResolvedProtoType::Enum { name, cel_type } => {
                self.set_reference(expr.id, ReferenceInfo::ident(&name));
                Some(cel_type)
            }
            ResolvedProtoType::Message { name, cel_type } => {
                self.set_reference(expr.id, ReferenceInfo::ident(&name));
                Some(cel_type)
            }
        }
    }

    /// Try to resolve a call as an enum constructor.
    ///
    /// Handles patterns like `TestAllTypes.NestedEnum(1)` and `GlobalEnum("BAZ")`.
    /// Enum constructors accept either an int or a string argument.
    fn try_enum_constructor(
        &mut self,
        name: &str,
        args: &[SpannedExpr],
        expr: &SpannedExpr,
    ) -> Option<CelType> {
        let registry = self.type_resolver?;

        // Try to expand abbreviations
        let expanded_name = self.expand_abbreviation(name);
        let name_to_resolve = expanded_name.as_deref().unwrap_or(name);
        let parts: Vec<&str> = name_to_resolve.split('.').collect();

        // Check if this resolves to an enum type
        match registry.resolve_qualified(&parts, self.container)? {
            ResolvedProtoType::Enum {
                name: enum_name, ..
            } => {
                // Check args: expect exactly 1 arg that is int or string
                if args.len() != 1 {
                    return None;
                }
                let arg_type = self.check_expr(&args[0]);

                match &arg_type {
                    CelType::Int | CelType::String | CelType::Dyn => {
                        // Store reference so evaluator knows this is an enum constructor
                        self.set_reference(
                            expr.id,
                            ReferenceInfo {
                                name: enum_name.clone(),
                                overload_ids: vec!["enum_constructor".to_string()],
                                value: None,
                                enum_type: Some(enum_name),
                            },
                        );
                        Some(CelType::Int)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Try to expand a name using abbreviations.
    ///
    /// If the first segment of the name matches an abbreviation, the full
    /// qualified name is substituted. For example, if "Foo" is abbreviated
    /// to "my.package.Foo", then "Foo.Bar" becomes "my.package.Foo.Bar".
    fn expand_abbreviation(&self, name: &str) -> Option<String> {
        let abbrevs = self.abbreviations?;

        // Get the first segment of the name
        let first_segment = name.split('.').next()?;

        // Check if it's an abbreviation
        if let Some(qualified) = abbrevs.get(first_segment) {
            // If the name has more segments, append them to the expanded name
            if let Some(rest) = name
                .strip_prefix(first_segment)
                .and_then(|s| s.strip_prefix('.'))
            {
                Some(format!("{}.{}", qualified, rest))
            } else {
                Some(qualified.clone())
            }
        } else {
            None
        }
    }

    /// Check if the leftmost identifier in a member chain resolves in scope.
    ///
    /// Used to skip qualified name resolution when the receiver is a known variable
    /// (e.g., comprehension variables should shadow qualified names).
    fn leftmost_ident_resolves(&self, expr: &SpannedExpr) -> bool {
        match &expr.node {
            Expr::Ident(name) => self.scopes.resolve(name).is_some(),
            Expr::RootIdent(_) => false, // Leading dot always uses qualified resolution
            Expr::Member { expr: inner, .. } => self.leftmost_ident_resolves(inner),
            _ => true, // Non-identifier expressions always evaluate normally
        }
    }

    /// Try to build a qualified name from a member chain.
    fn try_qualified_name(&self, obj: &SpannedExpr, field: &str) -> Option<String> {
        match &obj.node {
            Expr::Ident(name) => Some(format!("{}.{}", name, field)),
            Expr::RootIdent(name) => Some(format!(".{}.{}", name, field)),
            Expr::Member {
                expr: inner,
                field: inner_field,
                ..
            } => {
                let prefix = self.try_qualified_name(inner, inner_field)?;
                Some(format!("{}.{}", prefix, field))
            }
            _ => None,
        }
    }

    /// Try to resolve a qualified name (e.g., `pkg.Type`).
    ///
    /// This checks for the name in the following order:
    /// 1. As-is
    /// 2. Prepended with container
    /// 3. Via abbreviations
    fn resolve_qualified(&self, name: &str) -> Option<&VariableDecl> {
        // Try as-is first
        if let Some(decl) = self.scopes.resolve(name) {
            return Some(decl);
        }

        // Try with container prefix
        if !self.container.is_empty() {
            let qualified = format!("{}.{}", self.container, name);
            if let Some(decl) = self.scopes.resolve(&qualified) {
                return Some(decl);
            }
        }

        // Try abbreviations
        if let Some(abbrevs) = self.abbreviations {
            if let Some(qualified) = abbrevs.get(name) {
                if let Some(decl) = self.scopes.resolve(qualified) {
                    return Some(decl);
                }
            }
        }

        None
    }

    /// Check an index access expression.
    fn check_index(
        &mut self,
        obj: &SpannedExpr,
        index: &SpannedExpr,
        optional: bool,
        expr: &SpannedExpr,
    ) -> CelType {
        let obj_type = self.check_expr(obj);
        let index_type = self.check_expr(index);

        // Unwrap optional types for index access
        let (inner_type, was_optional) = match &obj_type {
            CelType::Optional(inner) => ((**inner).clone(), true),
            other => (other.clone(), false),
        };

        // Resolve index operation on inner type
        let result = self.resolve_function_call("_[_]", None, &[inner_type, index_type], expr);

        // Wrap in optional if using optional index ([?]) or receiver was optional
        // But flatten nested optionals - CEL semantics say chaining doesn't create optional<optional<T>>
        if optional || was_optional {
            match &result {
                CelType::Optional(_) => result, // Already optional, don't double-wrap
                _ => CelType::optional(result),
            }
        } else {
            result
        }
    }

    /// Check a function call expression.
    fn check_call(
        &mut self,
        callee: &SpannedExpr,
        args: &[SpannedExpr],
        expr: &SpannedExpr,
    ) -> CelType {
        // Determine if this is a method call or standalone call
        match &callee.node {
            Expr::Member {
                expr: receiver,
                field: func_name,
                ..
            } => {
                // First, try to resolve as a namespaced function (e.g., math.greatest)
                if let Some(qualified_name) = self.try_qualified_function_name(receiver, func_name)
                {
                    if self.functions.contains_key(&qualified_name) {
                        let arg_types: Vec<_> = args.iter().map(|a| self.check_expr(a)).collect();
                        return self.resolve_function_call(&qualified_name, None, &arg_types, expr);
                    }

                    // Try to resolve as an enum constructor (e.g., TestAllTypes.NestedEnum(1))
                    if let Some(result) = self.try_enum_constructor(&qualified_name, args, expr) {
                        return result;
                    }
                }

                // Fall back to method call: receiver.method(args)
                let receiver_type = self.check_expr(receiver);
                let arg_types: Vec<_> = args.iter().map(|a| self.check_expr(a)).collect();
                self.resolve_function_call(func_name, Some(receiver_type), &arg_types, expr)
            }
            Expr::Ident(func_name) => {
                // Try as enum constructor for top-level enums (e.g., GlobalEnum(1))
                if let Some(result) = self.try_enum_constructor(func_name, args, expr) {
                    return result;
                }

                // Standalone call: func(args)
                let arg_types: Vec<_> = args.iter().map(|a| self.check_expr(a)).collect();
                self.resolve_function_call(func_name, None, &arg_types, expr)
            }
            _ => {
                // Expression call (unusual)
                let _ = self.check_expr(callee);
                for arg in args {
                    self.check_expr(arg);
                }
                CelType::Dyn
            }
        }
    }

    /// Try to build a qualified function name from a member chain.
    ///
    /// This supports namespaced functions like `math.greatest` or `strings.quote`.
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

    /// Resolve a function call and return the result type.
    fn resolve_function_call(
        &mut self,
        name: &str,
        receiver: Option<CelType>,
        args: &[CelType],
        expr: &SpannedExpr,
    ) -> CelType {
        if let Some(func) = self.functions.get(name) {
            let func = func.clone(); // Clone to avoid borrow conflict
            self.resolve_with_function(&func, receiver, args, expr)
        } else {
            self.report_error(CheckError::undeclared_reference(
                name,
                expr.span.clone(),
                expr.id,
            ));
            CelType::Error
        }
    }

    /// Resolve a function call with a known function declaration.
    fn resolve_with_function(
        &mut self,
        func: &FunctionDecl,
        receiver: Option<CelType>,
        args: &[CelType],
        expr: &SpannedExpr,
    ) -> CelType {
        match resolve_overload(func, receiver.as_ref(), args, &mut self.substitutions) {
            Ok(result) => {
                self.set_reference(
                    expr.id,
                    ReferenceInfo::function(&func.name, result.overload_ids),
                );
                result.result_type
            }
            Err(OverloadMismatch::ArityMismatch { expected }) => {
                let got = args.len();
                let msg = format!(
                    "'{}' expects {}, got {}",
                    func.name,
                    format_expected_args(&expected),
                    got
                );
                self.report_error(CheckError::other(msg, expr.span.clone(), expr.id));
                CelType::Error
            }
            Err(_) => {
                let all_args: Vec<_> = receiver
                    .iter()
                    .cloned()
                    .chain(args.iter().cloned())
                    .collect();
                self.report_error(CheckError::no_matching_overload(
                    &func.name,
                    all_args,
                    expr.span.clone(),
                    expr.id,
                ));
                CelType::Error
            }
        }
    }

    /// Check a struct literal expression.
    fn check_struct(
        &mut self,
        type_name: &SpannedExpr,
        fields: &[StructField],
        expr: &SpannedExpr,
    ) -> CelType {
        // Get the type name
        let name = self.get_type_name(type_name);

        // Check field values
        for field in fields {
            self.check_expr(&field.value);
        }

        // Return a message type
        if let Some(ref name) = name {
            // Try to expand abbreviations first
            let expanded_name = self.expand_abbreviation(name);
            let name_to_resolve = expanded_name.as_ref().unwrap_or(name);

            // Try to resolve to fully qualified name using proto registry
            let fq_name = if let Some(registry) = self.type_resolver {
                registry
                    .resolve_message_name(name_to_resolve, self.container)
                    .unwrap_or_else(|| name_to_resolve.clone())
            } else {
                name_to_resolve.clone()
            };

            // Set reference on both the struct expression and the type_name expression
            // The evaluator looks up by type_name.id, so we need it there
            self.set_reference(expr.id, ReferenceInfo::ident(&fq_name));
            self.set_reference(type_name.id, ReferenceInfo::ident(&fq_name));
            CelType::message(&fq_name)
        } else {
            CelType::Dyn
        }
    }

    /// Get the type name from a type expression.
    fn get_type_name(&self, expr: &SpannedExpr) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => Some(name.clone()),
            Expr::RootIdent(name) => Some(format!(".{}", name)),
            Expr::Member {
                expr: inner, field, ..
            } => {
                let prefix = self.get_type_name(inner)?;
                Some(format!("{}.{}", prefix, field))
            }
            _ => None,
        }
    }

    /// Check a comprehension expression.
    fn check_comprehension(&mut self, comp: &ComprehensionData, _expr: &SpannedExpr) -> CelType {
        // Check iter_range in outer scope
        let range_type = self.check_expr(&comp.iter_range);

        // Determine iteration variable type(s) from range
        let (iter_type, iter_type2) = if !comp.iter_var2.is_empty() {
            // Two-variable form: first var is index/key, second is value/element
            match &range_type {
                CelType::List(elem) => (CelType::Int, (**elem).clone()),
                CelType::Map(key, value) => ((**key).clone(), (**value).clone()),
                CelType::Dyn => (CelType::Dyn, CelType::Dyn),
                _ => (CelType::Dyn, CelType::Dyn),
            }
        } else {
            // Single-variable form
            let t = match &range_type {
                CelType::List(elem) => (**elem).clone(),
                CelType::Map(key, _) => (**key).clone(),
                CelType::Optional(inner) => (**inner).clone(), // For optMap/optFlatMap macros
                CelType::Dyn => CelType::Dyn,
                _ => CelType::Dyn,
            };
            (t, CelType::Dyn)
        };

        // Check accu_init in outer scope
        let accu_type = self.check_expr(&comp.accu_init);

        // Enter new scope for comprehension body
        self.scopes.enter_scope();

        // Bind iteration variable(s)
        self.scopes.add_variable(&comp.iter_var, iter_type.clone());
        if !comp.iter_var2.is_empty() {
            self.scopes.add_variable(&comp.iter_var2, iter_type2);
        }

        // Bind accumulator variable
        self.scopes.add_variable(&comp.accu_var, accu_type.clone());

        // Check loop_condition (must be bool)
        let cond_type = self.check_expr(&comp.loop_condition);
        if !matches!(cond_type, CelType::Bool | CelType::Dyn | CelType::Error) {
            self.report_error(CheckError::type_mismatch(
                CelType::Bool,
                cond_type,
                comp.loop_condition.span.clone(),
                comp.loop_condition.id,
            ));
        }

        // Check loop_step (should match accu type)
        let step_type = self.check_expr(&comp.loop_step);

        // If the accumulator was an unresolved type (e.g., empty list from map macro),
        // refine it using the loop step type which has concrete element info.
        if contains_type_var_checker(&accu_type) && !contains_type_var_checker(&step_type) {
            self.scopes.add_variable(&comp.accu_var, step_type);
        }

        // Check result
        let result_type = self.check_expr(&comp.result);

        // Exit comprehension scope
        self.scopes.exit_scope();

        result_type
    }

    /// Check a member test (has() macro result).
    fn check_member_test(&mut self, obj: &SpannedExpr, field: &str, expr: &SpannedExpr) -> CelType {
        // Check the object
        let obj_type = self.check_expr(obj);

        // Unwrap optional types
        let inner_type = match &obj_type {
            CelType::Optional(inner) => (**inner).clone(),
            other => other.clone(),
        };

        // Validate field existence on the receiver type
        match &inner_type {
            CelType::Message(name) => {
                if let Some(registry) = self.type_resolver {
                    if registry.get_field_type(name, field).is_some() {
                        // Field exists — valid
                    } else if registry.has_message(name) && !registry.is_extension(name, field) {
                        self.report_error(CheckError::undefined_field(
                            name,
                            field,
                            expr.span.clone(),
                            expr.id,
                        ));
                    }
                }
            }
            CelType::Map(_, _) | CelType::Dyn | CelType::TypeVar(_) => {
                // Maps, Dyn, and type variables allow any key — no validation
            }
            CelType::Error => {
                // Already an error, don't cascade
            }
            _ => {
                self.report_error(CheckError::undefined_field(
                    &inner_type.display_name(),
                    field,
                    expr.span.clone(),
                    expr.id,
                ));
            }
        }

        // has() always returns bool
        CelType::Bool
    }

    /// Check a bind expression (cel.bind macro result).
    ///
    /// `cel.bind(var, init, body)` binds a variable to a value for use in the body.
    fn check_bind(
        &mut self,
        var_name: &str,
        init: &SpannedExpr,
        body: &SpannedExpr,
        _expr: &SpannedExpr,
    ) -> CelType {
        // Check the initializer expression
        let init_type = self.check_expr(init);

        // Enter a new scope with the bound variable
        self.scopes.enter_scope();
        self.scopes.add_variable(var_name, init_type);

        // Check the body in the new scope
        let body_type = self.check_expr(body);

        // Exit the scope
        self.scopes.exit_scope();

        // The type of the bind expression is the type of the body
        body_type
    }
}

/// Check an expression and return the result.
///
/// This is the main entry point for type checking. It takes raw data rather
/// than a type environment struct, making it independent and reusable.
///
/// # Arguments
/// * `expr` - The expression to type check
/// * `variables` - Variable declarations (name -> type)
/// * `functions` - Function declarations indexed by name
/// * `container` - Container namespace for qualified name resolution
pub fn check(
    expr: &SpannedExpr,
    variables: &HashMap<String, CelType>,
    functions: &HashMap<String, FunctionDecl>,
    container: &str,
) -> CheckResult {
    let checker = Checker::new(variables, functions, container);
    checker.check(expr)
}

/// Check an expression with a type resolver.
///
/// This is like `check`, but also takes a type resolver for resolving
/// protobuf types during type checking.
pub fn check_with_type_resolver(
    expr: &SpannedExpr,
    variables: &HashMap<String, CelType>,
    functions: &HashMap<String, FunctionDecl>,
    container: &str,
    type_resolver: &dyn ProtoTypeResolver,
) -> CheckResult {
    let checker = Checker::new(variables, functions, container).with_type_resolver(type_resolver);
    checker.check(expr)
}

/// Check an expression with abbreviations.
///
/// This is like `check`, but also takes abbreviations for qualified name shortcuts.
pub fn check_with_abbreviations(
    expr: &SpannedExpr,
    variables: &HashMap<String, CelType>,
    functions: &HashMap<String, FunctionDecl>,
    container: &str,
    abbreviations: &HashMap<String, String>,
) -> CheckResult {
    let checker = Checker::new(variables, functions, container).with_abbreviations(abbreviations);
    checker.check(expr)
}

/// Check an expression with a type resolver and abbreviations.
///
/// This is the most complete variant, supporting both a type resolver and abbreviations.
pub fn check_with_type_resolver_and_abbreviations(
    expr: &SpannedExpr,
    variables: &HashMap<String, CelType>,
    functions: &HashMap<String, FunctionDecl>,
    container: &str,
    type_resolver: &dyn ProtoTypeResolver,
    abbreviations: &HashMap<String, String>,
) -> CheckResult {
    let checker = Checker::new(variables, functions, container)
        .with_type_resolver(type_resolver)
        .with_abbreviations(abbreviations);
    checker.check(expr)
}

/// Check if a type contains any TypeVar (for comprehension type refinement).
fn contains_type_var_checker(ty: &CelType) -> bool {
    match ty {
        CelType::TypeVar(_) => true,
        CelType::List(elem) => contains_type_var_checker(elem),
        CelType::Map(key, val) => contains_type_var_checker(key) || contains_type_var_checker(val),
        CelType::Optional(inner) => contains_type_var_checker(inner),
        CelType::Wrapper(inner) => contains_type_var_checker(inner),
        CelType::Type(inner) => contains_type_var_checker(inner),
        _ => false,
    }
}

/// Compute the specificity of a type for join_types ordering.
///
/// Higher values = more specific types that should be preferred.
/// Dyn and TypeVar have specificity 0, concrete types have higher values,
/// and nested/parameterized types get bonus specificity from their inner types.
fn type_specificity(ty: &CelType) -> u32 {
    match ty {
        CelType::Dyn | CelType::TypeVar(_) => 0,
        CelType::Null => 1,
        CelType::Bool
        | CelType::Int
        | CelType::UInt
        | CelType::Double
        | CelType::String
        | CelType::Bytes
        | CelType::Timestamp
        | CelType::Duration => 2,
        CelType::Message(_) | CelType::Enum(_) => 2,
        CelType::List(elem) => 2 + type_specificity(elem),
        CelType::Map(key, val) => 2 + type_specificity(key) + type_specificity(val),
        CelType::Optional(inner) => 2 + type_specificity(inner),
        CelType::Wrapper(inner) => 3 + type_specificity(inner), // Wrappers get extra specificity
        CelType::Type(inner) => 2 + type_specificity(inner),
        CelType::Abstract { params, .. } => 2 + params.iter().map(type_specificity).sum::<u32>(),
        _ => 1,
    }
}

/// Convert a binary operator to its function name.
fn binary_op_to_function(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "_+_",
        BinaryOp::Sub => "_-_",
        BinaryOp::Mul => "_*_",
        BinaryOp::Div => "_/_",
        BinaryOp::Mod => "_%_",
        BinaryOp::Eq => "_==_",
        BinaryOp::Ne => "_!=_",
        BinaryOp::Lt => "_<_",
        BinaryOp::Le => "_<=_",
        BinaryOp::Gt => "_>_",
        BinaryOp::Ge => "_>=_",
        BinaryOp::And => "_&&_",
        BinaryOp::Or => "_||_",
        BinaryOp::In => "@in",
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use super::super::errors::CheckErrorKind;
    use super::super::standard_library::STANDARD_LIBRARY;
    use super::*;
    use crate::eval::proto_registry::ProtoTypeResolver;
    use crate::parser::parse;
    use crate::types::ResolvedProtoType;

    /// Build the standard library functions map.
    fn standard_functions() -> HashMap<String, FunctionDecl> {
        STANDARD_LIBRARY
            .iter()
            .map(|f| (f.name.clone(), f.clone()))
            .collect()
    }

    /// Build the standard type constants.
    fn standard_variables() -> HashMap<String, CelType> {
        let mut vars = HashMap::new();
        vars.insert("bool".to_string(), CelType::type_of(CelType::Bool));
        vars.insert("int".to_string(), CelType::type_of(CelType::Int));
        vars.insert("uint".to_string(), CelType::type_of(CelType::UInt));
        vars.insert("double".to_string(), CelType::type_of(CelType::Double));
        vars.insert("string".to_string(), CelType::type_of(CelType::String));
        vars.insert("bytes".to_string(), CelType::type_of(CelType::Bytes));
        vars.insert(
            "list".to_string(),
            CelType::type_of(CelType::list(CelType::Dyn)),
        );
        vars.insert(
            "map".to_string(),
            CelType::type_of(CelType::map(CelType::Dyn, CelType::Dyn)),
        );
        vars.insert("null_type".to_string(), CelType::type_of(CelType::Null));
        vars.insert(
            "type".to_string(),
            CelType::type_of(CelType::type_of(CelType::Dyn)),
        );
        vars.insert("dyn".to_string(), CelType::type_of(CelType::Dyn));
        vars
    }

    fn check_expr(source: &str) -> CheckResult {
        let result = parse(source);
        let ast = result.ast.expect("parse should succeed");
        let variables = standard_variables();
        let functions = standard_functions();
        check(&ast, &variables, &functions, "")
    }

    fn check_expr_with_var(source: &str, var: &str, cel_type: CelType) -> CheckResult {
        let result = parse(source);
        let ast = result.ast.expect("parse should succeed");
        let mut variables = standard_variables();
        variables.insert(var.to_string(), cel_type);
        let functions = standard_functions();
        check(&ast, &variables, &functions, "")
    }

    /// A minimal ProtoTypeResolver for testing field validation.
    #[derive(Debug)]
    struct MockProtoResolver {
        /// Maps (message_name, field_name) → field type.
        fields: HashMap<(String, String), CelType>,
        /// Known message names.
        messages: Vec<String>,
    }

    impl MockProtoResolver {
        fn new() -> Self {
            Self {
                fields: HashMap::new(),
                messages: Vec::new(),
            }
        }

        fn with_message(mut self, name: &str) -> Self {
            self.messages.push(name.to_string());
            self
        }

        fn with_field(mut self, message: &str, field: &str, cel_type: CelType) -> Self {
            self.fields
                .insert((message.to_string(), field.to_string()), cel_type);
            self
        }
    }

    impl ProtoTypeResolver for MockProtoResolver {
        fn get_field_type(&self, message: &str, field: &str) -> Option<CelType> {
            self.fields
                .get(&(message.to_string(), field.to_string()))
                .cloned()
        }

        fn has_message(&self, message: &str) -> bool {
            self.messages.contains(&message.to_string())
        }

        fn is_extension(&self, _message: &str, _ext_name: &str) -> bool {
            false
        }

        fn get_enum_value(&self, _enum_name: &str, _value_name: &str) -> Option<i32> {
            None
        }

        fn resolve_qualified(
            &self,
            _parts: &[&str],
            _container: &str,
        ) -> Option<ResolvedProtoType> {
            None
        }

        fn resolve_message_name(&self, _name: &str, _container: &str) -> Option<String> {
            None
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn check_expr_with_var_and_resolver(
        source: &str,
        var: &str,
        cel_type: CelType,
        resolver: &dyn ProtoTypeResolver,
    ) -> CheckResult {
        let result = parse(source);
        let ast = result.ast.expect("parse should succeed");
        let mut variables = standard_variables();
        variables.insert(var.to_string(), cel_type);
        let functions = standard_functions();
        check_with_type_resolver(&ast, &variables, &functions, "", resolver)
    }

    #[test]
    fn test_literal_types() {
        assert_eq!(check_expr("null").get_type(1), Some(&CelType::Null));
        assert_eq!(check_expr("true").get_type(1), Some(&CelType::Bool));
        assert_eq!(check_expr("42").get_type(1), Some(&CelType::Int));
        assert_eq!(check_expr("42u").get_type(1), Some(&CelType::UInt));
        assert_eq!(check_expr("3.14").get_type(1), Some(&CelType::Double));
        assert_eq!(check_expr("\"hello\"").get_type(1), Some(&CelType::String));
        assert_eq!(check_expr("b\"hello\"").get_type(1), Some(&CelType::Bytes));
    }

    #[test]
    fn test_undefined_variable() {
        let result = check_expr("x");
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::UndeclaredReference { name, .. } if name == "x"
        )));
    }

    #[test]
    fn test_defined_variable() {
        let result = check_expr_with_var("x", "x", CelType::Int);
        assert!(result.is_ok());
        assert_eq!(result.get_type(1), Some(&CelType::Int));
    }

    #[test]
    fn test_binary_add_int() {
        let result = check_expr_with_var("x + 1", "x", CelType::Int);
        assert!(result.is_ok());
        // The binary expression has ID 3 (after x=1 and 1=2)
        let types: Vec<_> = result.type_map.values().collect();
        assert!(types.contains(&&CelType::Int));
    }

    #[test]
    fn test_list_literal() {
        let result = check_expr("[1, 2, 3]");
        assert!(result.is_ok());
        // Find the list type
        let list_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::List(_)))
            .collect();
        assert_eq!(list_types.len(), 1);
        assert_eq!(list_types[0], &CelType::list(CelType::Int));
    }

    #[test]
    fn test_map_literal() {
        let result = check_expr("{\"a\": 1, \"b\": 2}");
        assert!(result.is_ok());
        // Find the map type
        let map_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::Map(_, _)))
            .collect();
        assert_eq!(map_types.len(), 1);
        assert_eq!(map_types[0], &CelType::map(CelType::String, CelType::Int));
    }

    #[test]
    fn test_comparison() {
        let result = check_expr_with_var("x > 0", "x", CelType::Int);
        assert!(result.is_ok());
        // The result type should be Bool
        let bool_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::Bool))
            .collect();
        assert!(!bool_types.is_empty());
    }

    #[test]
    fn test_method_call() {
        let result = check_expr("\"hello\".contains(\"lo\")");
        assert!(result.is_ok());
        // The result should be Bool
        let bool_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::Bool))
            .collect();
        assert!(!bool_types.is_empty());
    }

    #[test]
    fn test_size_method() {
        let result = check_expr("\"hello\".size()");
        assert!(result.is_ok());
        let int_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::Int))
            .collect();
        assert!(!int_types.is_empty());
    }

    #[test]
    fn test_ternary() {
        let result = check_expr_with_var("x ? 1 : 2", "x", CelType::Bool);
        assert!(result.is_ok());
    }

    #[test]
    fn test_type_mismatch_addition() {
        let result = check_expr_with_var("x + \"str\"", "x", CelType::Int);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::NoMatchingOverload { function, .. } if function == "_+_"
        )));
    }

    #[test]
    fn test_empty_list() {
        let result = check_expr("[]");
        assert!(result.is_ok());
        // Empty list should have a type variable element type that gets finalized to Dyn
        let list_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::List(_)))
            .collect();
        assert_eq!(list_types.len(), 1);
    }

    #[test]
    fn test_reference_recording() {
        let result = check_expr_with_var("x + 1", "x", CelType::Int);
        assert!(result.is_ok());

        // Should have a reference for the identifier
        let refs: Vec<_> = result.reference_map.values().collect();
        assert!(refs.iter().any(|r| r.name == "x"));

        // Should have a reference for the operator
        assert!(refs.iter().any(|r| r.name == "_+_"));
    }

    #[test]
    fn test_has_undefined_field() {
        let resolver =
            MockProtoResolver::new()
                .with_message("Msg")
                .with_field("Msg", "name", CelType::String);
        let result = check_expr_with_var_and_resolver(
            "has(x.nonexistent)",
            "x",
            CelType::Message("Msg".into()),
            &resolver,
        );
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::UndefinedField { field, .. } if field == "nonexistent"
        )));
    }

    #[test]
    fn test_has_valid_field() {
        let resolver =
            MockProtoResolver::new()
                .with_message("Msg")
                .with_field("Msg", "name", CelType::String);
        let result = check_expr_with_var_and_resolver(
            "has(x.name)",
            "x",
            CelType::Message("Msg".into()),
            &resolver,
        );
        assert!(result.is_ok());
        // has() always returns Bool
        let bool_types: Vec<_> = result
            .type_map
            .values()
            .filter(|t| matches!(t, CelType::Bool))
            .collect();
        assert!(!bool_types.is_empty());
    }

    #[test]
    fn test_has_map_field() {
        let result = check_expr_with_var(
            "has(m.anything)",
            "m",
            CelType::map(CelType::String, CelType::Int),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_has_dyn_field() {
        let result = check_expr_with_var("has(d.anything)", "d", CelType::Dyn);
        assert!(result.is_ok());
    }

    #[test]
    fn test_arity_error_size_standalone_no_args() {
        // size() with no args should report arity error, not type mismatch
        let result = check_expr("size()");
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::Other(msg) if msg.contains("'size' expects exactly 1 argument, got 0")
        )));
    }

    #[test]
    fn test_arity_error_method_extra_args() {
        // "hello".size(1) — method expects 0 args, got 1
        let result = check_expr("\"hello\".size(1)");
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::Other(msg) if msg.contains("'size' expects exactly 0 arguments, got 1")
        )));
    }

    #[test]
    fn test_arity_error_contains_no_args() {
        // "hello".contains() — method expects 1 arg, got 0
        let result = check_expr("\"hello\".contains()");
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::Other(msg) if msg.contains("'contains' expects exactly 1 argument, got 0")
        )));
    }

    #[test]
    fn test_type_mismatch_still_reported_for_correct_arity() {
        // size(123) — 1 arg is correct arity but int isn't string/bytes/list/map
        let result = check_expr("size(123)");
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::NoMatchingOverload { function, .. } if function == "size"
        )));
    }

    #[test]
    fn test_operator_type_mismatch_unchanged() {
        // 1 + "a" — arity is always 2 for operators, so this stays a type mismatch
        let result = check_expr_with_var("x + \"a\"", "x", CelType::Int);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::NoMatchingOverload { function, .. } if function == "_+_"
        )));
    }
}
