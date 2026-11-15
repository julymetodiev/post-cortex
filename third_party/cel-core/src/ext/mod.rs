//! CEL extension libraries.
//!
//! Extensions provide additional functions beyond the standard library.
//! Each extension returns `Vec<FunctionDecl>` with type declarations.
//!
//! # Available Extensions
//!
//! - **string_ext**: String manipulation functions like `charAt`, `indexOf`, `substring`
//! - **math_ext**: Math functions like `math.greatest`, `math.least`, `math.abs`
//! - **encoders_ext**: Encoding functions like `base64.encode`, `base64.decode`
//! - **optionals_ext**: Optional type functions like `optional.of`, `optional.none`
//!
//! # Example
//!
//! ```ignore
//! use cel_core::Env;
//! use cel_core_common::extensions::{string_extension, math_extension, encoders_extension, optionals_extension};
//!
//! let env = Env::with_standard_library()
//!     .with_extension(string_extension())
//!     .with_extension(math_extension())
//!     .with_extension(encoders_extension())
//!     .with_extension(optionals_extension());
//! ```

mod encoders_ext;
mod math_ext;
mod optionals_ext;
mod string_ext;

pub use encoders_ext::encoders_extension;
pub use math_ext::math_extension;
pub use optionals_ext::optionals_extension;
pub use string_ext::string_extension;
