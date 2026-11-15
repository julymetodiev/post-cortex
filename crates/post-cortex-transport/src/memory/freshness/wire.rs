// Copyright (c) 2026 Julius ML
// MIT License

//! `pcx.v1` freshness messages to and from the freshness port's domain
//! types (`post_cortex_core::core::freshness`), in one place.

use post_cortex_core::core::freshness as domain;

use post_cortex_proto::pb;

/// A registered source reference from the wire. An absent scope, or
/// `whole_file`, is the whole file.
pub fn reference(r: pb::SourceReference) -> domain::SourceReference {
    use pb::source_scope::Scope;
    let scope = match r.scope.and_then(|s| s.scope) {
        None | Some(Scope::WholeFile(_)) => domain::SourceScope::WholeFile,
        Some(Scope::Function(f)) => domain::SourceScope::Function(domain::FunctionScope {
            name: f.name,
            ast_hash: f.ast_hash,
            symbol_type: f.symbol_type,
            imports: f.imports,
        }),
        Some(Scope::Block(b)) => domain::SourceScope::Block(domain::BlockScope {
            start_line: b.start_line,
            end_line: b.end_line,
            content_hash: b.content_hash,
        }),
    };
    domain::SourceReference {
        entry_id: r.entry_id,
        file_path: r.file_path,
        content_hash: r.content_hash,
        captured_at_unix: r.captured_at_unix,
        scope,
    }
}

/// A stored source reference on the wire.
pub fn source_reference(r: domain::SourceReference) -> pb::SourceReference {
    use pb::source_scope::Scope;
    let scope = match r.scope {
        domain::SourceScope::WholeFile => Scope::WholeFile(true),
        domain::SourceScope::Function(f) => Scope::Function(pb::FunctionScope {
            name: f.name,
            ast_hash: f.ast_hash,
            symbol_type: f.symbol_type,
            imports: f.imports,
        }),
        domain::SourceScope::Block(b) => Scope::Block(pb::BlockScope {
            start_line: b.start_line,
            end_line: b.end_line,
            content_hash: b.content_hash,
        }),
    };
    pb::SourceReference {
        file_path: r.file_path,
        content_hash: r.content_hash,
        captured_at_unix: r.captured_at_unix,
        entry_id: r.entry_id,
        scope: Some(pb::SourceScope { scope: Some(scope) }),
    }
}

pub fn symbol(s: pb::SymbolId) -> domain::SymbolId {
    domain::SymbolId {
        file_path: s.file_path,
        symbol_name: s.symbol_name,
        symbol_type: s.symbol_type,
    }
}

pub fn status(s: domain::FreshnessStatus) -> pb::FreshnessStatus {
    match s {
        domain::FreshnessStatus::Fresh => pb::FreshnessStatus::Fresh,
        domain::FreshnessStatus::Stale => pb::FreshnessStatus::Stale,
        domain::FreshnessStatus::SourceMissing => pb::FreshnessStatus::SourceMissing,
        domain::FreshnessStatus::Unknown => pb::FreshnessStatus::Unknown,
    }
}

pub fn entry(e: domain::FreshnessEntry) -> pb::FreshnessEntry {
    pb::FreshnessEntry {
        entry_id: e.entry_id,
        file_path: e.file_path,
        status: status(e.status) as i32,
        stored_hash: e.stored_hash,
        current_hash: e.current_hash,
    }
}

pub fn cascade_report(r: domain::CascadeInvalidateReport) -> pb::CascadeInvalidateReport {
    pb::CascadeInvalidateReport {
        direct_invalidations: r.direct_invalidations,
        cascade_invalidations: r.cascade_invalidations,
        invalidated_symbols: r.invalidated_symbols,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every wire status maps to its own domain status and back.
    #[test]
    fn statuses_map_one_to_one() {
        use domain::FreshnessStatus::*;
        let wire: Vec<pb::FreshnessStatus> = [Fresh, Stale, SourceMissing, Unknown]
            .into_iter()
            .map(status)
            .collect();
        assert_eq!(
            wire,
            [
                pb::FreshnessStatus::Fresh,
                pb::FreshnessStatus::Stale,
                pb::FreshnessStatus::SourceMissing,
                pb::FreshnessStatus::Unknown
            ]
        );
    }

    /// A stored reference goes back on the wire as it came.
    #[test]
    fn references_round_trip() {
        use pb::source_scope::Scope;
        let wire = pb::SourceReference {
            entry_id: "e".into(),
            file_path: "a.rs".into(),
            content_hash: vec![1],
            captured_at_unix: 7,
            scope: Some(pb::SourceScope {
                scope: Some(Scope::Block(pb::BlockScope {
                    start_line: 3,
                    end_line: 9,
                    content_hash: vec![4],
                })),
            }),
        };
        assert_eq!(source_reference(reference(wire.clone())), wire);
    }

    #[test]
    fn scopes_map_from_the_wire() {
        use pb::source_scope::Scope;
        let with = |scope| pb::SourceReference {
            entry_id: "e".into(),
            file_path: "a.rs".into(),
            content_hash: vec![1],
            captured_at_unix: 7,
            scope,
        };
        assert_eq!(reference(with(None)).scope, domain::SourceScope::WholeFile);
        let function = reference(with(Some(pb::SourceScope {
            scope: Some(Scope::Function(pb::FunctionScope {
                name: "foo".into(),
                ast_hash: vec![2],
                symbol_type: "fn".into(),
                imports: vec!["Bar".into()],
            })),
        })));
        assert_eq!(function.function().unwrap().imports, ["Bar"]);
        assert_eq!(function.captured_at_unix, 7);
        let block = reference(with(Some(pb::SourceScope {
            scope: Some(Scope::Block(pb::BlockScope {
                start_line: 3,
                end_line: 9,
                content_hash: vec![4],
            })),
        })));
        assert!(matches!(
            block.scope,
            domain::SourceScope::Block(domain::BlockScope {
                start_line: 3,
                end_line: 9,
                ..
            })
        ));
    }
}
