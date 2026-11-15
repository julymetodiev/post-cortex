// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use crate::tools::operations::{Effect as Operation, OPERATIONS};

use super::*;

/// Each RPC other modules call states the effect its operation has in the
/// table: a read or a search is `read`, a write or a delete `write`.
#[test]
fn every_rpc_has_the_effect_of_its_operation() {
    for (service, method, _, effect) in RPCS {
        let operation = OPERATIONS
            .iter()
            .find(|op| {
                let names = if *service == BLOBS { op.blobs } else { op.grpc };
                names.contains(method)
            })
            .unwrap_or_else(|| panic!("{service}/{method} is no operation of the table"));
        let expected = match operation.effect {
            Operation::Read | Operation::Search { .. } => Effect::Read,
            Operation::Write { .. } | Operation::Delete { .. } => Effect::Write,
        };
        assert_eq!(*effect, expected, "{service}/{method}");
    }
}
