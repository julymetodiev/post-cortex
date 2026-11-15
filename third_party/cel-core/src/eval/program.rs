//! Compiled CEL program ready for evaluation.
//!
//! A `Program` combines a parsed/checked AST with a function registry,
//! providing a convenient interface for evaluating expressions.

use std::collections::HashMap;
use std::sync::Arc;

use super::proto_registry::ProtoRegistry;
use super::{Activation, Budget, EmptyActivation, Evaluator, FunctionRegistry, References, Value};
use crate::Ast;

/// A compiled CEL program ready for evaluation.
///
/// The program holds a reference to the AST and a function registry,
/// providing a convenient interface for evaluating the expression
/// against different variable bindings.
#[derive(Clone)]
pub struct Program {
    ast: Arc<Ast>,
    functions: Arc<FunctionRegistry>,
    proto_registry: Option<Arc<dyn ProtoRegistry>>,
    abbreviations: Option<Arc<HashMap<String, String>>>,
    strong_enums: bool,
    /// The AST's references, sorted once (pcx patch).
    references: Option<Arc<References>>,
}

/// The references of `ast`, when it was checked, sorted (pcx patch).
fn references_of(ast: &Ast) -> Option<Arc<References>> {
    ast.type_info()
        .map(|info| Arc::new(References::new(&info.reference_map)))
}

impl Program {
    /// Create a new program from an AST and function registry.
    pub fn new(ast: Arc<Ast>, functions: Arc<FunctionRegistry>) -> Self {
        let references = references_of(&ast);
        Self {
            ast,
            functions,
            proto_registry: None,
            abbreviations: None,
            strong_enums: true,
            references,
        }
    }

    /// Create a new program with a type registry.
    pub fn with_proto_registry(
        ast: Arc<Ast>,
        functions: Arc<FunctionRegistry>,
        proto_registry: Arc<dyn ProtoRegistry>,
    ) -> Self {
        let references = references_of(&ast);
        Self {
            ast,
            functions,
            proto_registry: Some(proto_registry),
            abbreviations: None,
            strong_enums: true,
            references,
        }
    }

    /// Create a new program with abbreviations.
    pub fn with_abbreviations(
        ast: Arc<Ast>,
        functions: Arc<FunctionRegistry>,
        abbreviations: HashMap<String, String>,
    ) -> Self {
        let references = references_of(&ast);
        Self {
            ast,
            functions,
            proto_registry: None,
            abbreviations: Some(Arc::new(abbreviations)),
            strong_enums: true,
            references,
        }
    }

    /// Create a new program with a type registry and abbreviations.
    pub fn with_proto_registry_and_abbreviations(
        ast: Arc<Ast>,
        functions: Arc<FunctionRegistry>,
        proto_registry: Arc<dyn ProtoRegistry>,
        abbreviations: HashMap<String, String>,
    ) -> Self {
        let references = references_of(&ast);
        Self {
            ast,
            functions,
            proto_registry: Some(proto_registry),
            abbreviations: Some(Arc::new(abbreviations)),
            strong_enums: true,
            references,
        }
    }

    /// Use legacy (weak) enum mode where enum values are returned as plain integers.
    pub fn with_legacy_enums(mut self) -> Self {
        self.strong_enums = false;
        self
    }

    /// Get the AST for this program.
    pub fn ast(&self) -> &Ast {
        &self.ast
    }

    /// Get the function registry for this program.
    pub fn functions(&self) -> &FunctionRegistry {
        &self.functions
    }

    /// Evaluate the program with the given variable bindings.
    pub fn eval(&self, activation: &dyn Activation) -> Value {
        self.eval_with_container(activation, "")
    }

    /// Evaluate the program with the given variable bindings and container namespace.
    ///
    /// The container is used for resolving unqualified type names following
    /// C++ namespace rules. For example, with container "cel.expr.conformance.proto3"
    /// and type name "TestAllTypes", resolution tries:
    /// 1. cel.expr.conformance.proto3.TestAllTypes
    /// 2. cel.expr.conformance.TestAllTypes
    /// 3. cel.expr.TestAllTypes
    /// 4. cel.TestAllTypes
    /// 5. TestAllTypes
    pub fn eval_with_container(&self, activation: &dyn Activation, container: &str) -> Value {
        self.evaluator(activation, container, None)
            .eval(self.ast.expr())
    }

    /// Evaluate the program with the given variable bindings, taking at
    /// most `budget`'s steps: a spent budget ends it with an error of kind
    /// `BudgetExceeded` (pcx patch).
    pub fn eval_with_budget(&self, activation: &dyn Activation, budget: &Budget) -> Value {
        self.evaluator(activation, "", Some(budget))
            .eval(self.ast.expr())
    }

    /// The evaluator of this program over `activation` in `container`,
    /// within `budget` when one is given.
    fn evaluator<'a>(
        &'a self,
        activation: &'a dyn Activation,
        container: &str,
        budget: Option<&'a Budget>,
    ) -> Evaluator<'a> {
        let mut evaluator = Evaluator::new(activation, &self.functions);

        // Pass type info from checked AST
        if let Some(references) = &self.references {
            evaluator = evaluator.with_reference_map(references);
        }

        // Pass type registry
        if let Some(ref proto_registry) = self.proto_registry {
            evaluator = evaluator.with_proto_registry(proto_registry.as_ref());
        }

        // Set container for type resolution
        if !container.is_empty() {
            evaluator = evaluator.with_container(container);
        }

        // Pass abbreviations
        if let Some(ref abbreviations) = self.abbreviations {
            evaluator = evaluator.with_abbreviations(abbreviations);
        }

        // Pass strong enum setting
        if !self.strong_enums {
            evaluator = evaluator.with_legacy_enums();
        }

        if let Some(budget) = budget {
            evaluator = evaluator.with_budget(budget);
        }
        evaluator
    }

    /// Evaluate the program with no variable bindings.
    pub fn eval_empty(&self) -> Value {
        self.eval(&EmptyActivation)
    }
}

impl std::fmt::Debug for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Program")
            .field("ast", &self.ast)
            .field("functions", &format!("{} functions", self.functions.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::MapActivation;
    use crate::parse;

    fn create_program(source: &str) -> Program {
        let result = parse(source);
        assert!(result.errors.is_empty());
        let ast = Ast::new_unchecked(result.ast.unwrap(), source);
        Program::new(Arc::new(ast), Arc::new(FunctionRegistry::new()))
    }

    #[test]
    fn test_eval_literal() {
        let program = create_program("42");
        assert_eq!(program.eval_empty(), Value::Int(42));
    }

    #[test]
    fn test_eval_with_variables() {
        let program = create_program("x + y");
        let mut activation = MapActivation::new();
        activation.insert("x", Value::Int(1));
        activation.insert("y", Value::Int(2));
        assert_eq!(program.eval(&activation), Value::Int(3));
    }

    #[test]
    fn test_reuse_program() {
        let program = create_program("x * 2");

        let mut act1 = MapActivation::new();
        act1.insert("x", Value::Int(5));
        assert_eq!(program.eval(&act1), Value::Int(10));

        let mut act2 = MapActivation::new();
        act2.insert("x", Value::Int(21));
        assert_eq!(program.eval(&act2), Value::Int(42));
    }
}
