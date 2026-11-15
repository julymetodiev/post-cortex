//! Unified environment for CEL expression processing.
//!
//! The `Env` struct coordinates parse, check, and (future) eval operations,
//! following the cel-go architecture pattern.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Error when creating abbreviations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbbrevError {
    /// Two qualified names map to the same short name.
    Conflict {
        short_name: String,
        existing: String,
        new: String,
    },
    /// The qualified name is empty or invalid.
    InvalidName(String),
}

impl fmt::Display for AbbrevError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AbbrevError::Conflict {
                short_name,
                existing,
                new,
            } => {
                write!(
                    f,
                    "abbreviation conflict: '{}' already maps to '{}', cannot map to '{}'",
                    short_name, existing, new
                )
            }
            AbbrevError::InvalidName(name) => {
                write!(f, "invalid qualified name: '{}'", name)
            }
        }
    }
}

impl std::error::Error for AbbrevError {}

/// A validated set of abbreviations mapping short names to fully-qualified names.
///
/// Abbreviations allow short names to be used instead of fully-qualified
/// type names in CEL expressions. This is useful when working with protobuf
/// types from multiple packages.
///
/// # Example
///
/// ```
/// use cel_core::Abbreviations;
///
/// let abbrevs = Abbreviations::new()
///     .with_abbreviation("my.package.Foo").unwrap()     // "Foo" -> "my.package.Foo"
///     .with_abbreviation("other.package.Bar").unwrap(); // "Bar" -> "other.package.Bar"
///
/// assert_eq!(abbrevs.resolve("Foo"), Some("my.package.Foo"));
/// assert_eq!(abbrevs.resolve("Bar"), Some("other.package.Bar"));
/// ```
#[derive(Debug, Clone, Default)]
pub struct Abbreviations {
    map: HashMap<String, String>, // short_name -> fully_qualified
}

impl Abbreviations {
    /// Create an empty abbreviations set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an abbreviation for a qualified name.
    ///
    /// The short name is derived from the last segment of the qualified name.
    /// Returns an error if the short name conflicts with an existing abbreviation.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Abbreviations;
    ///
    /// let abbrevs = Abbreviations::new()
    ///     .with_abbreviation("my.package.Foo").unwrap();
    ///
    /// assert_eq!(abbrevs.resolve("Foo"), Some("my.package.Foo"));
    /// ```
    pub fn with_abbreviation(mut self, qualified_name: &str) -> Result<Self, AbbrevError> {
        if qualified_name.is_empty() {
            return Err(AbbrevError::InvalidName(qualified_name.to_string()));
        }

        let short_name = qualified_name
            .rsplit('.')
            .next()
            .unwrap_or(qualified_name)
            .to_string();

        if short_name.is_empty() {
            return Err(AbbrevError::InvalidName(qualified_name.to_string()));
        }

        if let Some(existing) = self.map.get(&short_name) {
            if existing != qualified_name {
                return Err(AbbrevError::Conflict {
                    short_name,
                    existing: existing.clone(),
                    new: qualified_name.to_string(),
                });
            }
            // Same mapping already exists - idempotent, no error
        } else {
            self.map.insert(short_name, qualified_name.to_string());
        }

        Ok(self)
    }

    /// Create abbreviations from a slice of qualified names.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Abbreviations;
    ///
    /// let abbrevs = Abbreviations::from_qualified_names(&[
    ///     "my.package.Foo",
    ///     "other.package.Bar",
    /// ]).unwrap();
    /// ```
    pub fn from_qualified_names(names: &[&str]) -> Result<Self, AbbrevError> {
        let mut abbrevs = Self::new();
        for name in names {
            abbrevs = abbrevs.with_abbreviation(name)?;
        }
        Ok(abbrevs)
    }

    /// Get the underlying map of short names to fully-qualified names.
    pub fn as_map(&self) -> &HashMap<String, String> {
        &self.map
    }

    /// Resolve a short name to its fully qualified name.
    ///
    /// Returns `None` if the short name is not in the abbreviations set.
    pub fn resolve(&self, short_name: &str) -> Option<&str> {
        self.map.get(short_name).map(|s| s.as_str())
    }

    /// Check if the abbreviations set is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Get the number of abbreviations.
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

use crate::ast::Ast;
use crate::checker::{
    check, check_with_abbreviations, check_with_type_resolver,
    check_with_type_resolver_and_abbreviations, CheckError, CheckResult, STANDARD_LIBRARY,
};
use crate::eval::{Function, FunctionRegistry, Overload, Program, ProtoRegistry};
use crate::ext;
use crate::parser::{self, ParseError, ParseResult};
use crate::types::{CelType, FunctionDecl, SpannedExpr};

/// Error from compiling a CEL expression.
#[derive(Debug, Clone)]
pub enum CompileError {
    /// Parse errors occurred.
    Parse(Vec<ParseError>),
    /// Type checking errors occurred.
    Check(Vec<CheckError>),
    /// No AST was produced (should not happen normally).
    NoAst,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Parse(errors) => {
                write!(f, "parse errors: ")?;
                for (i, e) in errors.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}", e.message)?;
                }
                Ok(())
            }
            CompileError::Check(errors) => {
                write!(f, "check errors: ")?;
                for (i, e) in errors.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}", e.message())?;
                }
                Ok(())
            }
            CompileError::NoAst => write!(f, "no AST produced"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Unified environment for CEL expression processing.
///
/// The `Env` wraps the parser and checker, providing a high-level API
/// for working with CEL expressions. It manages:
/// - Variable declarations with their types
/// - Function declarations (standard library + extensions)
/// - Container namespace for qualified name resolution
/// - Abbreviations for type name shortcuts
///
/// # Example
///
/// ```
/// use cel_core::Env;
/// use cel_core::CelType;
///
/// let env = Env::with_standard_library()
///     .with_variable("x", CelType::Int);
///
/// let result = env.compile("x + 1");
/// assert!(result.is_ok());
/// ```
#[derive(Debug, Clone)]
pub struct Env {
    /// Variable declarations (name -> type).
    variables: HashMap<String, CelType>,
    /// Function declarations indexed by name.
    functions: HashMap<String, FunctionDecl>,
    /// Container namespace for qualified name resolution.
    container: String,
    /// Type registry for resolving protobuf types.
    proto_registry: Option<Arc<dyn ProtoRegistry>>,
    /// Abbreviations for qualified name shortcuts.
    abbreviations: Abbreviations,
    /// Whether to use strong enum typing (default: true).
    /// When false, enum values are returned as plain integers (legacy behavior).
    strong_enums: bool,
}

impl Env {
    /// Create a new empty environment.
    ///
    /// This environment has no functions or variables defined.
    /// Use `with_standard_library()` for a fully-featured environment.
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            functions: HashMap::new(),
            container: String::new(),
            proto_registry: None,
            abbreviations: Abbreviations::new(),
            strong_enums: true,
        }
    }

    /// Create a new environment with the CEL standard library.
    ///
    /// This includes all standard operators, functions, and type constants.
    pub fn with_standard_library() -> Self {
        let mut env = Self::new();

        // Add standard library functions
        for func in STANDARD_LIBRARY.iter() {
            env.functions.insert(func.name.clone(), func.clone());
        }

        // Add type constants (type values for type checking)
        env.add_type_constant("bool", CelType::Bool);
        env.add_type_constant("int", CelType::Int);
        env.add_type_constant("uint", CelType::UInt);
        env.add_type_constant("double", CelType::Double);
        env.add_type_constant("string", CelType::String);
        env.add_type_constant("bytes", CelType::Bytes);
        env.add_type_constant("list", CelType::list(CelType::Dyn));
        env.add_type_constant("map", CelType::map(CelType::Dyn, CelType::Dyn));
        env.add_type_constant("null_type", CelType::Null);
        env.add_type_constant("type", CelType::type_of(CelType::Dyn));
        env.add_type_constant("dyn", CelType::Dyn);

        env
    }

    /// Add a type constant to the environment.
    fn add_type_constant(&mut self, name: &str, cel_type: CelType) {
        self.variables
            .insert(name.to_string(), CelType::type_of(cel_type));
    }

    /// Add a variable to the environment (builder pattern).
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Env;
    /// use cel_core::CelType;
    ///
    /// let env = Env::with_standard_library()
    ///     .with_variable("x", CelType::Int)
    ///     .with_variable("y", CelType::String);
    /// ```
    pub fn with_variable(mut self, name: impl Into<String>, cel_type: CelType) -> Self {
        self.variables.insert(name.into(), cel_type);
        self
    }

    /// Add a variable to the environment (mutable).
    pub fn add_variable(&mut self, name: impl Into<String>, cel_type: CelType) {
        self.variables.insert(name.into(), cel_type);
    }

    /// Add a function declaration to the environment (builder pattern).
    pub fn with_function(mut self, decl: FunctionDecl) -> Self {
        self.add_function(decl);
        self
    }

    /// Add a function declaration to the environment (mutable).
    ///
    /// If a function with the same name already exists, overloads are merged.
    pub fn add_function(&mut self, decl: FunctionDecl) {
        if let Some(existing) = self.functions.get_mut(&decl.name) {
            // Merge overloads
            existing.overloads.extend(decl.overloads);
        } else {
            self.functions.insert(decl.name.clone(), decl);
        }
    }

    /// Set the container namespace (builder pattern).
    ///
    /// The container is used for qualified name resolution.
    pub fn with_container(mut self, container: impl Into<String>) -> Self {
        self.container = container.into();
        self
    }

    /// Set the container namespace (mutable).
    pub fn set_container(&mut self, container: impl Into<String>) {
        self.container = container.into();
    }

    /// Get the container namespace.
    pub fn container(&self) -> &str {
        &self.container
    }

    /// Set the proto registry (builder pattern).
    ///
    /// The proto registry is used for resolving protobuf types during type checking
    /// and evaluation. Pass an `Arc<dyn ProtoRegistry>` implementation such as
    /// `ProstProtoRegistry` from `cel-core-proto`.
    pub fn with_proto_registry(mut self, registry: Arc<dyn ProtoRegistry>) -> Self {
        self.proto_registry = Some(registry);
        self
    }

    /// Get the proto registry.
    pub fn proto_registry(&self) -> Option<&dyn ProtoRegistry> {
        self.proto_registry.as_ref().map(|r| r.as_ref())
    }

    /// Set abbreviations for qualified name resolution (builder pattern).
    ///
    /// Abbreviations allow short names to be used instead of fully-qualified
    /// type names in expressions. This is useful when working with protobuf
    /// types from multiple packages.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::{Env, Abbreviations};
    ///
    /// let abbrevs = Abbreviations::new()
    ///     .with_abbreviation("google.protobuf.Duration").unwrap();
    ///
    /// let env = Env::with_standard_library()
    ///     .with_abbreviations(abbrevs);
    /// ```
    pub fn with_abbreviations(mut self, abbreviations: Abbreviations) -> Self {
        self.abbreviations = abbreviations;
        self
    }

    /// Get the abbreviations.
    pub fn abbreviations(&self) -> &Abbreviations {
        &self.abbreviations
    }

    /// Use legacy (weak) enum mode where enum values are returned as plain integers.
    ///
    /// By default, strong enum typing is enabled and enum values carry their
    /// fully-qualified type name. In legacy mode, enum values are represented
    /// as `Value::Int`, matching older CEL behavior.
    pub fn with_legacy_enums(mut self) -> Self {
        self.strong_enums = false;
        self
    }

    /// Check if strong enum typing is enabled.
    pub fn strong_enums(&self) -> bool {
        self.strong_enums
    }

    /// Add an extension library to the environment (builder pattern).
    ///
    /// Extensions provide additional functions beyond the standard library.
    /// Each extension is a collection of `FunctionDecl` values.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Env;
    /// use cel_core::ext::string_extension;
    ///
    /// let env = Env::with_standard_library()
    ///     .with_extension(string_extension());
    /// ```
    pub fn with_extension(mut self, extension: impl IntoIterator<Item = FunctionDecl>) -> Self {
        for decl in extension {
            self.add_function(decl);
        }
        self
    }

    /// Add all available extension libraries to the environment (builder pattern).
    ///
    /// This is a convenience method that adds all standard extensions:
    /// - String extension (`charAt`, `indexOf`, `substring`, etc.)
    /// - Math extension (`math.greatest`, `math.least`, `math.abs`, etc.)
    /// - Encoders extension (`base64.encode`, `base64.decode`)
    /// - Optionals extension (`optional.of`, `optional.none`, `hasValue`, etc.)
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Env;
    ///
    /// let env = Env::with_standard_library()
    ///     .with_all_extensions();
    /// ```
    pub fn with_all_extensions(mut self) -> Self {
        self = self
            .with_extension(ext::string_extension())
            .with_extension(ext::math_extension())
            .with_extension(ext::encoders_extension())
            .with_extension(ext::optionals_extension());

        // Add optional_type type constant for the optionals extension
        self.add_type_constant("optional_type", CelType::optional(CelType::Dyn));

        self
    }

    /// Get the variables map.
    pub fn variables(&self) -> &HashMap<String, CelType> {
        &self.variables
    }

    /// Get the functions map.
    pub fn functions(&self) -> &HashMap<String, FunctionDecl> {
        &self.functions
    }

    /// Get all member functions compatible with a given receiver type.
    ///
    /// Returns (function_name, overload) pairs where at least one member overload
    /// has a receiver type that is assignable from the given type.
    /// Handles generic type parameters (e.g., `list(T)`) by treating them as wildcards.
    pub fn methods_for_type(&self, receiver: &CelType) -> Vec<(&str, &crate::types::OverloadDecl)> {
        let mut results = Vec::new();
        for func in self.functions.values() {
            if !func.has_member_overloads() {
                continue;
            }
            for overload in &func.overloads {
                if !overload.is_member {
                    continue;
                }
                if let Some(recv_type) = overload.receiver_type() {
                    if type_compatible_for_completion(recv_type, receiver) {
                        results.push((func.name.as_str(), overload));
                        break; // One match per function is enough
                    }
                }
            }
        }
        results
    }

    /// Get all standalone (non-operator) function names.
    ///
    /// Filters out internal operator functions (like `_+_`, `_-_`) and other
    /// internal functions that start with `_` or contain `@`.
    pub fn standalone_functions(&self) -> Vec<&str> {
        self.functions
            .values()
            .filter(|f| {
                f.has_standalone_overloads() && !f.name.starts_with('_') && !f.name.contains('@')
            })
            .map(|f| f.name.as_str())
            .collect()
    }

    /// Parse a CEL expression.
    ///
    /// This delegates to the parser. The returned `ParseResult` may contain
    /// both a partial AST and errors if parsing partially succeeded.
    pub fn parse(&self, source: &str) -> ParseResult {
        parser::parse(source)
    }

    /// Type-check a parsed expression.
    ///
    /// This delegates to the checker with the environment's variables,
    /// functions, container, and abbreviations.
    pub fn check(&self, expr: &SpannedExpr) -> CheckResult {
        let has_proto_registry = self.proto_registry.is_some();
        let has_abbreviations = !self.abbreviations.is_empty();

        match (has_proto_registry, has_abbreviations) {
            (true, true) => check_with_type_resolver_and_abbreviations(
                expr,
                &self.variables,
                &self.functions,
                &self.container,
                self.proto_registry.as_ref().unwrap().as_ref(),
                self.abbreviations.as_map(),
            ),
            (true, false) => check_with_type_resolver(
                expr,
                &self.variables,
                &self.functions,
                &self.container,
                self.proto_registry.as_ref().unwrap().as_ref(),
            ),
            (false, true) => check_with_abbreviations(
                expr,
                &self.variables,
                &self.functions,
                &self.container,
                self.abbreviations.as_map(),
            ),
            (false, false) => check(expr, &self.variables, &self.functions, &self.container),
        }
    }

    /// Parse and type-check a CEL expression, returning a checked Ast.
    ///
    /// This is the primary entry point for compiling CEL expressions.
    /// Returns a checked `Ast` that can be used for evaluation.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Env;
    /// use cel_core::CelType;
    ///
    /// let env = Env::with_standard_library()
    ///     .with_variable("x", CelType::Int);
    ///
    /// let ast = env.compile("x + 1").unwrap();
    /// assert!(ast.is_checked());
    /// assert_eq!(ast.result_type(), Some(&CelType::Int));
    /// ```
    pub fn compile(&self, source: &str) -> Result<Ast, CompileError> {
        let parse_result = self.parse(source);

        if !parse_result.errors.is_empty() {
            return Err(CompileError::Parse(parse_result.errors));
        }

        let expr = parse_result.ast.ok_or(CompileError::NoAst)?;
        let check_result = self.check(&expr);

        if !check_result.errors.is_empty() {
            return Err(CompileError::Check(check_result.errors));
        }

        Ok(Ast::new_checked(expr, source, check_result))
    }

    /// Parse a CEL expression without type-checking, returning an unchecked Ast.
    ///
    /// This is useful when you want to parse an expression but don't need
    /// type information, or when you want to defer type-checking.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::Env;
    ///
    /// let env = Env::with_standard_library();
    ///
    /// let ast = env.parse_only("1 + 2").unwrap();
    /// assert!(!ast.is_checked());
    /// assert_eq!(ast.to_cel_string(), "1 + 2");
    /// ```
    pub fn parse_only(&self, source: &str) -> Result<Ast, CompileError> {
        let parse_result = self.parse(source);

        if !parse_result.errors.is_empty() {
            return Err(CompileError::Parse(parse_result.errors));
        }

        let expr = parse_result.ast.ok_or(CompileError::NoAst)?;
        Ok(Ast::new_unchecked(expr, source))
    }

    /// Create a program from a compiled AST.
    ///
    /// The program contains the AST and a function registry with implementations
    /// for all functions registered in this environment.
    ///
    /// # Example
    ///
    /// ```
    /// use cel_core::{Env, CelType};
    /// use cel_core::{Value, MapActivation, Activation};
    ///
    /// let env = Env::with_standard_library()
    ///     .with_variable("x", CelType::Int);
    ///
    /// let ast = env.compile("x + 1").unwrap();
    /// let program = env.program(&ast).unwrap();
    ///
    /// let mut activation = MapActivation::new();
    /// activation.insert("x", Value::Int(41));
    ///
    /// let result = program.eval(&activation);
    /// assert_eq!(result, Value::Int(42));
    /// ```
    pub fn program(&self, ast: &Ast) -> Result<Program, CompileError> {
        let registry = self.build_function_registry();
        let has_proto_registry = self.proto_registry.is_some();
        let has_abbreviations = !self.abbreviations.is_empty();

        let mut program = match (has_proto_registry, has_abbreviations) {
            (true, true) => Program::with_proto_registry_and_abbreviations(
                Arc::new(ast.clone()),
                Arc::new(registry),
                Arc::clone(self.proto_registry.as_ref().unwrap()),
                self.abbreviations.as_map().clone(),
            ),
            (true, false) => Program::with_proto_registry(
                Arc::new(ast.clone()),
                Arc::new(registry),
                Arc::clone(self.proto_registry.as_ref().unwrap()),
            ),
            (false, true) => Program::with_abbreviations(
                Arc::new(ast.clone()),
                Arc::new(registry),
                self.abbreviations.as_map().clone(),
            ),
            (false, false) => Program::new(Arc::new(ast.clone()), Arc::new(registry)),
        };

        if !self.strong_enums {
            program = program.with_legacy_enums();
        }

        Ok(program)
    }

    /// Build the function registry from this environment's function declarations.
    fn build_function_registry(&self) -> FunctionRegistry {
        let mut registry = FunctionRegistry::new();

        for func_decl in self.functions.values() {
            let mut function = Function::new(&func_decl.name);

            for overload_decl in &func_decl.overloads {
                // Only add overloads that have implementations
                if let Some(ref impl_fn) = overload_decl.implementation {
                    let overload = Overload::new(
                        &overload_decl.id,
                        overload_decl.is_member,
                        overload_decl.params.len(),
                        impl_fn.clone(),
                    );
                    function = function.with_overload(overload);
                }
            }

            // Only register functions that have at least one implemented overload
            if !function.overloads.is_empty() {
                registry.register(function);
            }
        }

        registry
    }
}

/// Check if a declared type (which may contain TypeParams) is compatible
/// with a concrete receiver type for completion purposes.
///
/// This treats TypeParam as a wildcard that matches any type, unlike
/// `is_assignable_from` which requires explicit substitution.
fn type_compatible_for_completion(declared: &CelType, concrete: &CelType) -> bool {
    // Regular assignability covers most cases
    if declared.is_assignable_from(concrete) {
        return true;
    }
    match (declared, concrete) {
        // TypeParam matches anything (it's a generic placeholder)
        (CelType::TypeParam(_), _) | (_, CelType::TypeParam(_)) => true,
        // Recurse into list element types
        (CelType::List(d), CelType::List(c)) => type_compatible_for_completion(d, c),
        // Recurse into map key/value types
        (CelType::Map(dk, dv), CelType::Map(ck, cv)) => {
            type_compatible_for_completion(dk, ck) && type_compatible_for_completion(dv, cv)
        }
        // Recurse into optional inner types
        (CelType::Optional(d), CelType::Optional(c)) => type_compatible_for_completion(d, c),
        _ => false,
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::CheckErrorKind;
    use crate::types::OverloadDecl;

    #[test]
    fn test_new_env() {
        let env = Env::new();
        assert!(env.variables().is_empty());
        assert!(env.functions().is_empty());
    }

    #[test]
    fn test_with_standard_library() {
        let env = Env::with_standard_library();

        // Should have standard functions
        assert!(env.functions().contains_key("_+_"));
        assert!(env.functions().contains_key("size"));
        assert!(env.functions().contains_key("contains"));

        // Should have type constants
        assert!(env.variables().contains_key("bool"));
        assert!(env.variables().contains_key("int"));
    }

    #[test]
    fn test_with_variable() {
        let env = Env::with_standard_library().with_variable("x", CelType::Int);

        assert!(env.variables().contains_key("x"));
        assert_eq!(env.variables().get("x"), Some(&CelType::Int));
    }

    #[test]
    fn test_parse() {
        let env = Env::new();
        let result = env.parse("1 + 2");

        assert!(result.ast.is_some());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_check() {
        let env = Env::with_standard_library().with_variable("x", CelType::Int);

        let parse_result = env.parse("x + 1");
        let ast = parse_result.ast.unwrap();

        let check_result = env.check(&ast);
        assert!(check_result.is_ok());
    }

    #[test]
    fn test_check_undefined_variable() {
        let env = Env::with_standard_library();

        let parse_result = env.parse("x + 1");
        let ast = parse_result.ast.unwrap();

        let check_result = env.check(&ast);
        assert!(!check_result.is_ok());
        assert!(check_result.errors.iter().any(|e| matches!(
            &e.kind,
            CheckErrorKind::UndeclaredReference { name, .. } if name == "x"
        )));
    }

    #[test]
    fn test_compile_success() {
        let env = Env::with_standard_library().with_variable("x", CelType::Int);

        let ast = env.compile("x + 1").unwrap();
        assert!(ast.is_checked());
    }

    #[test]
    fn test_compile_parse_error() {
        let env = Env::with_standard_library();

        let result = env.compile("1 +");
        assert!(result.is_err());
    }

    #[test]
    fn test_container() {
        let env = Env::with_standard_library().with_container("google.protobuf");

        assert_eq!(env.container(), "google.protobuf");
    }

    #[test]
    fn test_add_function() {
        let mut env = Env::new();

        let func = FunctionDecl::new("custom").with_overload(OverloadDecl::function(
            "custom_int",
            vec![CelType::Int],
            CelType::Bool,
        ));

        env.add_function(func);

        assert!(env.functions().contains_key("custom"));
    }

    #[test]
    fn test_with_extension() {
        let env = Env::with_standard_library().with_extension(ext::string_extension());

        // String extension should add charAt function
        assert!(env.functions().contains_key("charAt"));
        assert!(env.functions().contains_key("indexOf"));
        assert!(env.functions().contains_key("substring"));
    }

    #[test]
    fn test_with_all_extensions() {
        let env = Env::with_standard_library().with_all_extensions();

        // String extension functions
        assert!(env.functions().contains_key("charAt"));
        assert!(env.functions().contains_key("indexOf"));
        assert!(env.functions().contains_key("join"));
        assert!(env.functions().contains_key("strings.quote"));

        // Math extension functions
        assert!(env.functions().contains_key("math.greatest"));
        assert!(env.functions().contains_key("math.least"));
        assert!(env.functions().contains_key("math.abs"));
        assert!(env.functions().contains_key("math.bitAnd"));

        // Encoders extension functions
        assert!(env.functions().contains_key("base64.encode"));
        assert!(env.functions().contains_key("base64.decode"));

        // Optionals extension functions
        assert!(env.functions().contains_key("optional.of"));
        assert!(env.functions().contains_key("optional.none"));
        assert!(env.functions().contains_key("optional.ofNonZeroValue"));
        assert!(env.functions().contains_key("hasValue"));
        assert!(env.functions().contains_key("value"));
        assert!(env.functions().contains_key("or"));
        assert!(env.functions().contains_key("orValue"));
    }

    #[test]
    fn test_encoders_extension_base64() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("data", CelType::Bytes)
            .with_variable("encoded", CelType::String);

        // base64.encode(bytes) -> string
        let result = env.compile("base64.encode(data)");
        assert!(result.is_ok(), "base64.encode should compile: {:?}", result);

        // base64.decode(string) -> bytes
        let result = env.compile("base64.decode(encoded)");
        assert!(result.is_ok(), "base64.decode should compile: {:?}", result);
    }

    #[test]
    fn test_string_extension_char_at() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("s", CelType::String);

        let result = env.compile("s.charAt(0)");
        assert!(result.is_ok(), "charAt should compile: {:?}", result);
    }

    #[test]
    fn test_string_extension_index_of() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("s", CelType::String);

        // Basic indexOf
        assert!(env.compile("s.indexOf(\"a\")").is_ok());

        // indexOf with offset
        assert!(env.compile("s.indexOf(\"a\", 2)").is_ok());
    }

    #[test]
    fn test_string_extension_substring() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("s", CelType::String);

        // substring with one arg
        assert!(env.compile("s.substring(1)").is_ok());

        // substring with two args
        assert!(env.compile("s.substring(1, 5)").is_ok());
    }

    #[test]
    fn test_string_extension_split() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("s", CelType::String);

        let ast = env.compile("s.split(\",\")").unwrap();
        assert!(ast.is_checked());

        // Result should be list<string>
        // The root expr type should be list<string>
    }

    #[test]
    fn test_string_extension_join() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("parts", CelType::list(CelType::String));

        // join without separator
        assert!(env.compile("parts.join()").is_ok());

        // join with separator
        assert!(env.compile("parts.join(\",\")").is_ok());
    }

    #[test]
    fn test_math_extension_greatest() {
        let env = Env::with_standard_library().with_all_extensions();

        // Binary
        assert!(env.compile("math.greatest(1, 2)").is_ok());

        // Ternary
        assert!(env.compile("math.greatest(1, 2, 3)").is_ok());

        // With list
        assert!(env.compile("math.greatest([1, 2, 3])").is_ok());
    }

    #[test]
    fn test_math_extension_least() {
        let env = Env::with_standard_library().with_all_extensions();
        assert!(env.compile("math.least(1, 2)").is_ok());
    }

    #[test]
    fn test_math_extension_abs() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("x", CelType::Int);

        assert!(env.compile("math.abs(x)").is_ok());
    }

    #[test]
    fn test_math_extension_bit_operations() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("a", CelType::Int)
            .with_variable("b", CelType::Int);

        assert!(env.compile("math.bitAnd(a, b)").is_ok());
        assert!(env.compile("math.bitOr(a, b)").is_ok());
        assert!(env.compile("math.bitNot(a)").is_ok());
    }

    #[test]
    fn test_optionals_extension_of_and_none() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("x", CelType::Int);

        // optional.of(x)
        let result = env.compile("optional.of(x)");
        assert!(result.is_ok(), "optional.of should compile: {:?}", result);

        // optional.none()
        let result = env.compile("optional.none()");
        assert!(result.is_ok(), "optional.none should compile: {:?}", result);

        // optional.ofNonZeroValue(x)
        let result = env.compile("optional.ofNonZeroValue(x)");
        assert!(
            result.is_ok(),
            "optional.ofNonZeroValue should compile: {:?}",
            result
        );
    }

    #[test]
    fn test_cel_bind_macro() {
        let env = Env::with_standard_library().with_all_extensions();

        // cel.bind(x, 10, x + 1) should work - bind x to 10, return x + 1
        let result = env.compile("cel.bind(x, 10, x + 1)");
        assert!(result.is_ok(), "cel.bind should compile: {:?}", result);

        // cel.bind with string
        let result = env.compile("cel.bind(msg, \"hello\", msg + msg)");
        assert!(
            result.is_ok(),
            "cel.bind with string should compile: {:?}",
            result
        );

        // Nested cel.bind
        let result = env.compile("cel.bind(x, 1, cel.bind(y, 2, x + y))");
        assert!(
            result.is_ok(),
            "nested cel.bind should compile: {:?}",
            result
        );
    }

    #[test]
    fn test_optionals_extension_methods() {
        let env = Env::with_standard_library()
            .with_all_extensions()
            .with_variable("opt", CelType::optional(CelType::Int))
            .with_variable("opt2", CelType::optional(CelType::Int));

        // opt.hasValue()
        let result = env.compile("opt.hasValue()");
        assert!(result.is_ok(), "hasValue should compile: {:?}", result);

        // opt.value()
        let result = env.compile("opt.value()");
        assert!(result.is_ok(), "value should compile: {:?}", result);

        // opt.or(opt2)
        let result = env.compile("opt.or(opt2)");
        assert!(result.is_ok(), "or should compile: {:?}", result);

        // opt.orValue(42)
        let result = env.compile("opt.orValue(42)");
        assert!(result.is_ok(), "orValue should compile: {:?}", result);
    }

    // =====================================================================
    // Abbreviations tests
    // =====================================================================

    #[test]
    fn test_abbreviations_new() {
        let abbrevs = Abbreviations::new();
        assert!(abbrevs.is_empty());
        assert_eq!(abbrevs.len(), 0);
    }

    #[test]
    fn test_abbreviations_add() {
        let abbrevs = Abbreviations::new()
            .with_abbreviation("my.package.Foo")
            .unwrap();

        assert_eq!(abbrevs.resolve("Foo"), Some("my.package.Foo"));
        assert_eq!(abbrevs.len(), 1);
    }

    #[test]
    fn test_abbreviations_multiple() {
        let abbrevs = Abbreviations::new()
            .with_abbreviation("my.package.Foo")
            .unwrap()
            .with_abbreviation("other.package.Bar")
            .unwrap();

        assert_eq!(abbrevs.resolve("Foo"), Some("my.package.Foo"));
        assert_eq!(abbrevs.resolve("Bar"), Some("other.package.Bar"));
        assert_eq!(abbrevs.len(), 2);
    }

    #[test]
    fn test_abbreviations_unqualified_name() {
        // Single-segment name is valid
        let abbrevs = Abbreviations::new().with_abbreviation("Foo").unwrap();
        assert_eq!(abbrevs.resolve("Foo"), Some("Foo"));
    }

    #[test]
    fn test_abbreviations_conflict() {
        let result = Abbreviations::new()
            .with_abbreviation("my.package.Foo")
            .unwrap()
            .with_abbreviation("other.package.Foo"); // Same short name!

        assert!(result.is_err());
        match result {
            Err(AbbrevError::Conflict {
                short_name,
                existing,
                new,
            }) => {
                assert_eq!(short_name, "Foo");
                assert_eq!(existing, "my.package.Foo");
                assert_eq!(new, "other.package.Foo");
            }
            _ => panic!("Expected Conflict error"),
        }
    }

    #[test]
    fn test_abbreviations_idempotent() {
        // Adding the same mapping twice is OK (idempotent)
        let abbrevs = Abbreviations::new()
            .with_abbreviation("my.package.Foo")
            .unwrap()
            .with_abbreviation("my.package.Foo") // Same mapping again
            .unwrap();

        assert_eq!(abbrevs.resolve("Foo"), Some("my.package.Foo"));
        assert_eq!(abbrevs.len(), 1);
    }

    #[test]
    fn test_abbreviations_empty_name() {
        let result = Abbreviations::new().with_abbreviation("");
        assert!(result.is_err());
        match result {
            Err(AbbrevError::InvalidName(name)) => {
                assert_eq!(name, "");
            }
            _ => panic!("Expected InvalidName error"),
        }
    }

    #[test]
    fn test_abbreviations_trailing_dot() {
        // A name ending with a dot has an empty last segment
        let result = Abbreviations::new().with_abbreviation("my.package.");
        assert!(result.is_err());
        match result {
            Err(AbbrevError::InvalidName(_)) => {}
            _ => panic!("Expected InvalidName error"),
        }
    }

    #[test]
    fn test_abbreviations_from_qualified_names() {
        let abbrevs =
            Abbreviations::from_qualified_names(&["my.package.Foo", "other.package.Bar"]).unwrap();

        assert_eq!(abbrevs.resolve("Foo"), Some("my.package.Foo"));
        assert_eq!(abbrevs.resolve("Bar"), Some("other.package.Bar"));
    }

    #[test]
    fn test_abbreviations_resolve_nonexistent() {
        let abbrevs = Abbreviations::new()
            .with_abbreviation("my.package.Foo")
            .unwrap();

        assert_eq!(abbrevs.resolve("Bar"), None);
    }

    #[test]
    fn test_methods_for_type_string() {
        let env = Env::with_standard_library();
        let methods = env.methods_for_type(&CelType::String);
        let names: Vec<&str> = methods.iter().map(|(name, _)| *name).collect();
        assert!(names.contains(&"contains"));
        assert!(names.contains(&"startsWith"));
        assert!(names.contains(&"endsWith"));
        assert!(names.contains(&"matches"));
        assert!(names.contains(&"size"));
        // Operators should not appear
        assert!(!names.contains(&"_+_"));
        assert!(!names.contains(&"_==_"));
    }

    #[test]
    fn test_methods_for_type_list() {
        let env = Env::with_standard_library();
        let methods = env.methods_for_type(&CelType::list(CelType::Int));
        let names: Vec<&str> = methods.iter().map(|(name, _)| *name).collect();
        assert!(names.contains(&"size"));
    }

    #[test]
    fn test_standalone_functions() {
        let env = Env::with_standard_library();
        let funcs = env.standalone_functions();
        assert!(funcs.contains(&"size"));
        assert!(funcs.contains(&"int"));
        assert!(funcs.contains(&"string"));
        assert!(funcs.contains(&"bool"));
        // Operators should not appear
        assert!(!funcs.contains(&"_+_"));
        assert!(!funcs.contains(&"_-_"));
        assert!(!funcs.contains(&"_==_"));
    }

    #[test]
    fn test_env_with_abbreviations() {
        let abbrevs = Abbreviations::new()
            .with_abbreviation("my.package.Foo")
            .unwrap();

        let env = Env::with_standard_library().with_abbreviations(abbrevs);

        assert!(!env.abbreviations().is_empty());
        assert_eq!(env.abbreviations().resolve("Foo"), Some("my.package.Foo"));
    }

    #[test]
    fn test_abbrev_error_display() {
        let err = AbbrevError::Conflict {
            short_name: "Foo".to_string(),
            existing: "a.Foo".to_string(),
            new: "b.Foo".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "abbreviation conflict: 'Foo' already maps to 'a.Foo', cannot map to 'b.Foo'"
        );

        let err = AbbrevError::InvalidName("".to_string());
        assert_eq!(err.to_string(), "invalid qualified name: ''");
    }
}
