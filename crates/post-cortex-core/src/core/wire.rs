// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One codec for the names of domain enums.
//!
//! Each enum has one wire name per variant ([`as_str`](crate::core::context_update::UpdateType::as_str),
//! what tools, gRPC and REST read and write) and a stored name (the serde
//! variant name, what older blobs and rows hold). `FromStr` is strict: it
//! inverts `as_str` and accepts the stored name as a legacy alias for data
//! written before the wire names existed. Anything else is an
//! [`UnknownVariant`] error, never a silent default.

use thiserror::Error;

/// A string that names no variant of the enum it was parsed as.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown {kind}: {value:?}")]
pub struct UnknownVariant {
    /// The enum, as a lowercase phrase ("update type")
    pub kind: &'static str,
    /// The string that was given
    pub value: String,
}

/// Implements `ALL`, `as_str`, `stored_name`, `Display` and a strict
/// `FromStr` for a fieldless enum from one table of wire names.
macro_rules! wire_names {
    ($ty:ident, $kind:literal, { $($variant:ident => $wire:literal),+ $(,)? }) => {
        impl $ty {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$ty] = &[$($ty::$variant),+];

            /// Wire name (the name the tools and APIs use).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $($ty::$variant => $wire),+
                }
            }

            /// Stored name: the serde variant name older data holds.
            pub fn stored_name(&self) -> &'static str {
                match self {
                    $($ty::$variant => stringify!($variant)),+
                }
            }
        }

        impl std::fmt::Display for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        /// Strict inverse of `as_str`; the stored (serde) name is accepted
        /// as a legacy alias.
        impl std::str::FromStr for $ty {
            type Err = $crate::core::wire::UnknownVariant;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($wire | stringify!($variant) => Ok($ty::$variant),)+
                    _ => Err($crate::core::wire::UnknownVariant {
                        kind: $kind,
                        value: s.to_string(),
                    }),
                }
            }
        }
    };
}

pub(crate) use wire_names;

#[cfg(test)]
mod tests;
