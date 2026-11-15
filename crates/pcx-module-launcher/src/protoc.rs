// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the proto compiler is told when a bundle is prepared. Pure.

use std::path::Path;

use pcx_module_api::DESCRIPTORS_FILE;

/// The arguments that compile `protos` beneath `proto` into `generated`: by
/// `plugin` for TypeScript (protoc-gen-es), else for Python. Generated code keeps
/// no comments, and an RPC's comment is what its tool says: the descriptors are
/// written with theirs, and the core reads them (`DESCRIPTORS_ENV`).
#[must_use]
pub fn args(
    proto: &Path,
    generated: &Path,
    plugin: Option<&Path>,
    protos: Vec<String>,
) -> Vec<String> {
    let out = generated.display();
    let mut args = vec![
        format!("-I{}", proto.display()),
        format!(
            "--descriptor_set_out={}",
            generated.join(DESCRIPTORS_FILE).display()
        ),
        "--include_source_info".to_string(),
        "--include_imports".to_string(),
    ];
    match plugin {
        Some(plugin) => {
            args.push(format!("--plugin=protoc-gen-es={}", plugin.display()));
            args.push(format!("--es_out={out}"));
            args.push("--es_opt=target=ts,import_extension=ts".to_string());
        }
        None => args.push(format!("--python_out={out}")),
    }
    args.extend(protos);
    args
}
