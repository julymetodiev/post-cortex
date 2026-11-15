//! Build script: the `pcx.v1` services (`PostCortex` and `Blobs`, with
//! their descriptor set), the
//! `pcx.views.v1` service, the `pcx.blocks.v1` blocks of a message, the
//! `pcx.view.v1` module views and the `pcx.module.v1` module protocol (the
//! last three with their proto3 JSON mapping) via `tonic-prost-build`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    // The pcx.v1 descriptor: a gateway maps the operation table's field
    // names to field numbers through it (`PCX_DESCRIPTOR_SET`).
    // It reuses the records' validity, relations and alias candidates of
    // pcx.views.v1 (at `crate::views`).
    tonic_prost_build::configure()
        .file_descriptor_set_path(out.join("pcx_descriptor.bin"))
        .extern_path(".pcx.views.v1", "crate::views")
        .extern_path(".pcx.blocks.v1", "crate::blocks")
        .compile_with_config(
            named(),
            &["proto/pcx.proto", "proto/pcx_blob.proto"],
            &["proto"],
        )?;
    // A search item's record and message are large: both boxed, so the
    // `oneof` stays the size of a pointer.
    // Its descriptor too: a gateway classifies view calls through it
    // (`VIEWS_DESCRIPTOR_SET`).
    tonic_prost_build::configure()
        .file_descriptor_set_path(out.join("views_descriptor.bin"))
        .boxed(".pcx.views.v1.SearchItem.source.record")
        .boxed(".pcx.views.v1.SearchItem.source.message")
        // A message's blocks are pcx.blocks.v1's (W25).
        .extern_path(".pcx.blocks.v1", "crate::blocks")
        .compile_protos(&["proto/views.proto"], &["proto"])?;

    // pcx.blocks.v1 apart, so memory's API and the module protocol share
    // its types at `crate::blocks` (a client of memory never carries the
    // module protocol), with the proto3 JSON mapping the module protocol's
    // JSON needs of them.
    let block_descriptors = out.join("pcx_blocks_descriptor.bin");
    tonic_prost_build::configure()
        .file_descriptor_set_path(&block_descriptors)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .compile_protos(&["proto/pcx_blocks.proto"], &["proto"])?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&block_descriptors)?)?
        .preserve_proto_field_names()
        .ignore_unknown_fields()
        .build(&[".pcx.blocks.v1"])?;

    // pcx.tunnel.v1: a daemon connected to an organisation of the cloud,
    // its pairing and its tunnel to the gateway (docs/connect.md).
    // Bodies are `Bytes`, so a forwarded body is never copied.
    // A runner's `Hello` carries module manifests: the module protocol's.
    tonic_prost_build::configure()
        .bytes(".pcx.tunnel.v1")
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_protos(&["proto/pcx_tunnel.proto"], &["proto"])?;

    // pcx.view.v1 apart, so its types sit at `crate::ui` (`view` would hold
    // the generated `view` module of View) and the module protocol refers
    // to them there. A view's root is boxed, so its `oneof` stays the size
    // of a pointer.
    let view_descriptors = out.join("pcx_view_descriptor.bin");
    tonic_prost_build::configure()
        .file_descriptor_set_path(&view_descriptors)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .boxed(".pcx.view.v1.View.body.root")
        .compile_protos(&["proto/pcx_view.proto"], &["proto"])?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&view_descriptors)?)?
        .preserve_proto_field_names()
        .ignore_unknown_fields()
        .build(&[".pcx.view.v1"])?;

    // pcx.agent.v1: the agent's turns, for a client of the host, a host
    // and an agent process, and a module and its host; with its
    // descriptors, which hold the agent's settings (its manifest carries
    // them).
    tonic_prost_build::configure()
        .file_descriptor_set_path(out.join("agent_descriptor.bin"))
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_protos(&["proto/pcx_agent.proto"], &["proto"])?;

    // The first-party modules' packages (W23): each named, so its
    // documents travel in an Any, and with its descriptors (source info
    // included: the comments are what agents and people read), which its
    // manifest carries.
    // Each one's code goes to a directory of its own: the generator writes
    // the packages it imports too, which must not overwrite theirs. A
    // package it imports is the crate's own module (the board's team and
    // memory's attachments).
    let modules = out.join("modules");
    for (name, imports) in [
        ("chief", &[][..]),
        ("skills", &[][..]),
        ("team", &[][..]),
        (
            "board",
            &[(".pcx.team.v1", "crate::team"), (".pcx.v1", "crate::pb")][..],
        ),
        ("tools_fs", &[][..]),
        ("tools_git", &[][..]),
        ("tools_shell", &[][..]),
        // docs/modules/agent.md, the providers: every provider in one module.
        ("models", &[][..]),
        // docs/modules/workflow.md: a job may run a workflow.
        (
            "scheduler",
            &[
                (".pcx.workflow.v1", "crate::workflow"),
                (".pcx.board.v1", "crate::board"),
                (".pcx.team.v1", "crate::team"),
                (".pcx.v1", "crate::pb"),
            ][..],
        ),
        ("channel", &[][..]),
        // docs/modules/workflow.md: an approval's answer is a board
        // decision.
        (
            "workflow",
            &[
                (".pcx.board.v1", "crate::board"),
                (".pcx.team.v1", "crate::team"),
                (".pcx.v1", "crate::pb"),
            ][..],
        ),
        // docs/modules/http.md: `expect` is a workflow's schema; a body is
        // pbjson's Value, which has the JSON mapping.
        (
            "http",
            &[
                (".google.protobuf.Value", "::pbjson_types::Value"),
                (".pcx.workflow.v1", "crate::workflow"),
                (".pcx.board.v1", "crate::board"),
                (".pcx.team.v1", "crate::team"),
                (".pcx.v1", "crate::pb"),
            ][..],
        ),
    ] {
        let dir = modules.join(name);
        std::fs::create_dir_all(&dir)?;
        // Their times are prost's, as memory's documents carry them; a
        // Struct is pbjson's, which has the JSON mapping.
        let mut builder = tonic_prost_build::configure()
            .out_dir(&dir)
            .file_descriptor_set_path(out.join(format!("{name}_descriptor.bin")))
            .extern_path(".google.protobuf.Struct", "::pbjson_types::Struct")
            .extern_path(".pcx.module.v1", "crate::module")
            .extern_path(".pcx.view.v1", "crate::ui")
            .extern_path(".pcx.agent.v1", "crate::agent");
        for (package, path) in imports {
            builder = builder.extern_path(*package, *path);
        }
        builder.compile_with_config(
            named(),
            &[format!("proto/pcx_{name}.proto")],
            &["proto".to_string()],
        )?;
    }

    // The webhooks' package (W27): its Alertmanager messages mirror an
    // outside JSON API, so the package has the proto3 JSON mapping (pbjson,
    // its well-known types pbjson's) for its adapter to decode bodies with,
    // lowerCamelCase as Alertmanager names its fields.
    let webhook_descriptors = out.join("webhooks_descriptor.bin");
    let webhooks = modules.join("webhooks");
    std::fs::create_dir_all(&webhooks)?;
    tonic_prost_build::configure()
        .out_dir(&webhooks)
        .file_descriptor_set_path(&webhook_descriptors)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_with_config(named(), &["proto/pcx_webhooks.proto"], &["proto"])?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&webhook_descriptors)?)?
        .out_dir(&webhooks)
        .ignore_unknown_fields()
        .build(&[".pcx.webhooks.v1"])?;

    // Telegram's package (W29): its Bot API messages mirror an outside JSON
    // API, so the package has the proto3 JSON mapping, the Bot API's
    // snake_case names kept.
    let telegram_descriptors = out.join("telegram_descriptor.bin");
    let telegram = modules.join("telegram");
    std::fs::create_dir_all(&telegram)?;
    tonic_prost_build::configure()
        .out_dir(&telegram)
        .file_descriptor_set_path(&telegram_descriptors)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_with_config(
            named(),
            // The channel service it serves, so its descriptors hold it.
            &["proto/pcx_telegram.proto", "proto/pcx_channel.proto"],
            &["proto"],
        )?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&telegram_descriptors)?)?
        .out_dir(&telegram)
        .preserve_proto_field_names()
        .ignore_unknown_fields()
        .build(&[".pcx.telegram.v1"])?;

    // Discord's package (docs/modules/discord.md): its service, settings and
    // events, and Discord's REST and Gateway JSON mirrored, so the package
    // has the proto3 JSON mapping, Discord's snake_case names kept.
    let discord_descriptors = out.join("discord_descriptor.bin");
    let discord = modules.join("discord");
    std::fs::create_dir_all(&discord)?;
    tonic_prost_build::configure()
        .out_dir(&discord)
        .file_descriptor_set_path(&discord_descriptors)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_with_config(
            named(),
            // The channel service it serves, so its descriptors hold it.
            &["proto/pcx_discord.proto", "proto/pcx_channel.proto"],
            &["proto"],
        )?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&discord_descriptors)?)?
        .out_dir(&discord)
        .preserve_proto_field_names()
        .ignore_unknown_fields()
        .build(&[".pcx.discord.v1"])?;

    // Observe's package (W30): its mirrors of Prometheus, Loki, Alertmanager
    // and Kubernetes JSON, with the proto3 JSON mapping its adapters decode
    // their answers with (their camelCase names).
    let observe_descriptors = out.join("observe_descriptor.bin");
    let observe = modules.join("observe");
    std::fs::create_dir_all(&observe)?;
    tonic_prost_build::configure()
        .out_dir(&observe)
        .file_descriptor_set_path(&observe_descriptors)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_with_config(named(), &["proto/pcx_observe.proto"], &["proto"])?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&observe_descriptors)?)?
        .out_dir(&observe)
        .ignore_unknown_fields()
        .build(&[".pcx.observe.v1"])?;

    // Ops' package (W31): its documents (named, so they travel in an Any,
    // their times prost's as memory's documents carry them) and its mirrors
    // of Kubernetes patches and objects and of Nomad's jobs, allocations and
    // revert, with the proto3 JSON mapping its adapter encodes what it sends
    // and decodes what it reads with.
    let ops_descriptors = out.join("ops_descriptor.bin");
    let ops = modules.join("ops");
    std::fs::create_dir_all(&ops)?;
    tonic_prost_build::configure()
        .out_dir(&ops)
        .file_descriptor_set_path(&ops_descriptors)
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_with_config(named(), &["proto/pcx_ops.proto"], &["proto"])?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&ops_descriptors)?)?
        .out_dir(&ops)
        .ignore_unknown_fields()
        .build(&[
            ".pcx.ops.v1.DeploymentPatch",
            ".pcx.ops.v1.PatchSpec",
            ".pcx.ops.v1.PatchTemplate",
            ".pcx.ops.v1.PatchMeta",
            ".pcx.ops.v1.ClaimPatch",
            ".pcx.ops.v1.ClaimSpec",
            ".pcx.ops.v1.ClaimResources",
            ".pcx.ops.v1.KubeThing",
            ".pcx.ops.v1.ThingMeta",
            ".pcx.ops.v1.OwnerRef",
            ".pcx.ops.v1.ThingSpec",
            ".pcx.ops.v1.ThingStatus",
            ".pcx.ops.v1.ThingList",
            ".pcx.ops.v1.NomadJob",
            ".pcx.ops.v1.NomadPeriodic",
            ".pcx.ops.v1.NomadVersions",
            ".pcx.ops.v1.NomadSummary",
            ".pcx.ops.v1.NomadChildren",
            ".pcx.ops.v1.NomadDeployment",
            ".pcx.ops.v1.NomadAlloc",
            ".pcx.ops.v1.NomadRevert",
        ])?;

    // The dynamic module's package (docs/modules/dynamic.md).
    let dynamic_descriptors = out.join("dynamic_descriptor.bin");
    let dynamic = modules.join("dynamic");
    std::fs::create_dir_all(&dynamic)?;
    tonic_prost_build::configure()
        .out_dir(&dynamic)
        .file_descriptor_set_path(&dynamic_descriptors)
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .extern_path(".pcx.v1", "crate::pb")
        .compile_with_config(named(), &["proto/pcx_dynamic.proto"], &["proto"])?;

    // Mail's package (docs/modules/mail.md): its settings and its log
    // (named, so they travel in an Any, their times prost's as memory's
    // documents carry them) and its mirrors of Resend's and Postmark's JSON,
    // with the proto3 JSON mapping its adapters encode sends and decode
    // answers with.
    let mail_descriptors = out.join("mail_descriptor.bin");
    let mail = modules.join("mail");
    std::fs::create_dir_all(&mail)?;
    tonic_prost_build::configure()
        .out_dir(&mail)
        .file_descriptor_set_path(&mail_descriptors)
        // A template's values are JSON: pbjson's Struct and Value, which
        // have the JSON mapping.
        .extern_path(".google.protobuf.Struct", "::pbjson_types::Struct")
        .extern_path(".google.protobuf.Value", "::pbjson_types::Value")
        .extern_path(".pcx.module.v1", "crate::module")
        .extern_path(".pcx.view.v1", "crate::ui")
        .compile_with_config(named(), &["proto/pcx_mail.proto"], &["proto"])?;
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&mail_descriptors)?)?
        .out_dir(&mail)
        .ignore_unknown_fields()
        .build(&[
            ".pcx.mail.v1.ResendEmail",
            ".pcx.mail.v1.ResendAnswer",
            ".pcx.mail.v1.PostmarkEmail",
            ".pcx.mail.v1.PostmarkAnswer",
        ])?;

    let descriptors = out.join("pcx_module_descriptor.bin");
    tonic_prost_build::configure()
        .file_descriptor_set_path(&descriptors)
        // The well-known types are pbjson's, which have the JSON mapping.
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .extern_path(".pcx.view.v1", "crate::ui")
        .extern_path(".pcx.blocks.v1", "crate::blocks")
        .compile_protos(
            &["proto/pcx_module.proto", "proto/pcx_model.proto"],
            &["proto"],
        )?;
    // The hook JSON of Claude Code and Codex uses the proto field names
    // (`hook_event_name`, `tool_input`), not proto3's lowerCamelCase, and
    // carries fields a module does not read (`permission_mode`, ...), which
    // proto3 JSON parsing skips.
    pbjson_build::Builder::new()
        .register_descriptors(&std::fs::read(&descriptors)?)?
        .preserve_proto_field_names()
        .ignore_unknown_fields()
        .build(&[".pcx.module.v1"])?;
    // pbjson 0.9 formats a borrowed `&FIELDS`, which the workspace lints
    // refuse (`useless_borrows_in_formatting`); the generated serde code is
    // written without the borrow (upstream: influxdata/pbjson).
    for serde in [
        out.join("pcx.module.v1.serde.rs"),
        out.join("pcx.blocks.v1.serde.rs"),
        out.join("pcx.view.v1.serde.rs"),
        webhooks.join("pcx.webhooks.v1.serde.rs"),
        telegram.join("pcx.telegram.v1.serde.rs"),
        discord.join("pcx.discord.v1.serde.rs"),
        observe.join("pcx.observe.v1.serde.rs"),
        ops.join("pcx.ops.v1.serde.rs"),
        mail.join("pcx.mail.v1.serde.rs"),
    ] {
        let code = std::fs::read_to_string(&serde)?;
        std::fs::write(&serde, code.replace("{:?}\", &FIELDS)", "{:?}\", FIELDS)"))?;
    }
    Ok(())
}

/// The code generation of a package whose messages name their type
/// (`prost::Name`), so they travel in a `google.protobuf.Any` (W23).
fn named() -> tonic_prost_build::Config {
    let mut config = tonic_prost_build::Config::new();
    config.enable_type_names();
    config
}
