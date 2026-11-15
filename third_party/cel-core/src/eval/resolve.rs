//! Identifier resolution and scope/container logic.

use super::{EvalError, TypeValue, Value};
use crate::types::{Expr, SpannedExpr};

use super::evaluator::Evaluator;

impl<'a> Evaluator<'a> {
    pub(super) fn eval_ident(&self, name: &str, expr: &SpannedExpr) -> Value {
        // Check for type constants
        match name {
            "null_type" => return Value::Type(TypeValue::null_type()),
            "bool" => return Value::Type(TypeValue::bool_type()),
            "int" => return Value::Type(TypeValue::int_type()),
            "uint" => return Value::Type(TypeValue::uint_type()),
            "double" => return Value::Type(TypeValue::double_type()),
            "string" => return Value::Type(TypeValue::string_type()),
            "bytes" => return Value::Type(TypeValue::bytes_type()),
            "list" => return Value::Type(TypeValue::list_type()),
            "map" => return Value::Type(TypeValue::map_type()),
            "type" => return Value::Type(TypeValue::type_type()),
            "optional_type" => return Value::Type(TypeValue::optional_type()),
            _ => {}
        }

        // If we have a reference_map entry with a resolved name, use that
        if let Some(ref_map) = self.reference_map {
            if let Some(ref_info) = ref_map.get(&expr.id) {
                if let Some(v) = self.activation.resolve(&ref_info.name) {
                    return v;
                }
            }
        }

        // Try container-aware resolution (container-prefixed first for shadowing)
        if let Some(v) = self.resolve_with_container(name) {
            return v;
        }

        Value::error(EvalError::unknown_identifier(name))
    }

    /// Resolve a variable name using container namespace prefixing (C++ namespace rules).
    ///
    /// When in a local scope (comprehension, bind), tries as-is first so that
    /// local variables shadow container-prefixed names. At the global level,
    /// container-prefixed names shadow unqualified names per CEL spec.
    pub(super) fn resolve_with_container(&self, name: &str) -> Option<Value> {
        if self.in_local_scope {
            // In local scope: as-is first (local variables shadow container names)
            if let Some(v) = self.activation.resolve(name) {
                return Some(v);
            }
            // Then try container prefixes
            if !self.container.is_empty() {
                let mut container = self.container.as_str();
                loop {
                    let qualified = format!("{}.{}", container, name);
                    if let Some(v) = self.activation.resolve(&qualified) {
                        return Some(v);
                    }
                    match container.rfind('.') {
                        Some(pos) => container = &container[..pos],
                        None => break,
                    }
                }
            }
        } else {
            // At global scope: container-prefixed first (namespace shadowing)
            if !self.container.is_empty() {
                let mut container = self.container.as_str();
                loop {
                    let qualified = format!("{}.{}", container, name);
                    if let Some(v) = self.activation.resolve(&qualified) {
                        return Some(v);
                    }
                    match container.rfind('.') {
                        Some(pos) => container = &container[..pos],
                        None => break,
                    }
                }
            }
            // Then try as-is
            if let Some(v) = self.activation.resolve(name) {
                return Some(v);
            }
        }

        None
    }

    /// Resolve a RootIdent (leading dot, e.g. `.y`).
    ///
    /// Leading dot means "absolute/root namespace" — bypass container prefixing
    /// and local scope, resolve from root activation only.
    pub(super) fn eval_root_ident(&self, name: &str) -> Value {
        self.root_activation
            .resolve(name)
            .unwrap_or_else(|| Value::error(EvalError::unknown_identifier(name)))
    }

    /// Try to build a qualified variable name from a Member chain.
    ///
    /// For `a.b.c`, builds the string "a.b.c".
    /// For `.a.b`, builds the string ".a.b" (leading dot preserved).
    /// Returns None if the chain contains non-identifier nodes.
    pub(super) fn try_qualified_variable_name(
        &self,
        obj: &SpannedExpr,
        field: &str,
    ) -> Option<String> {
        match &obj.node {
            Expr::Ident(name) => Some(format!("{}.{}", name, field)),
            Expr::RootIdent(name) => Some(format!(".{}.{}", name, field)),
            Expr::Member {
                expr: inner,
                field: inner_field,
                optional: false,
            } => {
                let prefix = self.try_qualified_variable_name(inner, inner_field)?;
                Some(format!("{}.{}", prefix, field))
            }
            _ => None,
        }
    }

    /// Check if the leftmost identifier in a member chain resolves in the current activation.
    ///
    /// This is used to determine whether to try qualified name resolution:
    /// if the leftmost ident resolves (e.g., a comprehension variable), we should
    /// use normal field access instead of qualified name lookup.
    pub(super) fn leftmost_ident_resolves(&self, expr: &SpannedExpr) -> bool {
        match &expr.node {
            Expr::Ident(name) => self.activation.resolve(name).is_some(),
            Expr::RootIdent(_) => false, // Leading dot always uses qualified resolution
            Expr::Member { expr: inner, .. } => self.leftmost_ident_resolves(inner),
            _ => true, // Non-identifier expressions always evaluate normally
        }
    }

    /// Try to resolve a qualified name using longest-prefix matching.
    ///
    /// For "a.b.c", tries:
    ///   1. resolve("a.b.c") — full name as variable
    ///   2. resolve("a.b") then field access ".c"
    ///   3. resolve("a") then field access ".b.c"
    ///
    /// When `root_only` is true (for leading-dot chains), resolves against the
    /// root activation without container prefixing.
    pub(super) fn try_longest_prefix_match(
        &self,
        qualified_name: &str,
        root_only: bool,
    ) -> Option<Value> {
        // Handle leading dot prefix
        let name = if let Some(stripped) = qualified_name.strip_prefix('.') {
            stripped
        } else {
            qualified_name
        };

        // Collect all dot positions to try progressively shorter prefixes
        let dots: Vec<usize> = name.match_indices('.').map(|(i, _)| i).collect();

        // Try full name first, then progressively shorter prefixes
        let candidates: Vec<(&str, &str)> = std::iter::once((name, ""))
            .chain(
                dots.iter()
                    .rev()
                    .map(|&pos| (&name[..pos], &name[pos + 1..])),
            )
            .collect();

        for (prefix, remainder) in &candidates {
            let resolved = if root_only {
                // Leading dot: resolve from root activation only, no container prefix
                self.root_activation.resolve(prefix)
            } else {
                self.resolve_with_container(prefix)
            };

            if let Some(mut value) = resolved {
                // Apply remaining field accesses
                if !remainder.is_empty() {
                    for field in remainder.split('.') {
                        if value.is_error() {
                            // Field access failed, try shorter prefix
                            break;
                        }
                        let next = self.access_field(&value, field, false);
                        if next.is_error() {
                            value = next;
                            break;
                        }
                        value = next;
                    }
                    // If field access chain failed, try shorter prefix
                    if value.is_error() {
                        continue;
                    }
                }
                return Some(value);
            }
        }

        // If no activation match found, check if the full qualified name is a known
        // proto message type — this allows expressions like `google.protobuf.Timestamp`
        // to resolve to their type value.
        if let Some(registry) = self.proto_registry {
            if registry
                .resolve_message_name(name, &self.container)
                .is_some()
            {
                return Some(Value::Type(TypeValue::new(name)));
            }
        }

        None
    }

    /// Try to expand a name using abbreviations.
    ///
    /// If the first segment of the name matches an abbreviation, the full
    /// qualified name is substituted.
    pub(super) fn expand_abbreviation(&self, name: &str) -> Option<String> {
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

    /// Resolve a type name expression to a fully qualified name.
    ///
    /// Uses the following resolution order:
    /// 1. Reference map (pre-resolved from type checking)
    /// 2. Abbreviations expansion
    /// 3. Container-based resolution (C++ namespace rules)
    /// 4. Return the unresolved name as last resort
    pub(super) fn resolve_type_name(&self, expr: &SpannedExpr) -> Option<String> {
        // First, check the reference_map for pre-resolved names
        if let Some(ref_map) = self.reference_map {
            if let Some(ref_info) = ref_map.get(&expr.id) {
                return Some(ref_info.name.clone());
            }
        }

        // Fall back to extracting the name from the expression
        let name = self.get_type_name_from_expr(expr)?;

        // Try to expand abbreviations
        let expanded_name = self.expand_abbreviation(&name);
        let name_to_resolve = expanded_name.as_ref().unwrap_or(&name);

        // If we have a proto registry, try to resolve using container
        if let Some(registry) = self.proto_registry {
            // Use container-based resolution (C++ namespace rules)
            if let Some(fq_name) = registry.resolve_message_name(name_to_resolve, &self.container) {
                return Some(fq_name);
            }
        }

        // Return the unresolved name as a last resort
        Some(name_to_resolve.clone())
    }

    /// Extract a type name from an expression (for unresolved cases).
    pub(super) fn get_type_name_from_expr(&self, expr: &SpannedExpr) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) => Some(name.clone()),
            Expr::RootIdent(name) => Some(name.clone()),
            Expr::Member {
                expr: inner, field, ..
            } => {
                let prefix = self.get_type_name_from_expr(inner)?;
                Some(format!("{}.{}", prefix, field))
            }
            _ => None,
        }
    }
}
