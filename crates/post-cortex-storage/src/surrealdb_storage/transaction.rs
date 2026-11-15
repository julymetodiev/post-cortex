// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Why a transaction's query failed. When a statement fails, or the commit
//! does, SurrealDB answers every other statement of the transaction with
//! "not executed" and only one row with the cause; `check` gives the first
//! error in order, which is the cause only when the first statement failed.
//! A commit that loses to another writer fails on its own row, the last.

use std::collections::HashMap;

use surrealdb::types::QueryError;
use surrealdb::{Error, IndexedResults};

/// The transaction of `response` committed, or what it failed of
/// ([`cause`]).
///
/// # Errors
/// The cause of the failure.
pub fn committed(mut response: IndexedResults) -> Result<(), Error> {
    cause(response.take_errors()).map_or(Ok(()), Err)
}

/// The cause among the `errors` of a transaction's statements, by statement:
/// the first that is not "not executed", else the last (a commit that
/// failed for another reason than a writer's race says so on its own row,
/// with that kind); none when no statement failed.
pub fn cause(errors: HashMap<usize, Error>) -> Option<Error> {
    let mut errors: Vec<(usize, Error)> = errors.into_iter().collect();
    errors.sort_by_key(|(statement, _)| *statement);
    match errors.iter().position(|(_, e)| !not_executed(e)) {
        Some(at) => Some(errors.swap_remove(at).1),
        None => errors.pop().map(|(_, e)| e),
    }
}

/// Another writer wrote first: the row this one creates exists, or its
/// commit met the other's.
pub fn lost_race(error: &Error) -> bool {
    error.is_already_exists() || error.query_details() == Some(&QueryError::TransactionConflict)
}

fn not_executed(error: &Error) -> bool {
    error.query_details() == Some(&QueryError::NotExecuted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skipped() -> Error {
        Error::query("not executed".into(), Some(QueryError::NotExecuted))
    }

    #[test]
    fn a_commit_that_lost_to_another_writer_is_the_cause() {
        let conflict = Error::query(
            "Cannot COMMIT".into(),
            Some(QueryError::TransactionConflict),
        );
        let found = cause(HashMap::from([
            (0, skipped()),
            (1, skipped()),
            (2, conflict),
        ]));
        assert!(found.as_ref().is_some_and(lost_race), "{found:?}");
    }

    #[test]
    fn the_statement_that_failed_is_the_cause() {
        let failed = Error::query("no such field".into(), None);
        let found = cause(HashMap::from([(0, skipped()), (1, failed), (2, skipped())]));
        assert_eq!(found.map(|e| e.to_string()), Some("no such field".into()));
    }

    #[test]
    fn a_commit_that_failed_otherwise_is_the_last_row() {
        let commit = Error::query("Cannot COMMIT: full".into(), Some(QueryError::NotExecuted));
        let found = cause(HashMap::from([(0, skipped()), (1, commit)]));
        assert_eq!(
            found.as_ref().map(ToString::to_string),
            Some("Cannot COMMIT: full".into())
        );
        assert!(found.is_some_and(|e| !lost_race(&e)));
    }

    #[test]
    fn no_error_is_no_cause() {
        assert!(cause(HashMap::new()).is_none());
    }
}
