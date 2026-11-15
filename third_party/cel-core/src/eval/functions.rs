//! Function implementations and registry for CEL evaluation.
//!
//! This module provides the infrastructure for calling functions during evaluation.
//! Functions are stored with their overloads and implementations, and the evaluator
//! uses the registry to dispatch function calls.

use std::collections::HashMap;
use std::sync::Arc;

use super::Value;

/// A function implementation that takes arguments and returns a value.
///
/// The implementation receives a slice of already-evaluated argument values
/// (including the receiver for member functions as the first argument).
pub type FunctionImpl = Arc<dyn Fn(&[Value]) -> Value + Send + Sync>;

/// A function overload with its implementation.
#[derive(Clone)]
pub struct Overload {
    /// The overload ID (e.g., "add_int64_int64").
    pub id: String,
    /// Whether this is a member function (receiver.method(args)).
    pub is_member: bool,
    /// The number of parameters (including receiver for member functions).
    pub arity: usize,
    /// The implementation function.
    pub implementation: FunctionImpl,
}

impl Overload {
    /// Create a new overload.
    pub fn new(
        id: impl Into<String>,
        is_member: bool,
        arity: usize,
        implementation: FunctionImpl,
    ) -> Self {
        Self {
            id: id.into(),
            is_member,
            arity,
            implementation,
        }
    }

    /// Call this overload with the given arguments.
    pub fn call(&self, args: &[Value]) -> Value {
        (self.implementation)(args)
    }
}

impl std::fmt::Debug for Overload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overload")
            .field("id", &self.id)
            .field("is_member", &self.is_member)
            .field("arity", &self.arity)
            .finish()
    }
}

/// A function with all its overloads.
#[derive(Debug, Clone, Default)]
pub struct Function {
    /// The function name.
    pub name: String,
    /// All overloads for this function.
    pub overloads: Vec<Overload>,
}

impl Function {
    /// Create a new function with no overloads.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            overloads: Vec::new(),
        }
    }

    /// Add an overload to this function.
    pub fn with_overload(mut self, overload: Overload) -> Self {
        self.overloads.push(overload);
        self
    }

    /// Find an overload by ID.
    pub fn find_overload(&self, id: &str) -> Option<&Overload> {
        self.overloads.iter().find(|o| o.id == id)
    }

    /// Find overloads that match the given arity and member status.
    pub fn find_matching_overloads(&self, arity: usize, is_member: bool) -> Vec<&Overload> {
        self.overloads
            .iter()
            .filter(|o| o.arity == arity && o.is_member == is_member)
            .collect()
    }
}

/// Registry of all functions available during evaluation.
///
/// The registry maps function names to their implementations and provides
/// lookup for both standalone and member functions.
#[derive(Debug, Clone, Default)]
pub struct FunctionRegistry {
    functions: HashMap<String, Function>,
}

impl FunctionRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a function with its overloads.
    pub fn register(&mut self, function: Function) {
        self.functions.insert(function.name.clone(), function);
    }

    /// Get a function by name.
    pub fn get(&self, name: &str) -> Option<&Function> {
        self.functions.get(name)
    }

    /// Check if a function exists.
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

    /// Find an overload by function name and overload ID.
    pub fn find_overload(&self, function_name: &str, overload_id: &str) -> Option<&Overload> {
        self.functions
            .get(function_name)
            .and_then(|f| f.find_overload(overload_id))
    }

    /// Find overloads for a function call with the given arity.
    pub fn find_overloads(
        &self,
        function_name: &str,
        arity: usize,
        is_member: bool,
    ) -> Vec<&Overload> {
        self.functions
            .get(function_name)
            .map(|f| f.find_matching_overloads(arity, is_member))
            .unwrap_or_default()
    }

    /// Merge another registry into this one.
    ///
    /// If both registries have a function with the same name, the overloads are merged.
    pub fn merge(&mut self, other: FunctionRegistry) {
        for (name, function) in other.functions {
            if let Some(existing) = self.functions.get_mut(&name) {
                existing.overloads.extend(function.overloads);
            } else {
                self.functions.insert(name, function);
            }
        }
    }

    /// Get the number of registered functions.
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Iterate over all functions.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Function)> {
        self.functions.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_impl(args: &[Value]) -> Value {
        match args.first() {
            Some(Value::Int(i)) => Value::Int(i * 2),
            _ => Value::error("expected int"),
        }
    }

    #[test]
    fn test_overload_call() {
        let overload = Overload::new("test_int", false, 1, Arc::new(test_impl));
        let result = overload.call(&[Value::Int(21)]);
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_function_find_overload() {
        let func = Function::new("test")
            .with_overload(Overload::new("test_int", false, 1, Arc::new(test_impl)))
            .with_overload(Overload::new("test_string", false, 1, Arc::new(test_impl)));

        assert!(func.find_overload("test_int").is_some());
        assert!(func.find_overload("test_string").is_some());
        assert!(func.find_overload("test_bool").is_none());
    }

    #[test]
    fn test_registry() {
        let mut registry = FunctionRegistry::new();

        let func = Function::new("double").with_overload(Overload::new(
            "double_int",
            false,
            1,
            Arc::new(test_impl),
        ));

        registry.register(func);

        assert!(registry.contains("double"));
        assert!(!registry.contains("triple"));

        let overload = registry.find_overload("double", "double_int");
        assert!(overload.is_some());

        let result = overload.unwrap().call(&[Value::Int(21)]);
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_registry_merge() {
        let mut reg1 = FunctionRegistry::new();
        reg1.register(Function::new("f1").with_overload(Overload::new(
            "f1_int",
            false,
            1,
            Arc::new(test_impl),
        )));

        let mut reg2 = FunctionRegistry::new();
        reg2.register(Function::new("f2").with_overload(Overload::new(
            "f2_int",
            false,
            1,
            Arc::new(test_impl),
        )));

        reg1.merge(reg2);

        assert!(reg1.contains("f1"));
        assert!(reg1.contains("f2"));
    }
}
