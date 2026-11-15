// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A message to a channel as its sender names where it goes
//! (docs/modules/telegram.md, the channel service): `<module>/<name>` is
//! destination `<name>` of channel module `<module>` (`telegram/ops`,
//! `discord/alerts`), a plain name a destination of the one channel that
//! runs. Several modules serve `pcx.channel.v1.Channel`, so the call names
//! the module as its target (docs/host.md, a service several modules serve).

use post_cortex_proto::channel::SendRequest;
use tonic::metadata::AsciiMetadataValue;

use crate::scope::metadata;
use crate::{ModuleError, ModuleErrorKind, Scope};

/// Where a message goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination<'a> {
    /// The channel module, when it is named.
    pub module: Option<&'a str>,
    /// The destination, as that module's settings name it.
    pub name: &'a str,
}

/// The destination `named` names.
///
/// # Errors
/// `InvalidArgument` when it names no destination, or `<module>/<name>`
/// with no module.
pub fn destination(named: &str) -> Result<Destination<'_>, ModuleError> {
    let (module, name) = match named.split_once('/') {
        Some((module, name)) => (Some(module), name),
        None => (None, named),
    };
    if name.is_empty() || module.is_some_and(str::is_empty) {
        return Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("\"{named}\" is not a destination: <name> or <module>/<name>"),
        ));
    }
    Ok(Destination { module, name })
}

/// The call that sends `text` to `named` in `scope`.
///
/// # Errors
/// `InvalidArgument` as [`destination`], or for a scope or a module's id
/// that is not ASCII text.
pub fn send(
    scope: &Scope,
    named: &str,
    text: &str,
) -> Result<tonic::Request<SendRequest>, ModuleError> {
    let to = destination(named)?;
    let mut request = tonic::Request::new(SendRequest {
        destination: to.name.to_string(),
        text: text.to_string(),
    });
    scope.to_metadata(request.metadata_mut())?;
    if let Some(module) = to.module {
        let target = AsciiMetadataValue::try_from(module).map_err(|_| {
            ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!("\"{named}\": the module's id is not ASCII text"),
            )
        })?;
        request.metadata_mut().insert(metadata::TARGET, target);
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::*;

    #[test]
    fn a_destination_names_its_module_or_only_itself() {
        assert_eq!(
            destination("discord/alerts").unwrap(),
            Destination {
                module: Some("discord"),
                name: "alerts"
            }
        );
        assert_eq!(
            destination("ops").unwrap(),
            Destination {
                module: None,
                name: "ops"
            }
        );
        for bad in ["", "/ops", "telegram/"] {
            let refused = destination(bad).unwrap_err();
            assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument, "{bad}");
        }
    }

    #[test]
    fn a_send_names_the_module_as_the_calls_target() {
        let scope = Scope::own("ops", "w", Duration::from_secs(5), CancellationToken::new());
        let named = send(&scope, "discord/alerts", "down").unwrap();
        assert_eq!(named.get_ref().destination, "alerts");
        assert_eq!(named.metadata().get(metadata::TARGET).unwrap(), "discord");
        assert_eq!(named.metadata().get(metadata::AUTHOR).unwrap(), "ops");

        let plain = send(&scope, "ops", "down").unwrap();
        assert_eq!(plain.get_ref().destination, "ops");
        assert!(plain.metadata().get(metadata::TARGET).is_none());
    }
}
