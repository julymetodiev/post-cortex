//! Variable bindings for CEL evaluation.
//!
//! The `Activation` trait provides a way to resolve variable names to values
//! during expression evaluation. Different implementations support various
//! use cases like simple maps, hierarchical scopes, and lazy evaluation.

use std::collections::HashMap;
use std::sync::Arc;

use super::Value;

/// Trait for resolving variable bindings during evaluation.
///
/// An activation provides the values for variables referenced in CEL expressions.
/// Implementations can support simple key-value lookup, hierarchical scopes,
/// or lazy evaluation.
pub trait Activation: Send + Sync {
    /// Resolve a variable name to its value.
    ///
    /// Returns `None` if the variable is not defined in this activation.
    fn resolve(&self, name: &str) -> Option<Value>;

    /// Check if a variable is defined (present check for `has()` macro).
    ///
    /// Default implementation returns true if `resolve()` returns Some.
    fn has(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }
}

/// A simple activation backed by a HashMap.
#[derive(Debug, Clone, Default)]
pub struct MapActivation {
    bindings: HashMap<String, Value>,
}

impl MapActivation {
    /// Create a new empty activation.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a binding.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<Value>) {
        self.bindings.insert(name.into(), value.into());
    }

    /// Remove a binding.
    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.bindings.remove(name)
    }

    /// Get the number of bindings.
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

impl Activation for MapActivation {
    fn resolve(&self, name: &str) -> Option<Value> {
        self.bindings.get(name).cloned()
    }

    fn has(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }
}

impl FromIterator<(String, Value)> for MapActivation {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        Self {
            bindings: iter.into_iter().collect(),
        }
    }
}

/// A hierarchical activation that delegates to a parent if not found locally.
///
/// This is useful for implementing variable scopes in comprehensions
/// where iteration variables shadow outer variables.
pub struct HierarchicalActivation<'a> {
    parent: &'a dyn Activation,
    local: HashMap<String, Value>,
}

impl<'a> HierarchicalActivation<'a> {
    /// Create a new hierarchical activation with a parent.
    pub fn new(parent: &'a dyn Activation) -> Self {
        Self {
            parent,
            local: HashMap::new(),
        }
    }

    /// Add a local binding that shadows the parent.
    pub fn with_binding(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.local.insert(name.into(), value.into());
        self
    }

    /// Insert a local binding.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<Value>) {
        self.local.insert(name.into(), value.into());
    }

    /// Remove a local binding.
    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.local.remove(name)
    }
}

impl Activation for HierarchicalActivation<'_> {
    fn resolve(&self, name: &str) -> Option<Value> {
        // Check local bindings first, then delegate to parent
        self.local
            .get(name)
            .cloned()
            .or_else(|| self.parent.resolve(name))
    }

    fn has(&self, name: &str) -> bool {
        self.local.contains_key(name) || self.parent.has(name)
    }
}

/// An empty activation with no bindings.
///
/// Useful as a default or when no variables are needed.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyActivation;

impl EmptyActivation {
    /// Create a new empty activation.
    pub fn new() -> Self {
        Self
    }
}

impl Activation for EmptyActivation {
    fn resolve(&self, _name: &str) -> Option<Value> {
        None
    }

    fn has(&self, _name: &str) -> bool {
        false
    }
}

/// An activation that wraps an Arc for shared ownership.
#[derive(Clone)]
pub struct SharedActivation {
    inner: Arc<dyn Activation>,
}

impl std::fmt::Debug for SharedActivation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedActivation").finish_non_exhaustive()
    }
}

impl SharedActivation {
    /// Create a new shared activation.
    pub fn new(activation: impl Activation + 'static) -> Self {
        Self {
            inner: Arc::new(activation),
        }
    }
}

impl Activation for SharedActivation {
    fn resolve(&self, name: &str) -> Option<Value> {
        self.inner.resolve(name)
    }

    fn has(&self, name: &str) -> bool {
        self.inner.has(name)
    }
}

impl<T: Activation> Activation for Arc<T> {
    fn resolve(&self, name: &str) -> Option<Value> {
        (**self).resolve(name)
    }

    fn has(&self, name: &str) -> bool {
        (**self).has(name)
    }
}

impl<T: Activation> Activation for Box<T> {
    fn resolve(&self, name: &str) -> Option<Value> {
        (**self).resolve(name)
    }

    fn has(&self, name: &str) -> bool {
        (**self).has(name)
    }
}

impl<T: Activation + ?Sized> Activation for &T {
    fn resolve(&self, name: &str) -> Option<Value> {
        (**self).resolve(name)
    }

    fn has(&self, name: &str) -> bool {
        (**self).has(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_activation() {
        let mut activation = MapActivation::new();
        activation.insert("x", 42i64);
        activation.insert("name", "hello");

        assert_eq!(activation.resolve("x"), Some(Value::Int(42)));
        assert_eq!(activation.resolve("name"), Some(Value::from("hello")));
        assert_eq!(activation.resolve("unknown"), None);

        assert!(activation.has("x"));
        assert!(!activation.has("unknown"));
    }

    #[test]
    fn test_hierarchical_activation() {
        let parent: MapActivation = [
            ("x".to_string(), Value::Int(1)),
            ("y".to_string(), Value::Int(2)),
        ]
        .into_iter()
        .collect();

        let child = HierarchicalActivation::new(&parent).with_binding("x", 10i64);

        // Local binding shadows parent
        assert_eq!(child.resolve("x"), Some(Value::Int(10)));
        // Parent binding is accessible
        assert_eq!(child.resolve("y"), Some(Value::Int(2)));
        // Unknown still returns None
        assert_eq!(child.resolve("z"), None);
    }

    #[test]
    fn test_empty_activation() {
        let activation = EmptyActivation::new();
        assert_eq!(activation.resolve("anything"), None);
        assert!(!activation.has("anything"));
    }

    #[test]
    fn test_activation_insert_without_suffix() {
        // This is the ergonomic improvement - no i64 suffix needed
        let mut activation = MapActivation::new();
        activation.insert("count", 42); // i32 default works now
        activation.insert("small", 5i8);
        activation.insert("medium", 1000i16);
        activation.insert("len", vec![1u8, 2, 3].len()); // usize from .len()

        assert_eq!(activation.resolve("count"), Some(Value::Int(42)));
        assert_eq!(activation.resolve("small"), Some(Value::Int(5)));
        assert_eq!(activation.resolve("medium"), Some(Value::Int(1000)));
        assert_eq!(activation.resolve("len"), Some(Value::UInt(3)));
    }
}
