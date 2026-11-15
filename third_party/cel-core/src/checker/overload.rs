//! Overload resolution for CEL function calls.
//!
//! This module implements the algorithm for selecting matching function overloads
//! based on argument types, following cel-go's approach.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::types::{CelType, FunctionDecl};

/// Counter for generating unique scope IDs for type parameter scoping.
static SCOPE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Result of overload resolution.
#[derive(Debug)]
pub struct OverloadResult {
    /// The resolved result type.
    pub result_type: CelType,
    /// The matching overload IDs (may be multiple if resolution is ambiguous).
    pub overload_ids: Vec<String>,
}

/// Describes why overload resolution failed.
#[derive(Debug)]
pub enum OverloadMismatch {
    /// No overloads match the call style (all are methods but called standalone, or vice versa).
    NoMatchingCallStyle,
    /// Call style matched but no overloads accept this argument count.
    ArityMismatch {
        /// Sorted, deduplicated arg counts from matching-style overloads.
        expected: Vec<usize>,
    },
    /// At least one overload matched arity but types didn't match.
    TypeMismatch,
}

/// Resolve the best matching overload(s) for a function call.
///
/// # Arguments
///
/// * `func` - The function declaration with all overloads
/// * `target` - The receiver type for method calls (None for standalone calls)
/// * `args` - The argument types
/// * `substitutions` - Mutable map for collecting type parameter bindings
///
/// # Returns
///
/// The overload resolution result, or an `OverloadMismatch` describing why resolution failed.
pub fn resolve_overload(
    func: &FunctionDecl,
    target: Option<&CelType>,
    args: &[CelType],
    substitutions: &mut HashMap<Arc<str>, CelType>,
) -> Result<OverloadResult, OverloadMismatch> {
    let mut matching_overloads = Vec::new();
    let mut result_type = None;
    let mut any_call_style_match = false;
    let mut any_arity_match = false;

    for overload in &func.overloads {
        // Skip if call style doesn't match
        if target.is_some() != overload.is_member {
            continue;
        }

        any_call_style_match = true;

        // Build full argument list (receiver + args for methods)
        let full_args: Vec<&CelType> = if let Some(recv) = target {
            std::iter::once(recv).chain(args.iter()).collect()
        } else {
            args.iter().collect()
        };

        // Check if argument count matches
        if full_args.len() != overload.params.len() {
            continue;
        }

        any_arity_match = true;

        // Try to match this overload with scoped type params
        let mut local_subs = substitutions.clone();
        if let Some(res) = try_match_overload(
            &full_args,
            &overload.params,
            &overload.result,
            &overload.type_params,
            &mut local_subs,
        ) {
            matching_overloads.push(overload);
            result_type = Some(res);

            // Update substitutions (only keep non-scoped bindings)
            *substitutions = local_subs;
        }
    }

    if matching_overloads.is_empty() {
        if !any_call_style_match {
            return Err(OverloadMismatch::NoMatchingCallStyle);
        }
        if !any_arity_match {
            // Collect expected arg counts from overloads matching the call style
            let is_method = target.is_some();
            let mut expected: Vec<usize> = func
                .overloads
                .iter()
                .filter(|o| o.is_member == is_method)
                .map(|o| {
                    if is_method {
                        o.params.len() - 1
                    } else {
                        o.params.len()
                    }
                })
                .collect();
            expected.sort_unstable();
            expected.dedup();
            return Err(OverloadMismatch::ArityMismatch { expected });
        }
        return Err(OverloadMismatch::TypeMismatch);
    }

    // Collect overload IDs
    let overload_ids: Vec<String> = matching_overloads.iter().map(|o| o.id.clone()).collect();

    Ok(OverloadResult {
        result_type: result_type.unwrap_or(CelType::Dyn),
        overload_ids,
    })
}

/// Format expected argument counts for error messages.
pub fn format_expected_args(expected: &[usize]) -> String {
    match expected {
        [n] => format!("exactly {} argument{}", n, if *n == 1 { "" } else { "s" }),
        _ => {
            let parts: Vec<String> = expected.iter().map(|n| n.to_string()).collect();
            format!("{} arguments", parts.join(" or "))
        }
    }
}

/// Rename type params in a type to scoped names using the given rename map.
fn rename_type_params(ty: &CelType, rename_map: &HashMap<String, String>) -> CelType {
    match ty {
        CelType::TypeParam(name) => {
            if let Some(scoped) = rename_map.get(name.as_ref()) {
                CelType::TypeParam(Arc::from(scoped.as_str()))
            } else {
                ty.clone()
            }
        }
        CelType::List(elem) => CelType::list(rename_type_params(elem, rename_map)),
        CelType::Map(key, val) => CelType::map(
            rename_type_params(key, rename_map),
            rename_type_params(val, rename_map),
        ),
        CelType::Type(inner) => CelType::type_of(rename_type_params(inner, rename_map)),
        CelType::Wrapper(inner) => CelType::wrapper(rename_type_params(inner, rename_map)),
        CelType::Optional(inner) => CelType::optional(rename_type_params(inner, rename_map)),
        CelType::Function { params, result } => CelType::Function {
            params: Arc::from(
                params
                    .iter()
                    .map(|p| rename_type_params(p, rename_map))
                    .collect::<Vec<_>>(),
            ),
            result: Arc::new(rename_type_params(result, rename_map)),
        },
        CelType::Abstract { name, params } => CelType::Abstract {
            name: name.clone(),
            params: Arc::from(
                params
                    .iter()
                    .map(|p| rename_type_params(p, rename_map))
                    .collect::<Vec<_>>(),
            ),
        },
        _ => ty.clone(),
    }
}

/// Try to match argument types against parameter types, binding type parameters.
///
/// Returns the resolved result type if the overload matches, None otherwise.
/// Uses scoped type parameter names to avoid cross-expression collision.
fn try_match_overload(
    args: &[&CelType],
    params: &[CelType],
    result: &CelType,
    type_params: &[String],
    substitutions: &mut HashMap<Arc<str>, CelType>,
) -> Option<CelType> {
    // Create scoped names for type parameters to prevent cross-expression collision
    let scope_id = SCOPE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let rename_map: HashMap<String, String> = type_params
        .iter()
        .map(|name| (name.clone(), format!("{}#{}", name, scope_id)))
        .collect();

    // Rename type params in parameter types and result type
    let scoped_params: Vec<CelType> = params
        .iter()
        .map(|p| rename_type_params(p, &rename_map))
        .collect();
    let scoped_result = rename_type_params(result, &rename_map);

    // Check each argument against its (scoped) parameter
    // Don't substitute type params before is_assignable - let the TypeParam branch handle binding
    for (arg, param) in args.iter().zip(scoped_params.iter()) {
        if !is_assignable(arg, param, substitutions) {
            return None;
        }
    }

    // Compute result type with substitutions
    Some(substitute_type(&scoped_result, substitutions))
}

/// Check if an argument type is assignable to a parameter type.
fn is_assignable(
    arg: &CelType,
    param: &CelType,
    substitutions: &mut HashMap<Arc<str>, CelType>,
) -> bool {
    // Type parameters bind to argument types
    if let CelType::TypeParam(name) = param {
        if let Some(bound) = substitutions.get(name).cloned() {
            // Already bound - check compatibility
            if is_types_compatible(&bound, arg, substitutions) {
                // If bound is less specific than arg, widen to the arg type.
                // Null widens to any nullable type, TypeVar/Dyn widen to concrete types.
                if should_widen_binding(&bound, arg) {
                    substitutions.insert(name.clone(), arg.clone());
                }
                return true;
            }
            // Incompatible types widen the param to Dyn
            substitutions.insert(name.clone(), CelType::Dyn);
            return true;
        } else {
            // Bind the type parameter
            // If arg contains TypeVars, bind to Dyn instead (concrete types will widen later)
            if contains_type_var(arg) {
                substitutions.insert(name.clone(), CelType::Dyn);
            } else {
                substitutions.insert(name.clone(), arg.clone());
            }
            return true;
        }
    }

    // Type variables unify with anything
    if matches!(param, CelType::TypeVar(_)) || matches!(arg, CelType::TypeVar(_)) {
        return true;
    }

    // Dyn is compatible with everything
    if matches!(arg, CelType::Dyn) || matches!(param, CelType::Dyn) {
        return true;
    }

    // Error is compatible with everything (for error recovery)
    if matches!(arg, CelType::Error) || matches!(param, CelType::Error) {
        return true;
    }

    // Same type is always assignable
    if arg == param {
        return true;
    }

    // Check structural compatibility
    match (arg, param) {
        (CelType::List(arg_elem), CelType::List(param_elem)) => {
            is_assignable(arg_elem, param_elem, substitutions)
        }
        (CelType::Map(arg_key, arg_val), CelType::Map(param_key, param_val)) => {
            is_assignable(arg_key, param_key, substitutions)
                && is_assignable(arg_val, param_val, substitutions)
        }
        (CelType::Type(arg_inner), CelType::Type(param_inner)) => {
            is_assignable(arg_inner, param_inner, substitutions)
        }
        (CelType::Wrapper(arg_inner), CelType::Wrapper(param_inner)) => {
            is_assignable(arg_inner, param_inner, substitutions)
        }
        // Null is assignable to wrapper types, message, duration, timestamp, optional, abstract
        (CelType::Null, CelType::Wrapper(_)) => true,
        (CelType::Null, CelType::Message(_)) => true,
        (CelType::Null, CelType::Timestamp) => true,
        (CelType::Null, CelType::Duration) => true,
        (CelType::Null, CelType::Optional(_)) => true,
        (CelType::Null, CelType::Abstract { .. }) => true,
        // Enum types are assignable to/from Int
        (CelType::Enum(_), CelType::Int) => true,
        (CelType::Int, CelType::Enum(_)) => true,
        // Underlying type is assignable to wrapper (boxing)
        (inner, CelType::Wrapper(param_inner)) => is_assignable(inner, param_inner, substitutions),
        // Wrapper is assignable to underlying type (unboxing)
        (CelType::Wrapper(arg_inner), inner) => {
            is_assignable(arg_inner.as_ref(), inner, substitutions)
        }
        // Optional types
        (CelType::Optional(arg_inner), CelType::Optional(param_inner)) => {
            is_assignable(arg_inner, param_inner, substitutions)
        }
        // Abstract types - match by name and parameter types
        (
            CelType::Abstract {
                name: arg_name,
                params: arg_params,
            },
            CelType::Abstract {
                name: param_name,
                params: param_params,
            },
        ) => {
            arg_name == param_name
                && arg_params.len() == param_params.len()
                && arg_params
                    .iter()
                    .zip(param_params.iter())
                    .all(|(a, p)| is_assignable(a, p, substitutions))
        }
        _ => false,
    }
}

/// Check if a type contains any TypeVar.
fn contains_type_var(ty: &CelType) -> bool {
    match ty {
        CelType::TypeVar(_) => true,
        CelType::List(elem) => contains_type_var(elem),
        CelType::Map(key, val) => contains_type_var(key) || contains_type_var(val),
        CelType::Optional(inner) => contains_type_var(inner),
        CelType::Wrapper(inner) => contains_type_var(inner),
        CelType::Type(inner) => contains_type_var(inner),
        _ => false,
    }
}

/// Check if a binding should be widened from `bound` to `arg`.
///
/// This implements the cel-go behavior where less specific types
/// (Null, Dyn, types with TypeVars) are replaced by more specific types.
fn should_widen_binding(bound: &CelType, arg: &CelType) -> bool {
    if bound == arg {
        return false;
    }
    // Null should be widened to any non-null type
    if matches!(bound, CelType::Null) && !matches!(arg, CelType::Null) {
        return true;
    }
    // TypeVar/Dyn should be widened to concrete types
    if (matches!(bound, CelType::TypeVar(_)) || matches!(bound, CelType::Dyn))
        && !matches!(arg, CelType::TypeVar(_) | CelType::Dyn)
    {
        return true;
    }
    // Types with TypeVars should be widened to types without
    if contains_type_var(bound) && !contains_type_var(arg) {
        return true;
    }
    false
}

/// Check if two types are compatible (for checking bound type parameters).
fn is_types_compatible(
    bound: &CelType,
    arg: &CelType,
    substitutions: &mut HashMap<Arc<str>, CelType>,
) -> bool {
    // Type variables are compatible with anything
    if matches!(bound, CelType::TypeVar(_)) {
        return true;
    }

    // Try assignability in both directions for type parameter resolution
    is_assignable(arg, bound, substitutions) || is_assignable(bound, arg, &mut HashMap::new())
}

/// Substitute type parameters in a type with their bound values.
pub fn substitute_type(ty: &CelType, substitutions: &HashMap<Arc<str>, CelType>) -> CelType {
    match ty {
        CelType::TypeParam(name) => {
            if let Some(bound) = substitutions.get(name) {
                // Recursively substitute in case the bound type also has parameters
                substitute_type(bound, substitutions)
            } else {
                ty.clone()
            }
        }
        CelType::List(elem) => CelType::list(substitute_type(elem, substitutions)),
        CelType::Map(key, val) => CelType::map(
            substitute_type(key, substitutions),
            substitute_type(val, substitutions),
        ),
        CelType::Type(inner) => CelType::type_of(substitute_type(inner, substitutions)),
        CelType::Wrapper(inner) => CelType::wrapper(substitute_type(inner, substitutions)),
        CelType::Optional(inner) => CelType::optional(substitute_type(inner, substitutions)),
        CelType::Function { params, result } => CelType::Function {
            params: Arc::from(
                params
                    .iter()
                    .map(|p| substitute_type(p, substitutions))
                    .collect::<Vec<_>>(),
            ),
            result: Arc::new(substitute_type(result, substitutions)),
        },
        CelType::Abstract {
            name,
            params: abs_params,
        } => CelType::Abstract {
            name: name.clone(),
            params: Arc::from(
                abs_params
                    .iter()
                    .map(|p| substitute_type(p, substitutions))
                    .collect::<Vec<_>>(),
            ),
        },
        // Other types don't have type parameters
        _ => ty.clone(),
    }
}

/// Finalize type by replacing unbound type variables with Dyn.
pub fn finalize_type(ty: &CelType) -> CelType {
    match ty {
        CelType::TypeVar(_) => CelType::Dyn,
        CelType::TypeParam(_) => CelType::Dyn,
        CelType::List(elem) => CelType::list(finalize_type(elem)),
        CelType::Map(key, val) => CelType::map(finalize_type(key), finalize_type(val)),
        CelType::Type(inner) => CelType::type_of(finalize_type(inner)),
        CelType::Wrapper(inner) => CelType::wrapper(finalize_type(inner)),
        CelType::Optional(inner) => CelType::optional(finalize_type(inner)),
        CelType::Function { params, result } => CelType::Function {
            params: Arc::from(params.iter().map(finalize_type).collect::<Vec<_>>()),
            result: Arc::new(finalize_type(result)),
        },
        CelType::Abstract {
            name,
            params: abs_params,
        } => CelType::Abstract {
            name: name.clone(),
            params: Arc::from(abs_params.iter().map(finalize_type).collect::<Vec<_>>()),
        },
        _ => ty.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OverloadDecl;

    #[test]
    fn test_resolve_simple_overload() {
        let func = FunctionDecl::new("_+_")
            .with_overload(OverloadDecl::function(
                "add_int64_int64",
                vec![CelType::Int, CelType::Int],
                CelType::Int,
            ))
            .with_overload(OverloadDecl::function(
                "add_string_string",
                vec![CelType::String, CelType::String],
                CelType::String,
            ));

        let mut subs = HashMap::new();
        let result = resolve_overload(&func, None, &[CelType::Int, CelType::Int], &mut subs);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.result_type, CelType::Int);
        assert!(result.overload_ids.contains(&"add_int64_int64".to_string()));
    }

    #[test]
    fn test_resolve_method_overload() {
        let func = FunctionDecl::new("contains").with_overload(OverloadDecl::method(
            "string_contains_string",
            vec![CelType::String, CelType::String],
            CelType::Bool,
        ));

        let mut subs = HashMap::new();
        let result = resolve_overload(&func, Some(&CelType::String), &[CelType::String], &mut subs);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.result_type, CelType::Bool);
    }

    #[test]
    fn test_resolve_generic_overload() {
        let func = FunctionDecl::new("_==_").with_overload(
            OverloadDecl::function(
                "equals",
                vec![CelType::type_param("T"), CelType::type_param("T")],
                CelType::Bool,
            )
            .with_type_params(vec!["T".to_string()]),
        );

        let mut subs = HashMap::new();
        let result = resolve_overload(&func, None, &[CelType::Int, CelType::Int], &mut subs);

        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.result_type, CelType::Bool);
    }

    #[test]
    fn test_resolve_no_match_type_mismatch() {
        let func = FunctionDecl::new("_+_").with_overload(OverloadDecl::function(
            "add_int64_int64",
            vec![CelType::Int, CelType::Int],
            CelType::Int,
        ));

        let mut subs = HashMap::new();
        let result = resolve_overload(&func, None, &[CelType::String, CelType::Int], &mut subs);

        assert!(matches!(result, Err(OverloadMismatch::TypeMismatch)));
    }

    #[test]
    fn test_resolve_no_match_arity() {
        let func = FunctionDecl::new("_+_").with_overload(OverloadDecl::function(
            "add_int64_int64",
            vec![CelType::Int, CelType::Int],
            CelType::Int,
        ));

        let mut subs = HashMap::new();
        let result = resolve_overload(&func, None, &[CelType::Int], &mut subs);

        match result {
            Err(OverloadMismatch::ArityMismatch { expected }) => {
                assert_eq!(expected, vec![2]);
            }
            other => panic!("expected ArityMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_no_match_call_style() {
        let func = FunctionDecl::new("contains").with_overload(OverloadDecl::method(
            "string_contains_string",
            vec![CelType::String, CelType::String],
            CelType::Bool,
        ));

        let mut subs = HashMap::new();
        // Call as standalone when only method overloads exist
        let result = resolve_overload(&func, None, &[CelType::String, CelType::String], &mut subs);

        assert!(matches!(result, Err(OverloadMismatch::NoMatchingCallStyle)));
    }

    #[test]
    fn test_substitute_type() {
        let mut subs = HashMap::new();
        subs.insert(Arc::from("T"), CelType::Int);

        let list_t = CelType::list(CelType::type_param("T"));
        let result = substitute_type(&list_t, &subs);

        assert_eq!(result, CelType::list(CelType::Int));
    }

    #[test]
    fn test_finalize_type() {
        let list_var = CelType::list(CelType::fresh_type_var());
        let result = finalize_type(&list_var);

        assert_eq!(result, CelType::list(CelType::Dyn));
    }

    #[test]
    fn test_dyn_compatible() {
        let mut subs = HashMap::new();
        assert!(is_assignable(&CelType::Dyn, &CelType::Int, &mut subs));
        assert!(is_assignable(&CelType::Int, &CelType::Dyn, &mut subs));
    }
}
