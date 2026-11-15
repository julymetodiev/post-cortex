//! Scope management for variable resolution during type checking.
//!
//! Scopes are used to track variable bindings, especially for comprehensions
//! where iteration variables shadow outer variables.

use std::collections::HashMap;

use crate::types::{CelType, VariableDecl};

/// A scope containing variable declarations.
///
/// Scopes form a stack during type checking, with inner scopes
/// (e.g., inside comprehensions) shadowing outer bindings.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Variables declared in this scope.
    variables: HashMap<String, VariableDecl>,
}

impl Scope {
    /// Create a new empty scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a variable to this scope.
    pub fn add_variable(&mut self, name: impl Into<String>, cel_type: CelType) {
        let name = name.into();
        self.variables
            .insert(name.clone(), VariableDecl::new(name, cel_type));
    }

    /// Add a variable declaration to this scope.
    #[allow(dead_code)]
    pub fn add_decl(&mut self, decl: VariableDecl) {
        self.variables.insert(decl.name.clone(), decl);
    }

    /// Look up a variable in this scope only.
    pub fn get(&self, name: &str) -> Option<&VariableDecl> {
        self.variables.get(name)
    }

    /// Check if this scope contains a variable.
    #[allow(dead_code)]
    pub fn contains(&self, name: &str) -> bool {
        self.variables.contains_key(name)
    }
}

/// A stack of scopes for nested variable resolution.
///
/// Variables are resolved from innermost to outermost scope,
/// allowing inner scopes to shadow outer bindings.
#[derive(Debug)]
pub struct ScopeStack {
    /// The scope stack (innermost scope is last).
    scopes: Vec<Scope>,
}

impl ScopeStack {
    /// Create a new scope stack with a single empty scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::new()],
        }
    }

    /// Push a new scope onto the stack.
    pub fn enter_scope(&mut self) {
        self.scopes.push(Scope::new());
    }

    /// Pop the current scope from the stack.
    ///
    /// Returns the popped scope, or None if this is the last scope.
    pub fn exit_scope(&mut self) -> Option<Scope> {
        if self.scopes.len() > 1 {
            self.scopes.pop()
        } else {
            None
        }
    }

    /// Get the current (innermost) scope.
    #[allow(dead_code)]
    pub fn current(&self) -> &Scope {
        self.scopes
            .last()
            .expect("scope stack should never be empty")
    }

    /// Get the current (innermost) scope mutably.
    pub fn current_mut(&mut self) -> &mut Scope {
        self.scopes
            .last_mut()
            .expect("scope stack should never be empty")
    }

    /// Add a variable to the current scope.
    pub fn add_variable(&mut self, name: impl Into<String>, cel_type: CelType) {
        self.current_mut().add_variable(name, cel_type);
    }

    /// Add a variable declaration to the current scope.
    #[allow(dead_code)]
    pub fn add_decl(&mut self, decl: VariableDecl) {
        self.current_mut().add_decl(decl);
    }

    /// Look up a variable, searching from innermost to outermost scope.
    pub fn resolve(&self, name: &str) -> Option<&VariableDecl> {
        for scope in self.scopes.iter().rev() {
            if let Some(decl) = scope.get(name) {
                return Some(decl);
            }
        }
        None
    }

    /// Look up a variable only in local (non-root) scopes.
    ///
    /// This checks comprehension and bind scopes but NOT the root scope.
    /// Returns true if the name is a local variable that should shadow
    /// container-prefixed names.
    pub fn is_local(&self, name: &str) -> bool {
        // Check all scopes except the root (index 0)
        for scope in self.scopes[1..].iter().rev() {
            if scope.get(name).is_some() {
                return true;
            }
        }
        false
    }

    /// Check if a variable is declared in any scope.
    #[allow(dead_code)]
    pub fn contains(&self, name: &str) -> bool {
        self.resolve(name).is_some()
    }

    /// Get the depth of the scope stack.
    #[allow(dead_code)]
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_add_and_get() {
        let mut scope = Scope::new();
        scope.add_variable("x", CelType::Int);

        let decl = scope.get("x").unwrap();
        assert_eq!(decl.name, "x");
        assert_eq!(decl.cel_type, CelType::Int);
    }

    #[test]
    fn test_scope_stack_resolve() {
        let mut stack = ScopeStack::new();
        stack.add_variable("x", CelType::Int);

        let decl = stack.resolve("x").unwrap();
        assert_eq!(decl.cel_type, CelType::Int);
    }

    #[test]
    fn test_scope_stack_shadowing() {
        let mut stack = ScopeStack::new();
        stack.add_variable("x", CelType::Int);

        // Enter inner scope
        stack.enter_scope();
        stack.add_variable("x", CelType::String); // Shadow x

        // Inner scope should see String
        let decl = stack.resolve("x").unwrap();
        assert_eq!(decl.cel_type, CelType::String);

        // Exit inner scope
        stack.exit_scope();

        // Outer scope should see Int again
        let decl = stack.resolve("x").unwrap();
        assert_eq!(decl.cel_type, CelType::Int);
    }

    #[test]
    fn test_scope_stack_cannot_pop_last() {
        let mut stack = ScopeStack::new();
        assert!(stack.exit_scope().is_none());
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn test_scope_stack_nested() {
        let mut stack = ScopeStack::new();
        stack.add_variable("a", CelType::Int);

        stack.enter_scope();
        stack.add_variable("b", CelType::String);

        stack.enter_scope();
        stack.add_variable("c", CelType::Bool);

        // All three should be visible
        assert!(stack.resolve("a").is_some());
        assert!(stack.resolve("b").is_some());
        assert!(stack.resolve("c").is_some());

        stack.exit_scope();
        assert!(stack.resolve("a").is_some());
        assert!(stack.resolve("b").is_some());
        assert!(stack.resolve("c").is_none());

        stack.exit_scope();
        assert!(stack.resolve("a").is_some());
        assert!(stack.resolve("b").is_none());
        assert!(stack.resolve("c").is_none());
    }
}
