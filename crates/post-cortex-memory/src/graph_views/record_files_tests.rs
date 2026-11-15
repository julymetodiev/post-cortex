// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The files a record names, from the texts agents actually wrote.

use crate::graph_views::record_files::paths_in;

fn paths(text: &str) -> Vec<String> {
    paths_in(text)
}

#[test]
fn a_code_reference_with_a_symbol_after_it_names_the_file() {
    assert_eq!(
        paths("crates/post-cortex-storage/src/export_import.rs merge_card / preview_import"),
        vec!["crates/post-cortex-storage/src/export_import.rs"]
    );
    assert_eq!(
        paths("crates/post-cortex-memory/src/content_vectorizer/ingestion.rs vectorize_messages"),
        vec!["crates/post-cortex-memory/src/content_vectorizer/ingestion.rs"]
    );
    assert_eq!(paths("src/main.rs:42"), vec!["src/main.rs"]);
    assert_eq!(paths("src/main.rs:10-20"), vec!["src/main.rs"]);
}

#[test]
fn several_files_in_one_text() {
    assert_eq!(
        paths(
            "post-cortex-cloud/crates/pcx-platform/src/lib.rs get_radar, radar_dismissals; src/explore.rs Scope::radar"
        ),
        vec![
            "post-cortex-cloud/crates/pcx-platform/src/lib.rs",
            "src/explore.rs"
        ]
    );
}

#[test]
fn brace_lists_expand_against_their_prefix() {
    assert_eq!(
        paths(
            "crates/post-cortex-daemon/src/bin/pcx/{project.rs (new), setup.rs, cli.rs}, templates/{pcx_resolve.py (new), settings.json}"
        ),
        vec![
            "crates/post-cortex-daemon/src/bin/pcx/project.rs",
            "crates/post-cortex-daemon/src/bin/pcx/setup.rs",
            "crates/post-cortex-daemon/src/bin/pcx/cli.rs",
            "templates/pcx_resolve.py",
            "templates/settings.json",
        ]
    );
    assert_eq!(
        paths("app/lib/data/{models,repositories}.dart"),
        vec!["app/lib/data/models.dart", "app/lib/data/repositories.dart"]
    );
    assert_eq!(
        paths("lib/{fake/{fake_backend,sample_data}.dart, state/providers.dart}"),
        vec![
            "lib/fake/fake_backend.dart",
            "lib/fake/sample_data.dart",
            "lib/state/providers.dart"
        ]
    );
}

#[test]
fn prose_is_not_a_file() {
    assert!(
        paths("post-cortex-cloud 6c2fd41 Committed: the gateway routes pcx.module.v1.Host")
            .is_empty()
    );
    assert!(paths("Code functionality updated").is_empty());
    assert!(paths("W8/W9 and pcx-control/src").is_empty());
    assert!(paths("see https://example.com/a.html").is_empty());
    assert!(paths("pcx.views.v1, pcx.account.v1").is_empty());
    assert!(paths("").is_empty());
    assert!(paths("{unbalanced, list").is_empty());
}

#[test]
fn bare_and_absolute_names_stay() {
    assert_eq!(paths("README.md, Makefile"), vec!["README.md", "Makefile"]);
    assert_eq!(
        paths("/srv/projects/hermes-plugin-post-cortex/shared_config.py"),
        vec!["/srv/projects/hermes-plugin-post-cortex/shared_config.py"]
    );
    assert_eq!(
        paths("`docs/ARCHITECTURE.md`."),
        vec!["docs/ARCHITECTURE.md"]
    );
}

#[test]
fn a_bare_name_is_a_sibling_of_the_path_before_it() {
    assert_eq!(
        paths("crates/pcx-gateway/src/access.rs, grpc.rs, mcp.rs; docs/A.md"),
        vec![
            "crates/pcx-gateway/src/access.rs",
            "crates/pcx-gateway/src/grpc.rs",
            "crates/pcx-gateway/src/mcp.rs",
            "docs/A.md"
        ]
    );
    assert_eq!(
        paths("crates/x/src/{a.rs, b.rs}, c.rs"),
        vec![
            "crates/x/src/a.rs",
            "crates/x/src/b.rs",
            "crates/x/src/c.rs"
        ]
    );
    assert_eq!(paths("README.md, Makefile"), vec!["README.md", "Makefile"]);
}

#[test]
fn an_expression_is_not_a_file() {
    assert!(paths("Rerank.fallback").is_empty());
    assert_eq!(paths("app/build.gradle"), vec!["app/build.gradle"]);
}

#[test]
fn a_leading_base_is_the_directory_of_what_follows() {
    assert_eq!(
        paths(
            "post-cortex-cloud/app: lib/features/modules/modules_screen.dart (new), lib/data/{models,repositories}.dart, README.md"
        ),
        vec![
            "post-cortex-cloud/app/lib/features/modules/modules_screen.dart",
            "post-cortex-cloud/app/lib/data/models.dart",
            "post-cortex-cloud/app/lib/data/repositories.dart",
            "post-cortex-cloud/app/lib/data/README.md"
        ]
    );
    assert_eq!(paths("Committed: the gateway"), Vec::<String>::new());
    assert_eq!(paths("src/main.rs:42"), vec!["src/main.rs"]);
}
