// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Who and what a call or event is for (T7): explicit on every call.

use std::time::{Duration, Instant};

use post_cortex_proto::module as wire;
use post_cortex_proto::module::PersonRole;
use tokio_util::sync::CancellationToken;
use tonic::metadata::{AsciiMetadataValue, MetadataMap};

use crate::{ModuleError, ModuleErrorKind};

mod workspaces;

pub use workspaces::narrower;

/// How many module calls deep a call chain may go.
pub const MAX_CALL_DEPTH: u32 = 4;

/// The metadata of a scope on a gRPC call (W23). The host sets the caller,
/// the depth and the turn of a module's call, never the module.
pub mod metadata {
    use post_cortex_proto::module::PersonRole;

    /// The tenant: the organisation's slug, named by the host that serves
    /// it whatever the call carried (docs/host.md).
    pub const TENANT: &str = "x-pcx-tenant";
    /// The profile.
    pub const PROFILE: &str = "x-pcx-profile";
    /// The session.
    pub const SESSION: &str = "x-pcx-session";
    /// The workspace.
    pub const WORKSPACE: &str = "x-pcx-workspace";
    /// Who acts.
    pub const AUTHOR: &str = post_cortex_proto::AUTHOR_HEADER;
    /// The module that makes the call (the host's).
    pub const CALLER: &str = "x-pcx-caller";
    /// How many module calls deep the call is (the host's).
    pub const DEPTH: &str = "x-pcx-depth";
    /// The token of a module's turn (the host's).
    pub const TURN: &str = "x-pcx-turn";
    /// Milliseconds left for the call.
    pub const DEADLINE_MS: &str = "x-pcx-deadline-ms";
    /// The workspaces the call may reach, comma-separated ([`NO_WORKSPACE`]:
    /// none); absent: every one.
    pub const REACH: &str = "x-pcx-reach";
    /// The value of [`REACH`] that reaches no workspace.
    pub const NO_WORKSPACE: &str = "none";
    /// The organisation's role of the person who is the author: `owner`,
    /// `admin`, `member` or `viewer` (set by what authenticated the person).
    pub const ROLE: &str = "x-pcx-role";
    /// The caller may only read (docs/host.md, effects): present, it is
    /// [`READ_ONLY_VALUE`].
    pub const READ_ONLY: &str = "x-pcx-read-only";
    /// The one value of [`READ_ONLY`].
    pub const READ_ONLY_VALUE: &str = "1";
    /// The person the call is for: a cloud user's id, set by the gateway
    /// from the person's session or personal key (docs/host.md); absent:
    /// none, as on a local host.
    pub const PERSON: &str = "x-pcx-person";
    /// A call that acts outside once whatever retries it (a workflow's
    /// `external` block, docs/modules/workflow.md): its key, which a module
    /// passes on where the outside API takes one. Not a part of the scope:
    /// it is the one call's, and the host passes it on unchanged.
    pub const IDEMPOTENCY_KEY: &str = "x-pcx-idempotency-key";
    /// The module a call goes to, by id, where several serve its service
    /// (docs/host.md, a service several modules serve). Not a part of the
    /// scope: the caller's client sets it on the one call.
    pub const TARGET: &str = "x-pcx-target";

    /// The value of [`ROLE`] that names `role` (empty for none).
    #[must_use]
    pub fn role_value(role: PersonRole) -> &'static str {
        match role {
            PersonRole::Unspecified => "",
            PersonRole::Owner => "owner",
            PersonRole::Admin => "admin",
            PersonRole::Member => "member",
            PersonRole::Viewer => "viewer",
        }
    }

    /// The workspaces a [`REACH`] value names.
    #[must_use]
    pub fn reach_of(value: &str) -> Vec<String> {
        if value == NO_WORKSPACE {
            return Vec::new();
        }
        value
            .split(',')
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    }
    /// The [`REACH`] value that names `workspaces`.
    #[must_use]
    pub fn reach_value(workspaces: &[String]) -> String {
        if workspaces.is_empty() {
            NO_WORKSPACE.to_string()
        } else {
            workspaces.join(",")
        }
    }

    /// The keys only the host sets: a call from outside it that carries
    /// them has them removed.
    pub const HOST: [&str; 3] = [CALLER, DEPTH, TURN];
    /// Every key of a scope.
    pub const ALL: [&str; 13] = [
        TENANT,
        PROFILE,
        SESSION,
        WORKSPACE,
        AUTHOR,
        CALLER,
        DEPTH,
        TURN,
        DEADLINE_MS,
        REACH,
        ROLE,
        READ_ONLY,
        PERSON,
    ];
}

/// The role named `name` in [`metadata::ROLE`].
fn role_named(name: &str) -> Option<PersonRole> {
    [
        PersonRole::Owner,
        PersonRole::Admin,
        PersonRole::Member,
        PersonRole::Viewer,
    ]
    .into_iter()
    .find(|role| metadata::role_value(*role) == name)
}

/// The scope of one module call or event.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The tenant: the slug of the organisation whose host routes the call
    /// (a runner's calls carry its organisation's); empty for a local host.
    pub tenant: String,
    /// The profile within the tenant.
    pub profile: String,
    /// The session, when there is one.
    pub session_id: Option<String>,
    /// The workspace, when there is one.
    pub workspace_id: Option<String>,
    /// Who acts (agent, person), when known.
    pub author: Option<String>,
    /// The module that makes the call, set by the host only; none: not a
    /// module.
    pub caller_module: Option<String>,
    /// How many module calls deep the call is (0: not a module's call).
    pub call_depth: u32,
    /// The host's token of a turn a module asked of the agent, set by the
    /// host only: the agent's calls in it are under that module's reach
    /// and grant; none: not such a turn.
    pub turn: Option<String>,
    /// The workspaces the call may reach (`docs/connect.md`, reach): set for
    /// a person limited to some workspaces, carried by every call made in
    /// this scope; none: every one.
    pub reach: Option<Vec<String>>,
    /// The organisation's role of the person who is the author, carried by
    /// a client's call only (docs/host.md); none: not known.
    pub role: Option<PersonRole>,
    /// The caller may only read: the host refuses every RPC and tool whose
    /// effect is not `read`, and every turn; carried by every call made in
    /// this scope (docs/host.md, effects).
    pub read_only: bool,
    /// The person the call is for (a cloud user's id), set by a gateway;
    /// carried by every call made in this scope, so what a module or an
    /// agent does for the person reads the person's user card
    /// (docs/memory.md, cards); none: no person, as on a local host.
    pub person: Option<String>,
    /// When the call must be done.
    pub deadline: Instant,
    /// Cancelled when the caller gives up.
    pub cancel: CancellationToken,
}

/// The domain of the address an agent's commits carry (owner 2026-11-07):
/// one that receives no mail, so no agent passes for a person.
pub const AGENTS_DOMAIN: &str = "agents.pcx.is";

/// The agent that acts in a scope, as git names a commit's author.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAuthor {
    /// Its name, the scope's author.
    pub name: String,
    /// `<name>@<tenant>.agents.pcx.is`; `<name>@agents.pcx.is` on a local
    /// host. The name's characters other than letters, digits, `.`, `_` and
    /// `-` are `-` in it.
    pub email: String,
}

impl Scope {
    /// The agent that acts in this scope, as a commit names its author
    /// (docs/host.md, a commit's author); none when no one is named or a
    /// person acts (a call that carries a person's role).
    #[must_use]
    pub fn agent_author(&self) -> Option<AgentAuthor> {
        if self.role.is_some() {
            return None;
        }
        let name = self.author.as_deref().filter(|a| !a.is_empty())?;
        let local: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let domain = if self.tenant.is_empty() {
            AGENTS_DOMAIN.to_string()
        } else {
            format!("{}.{AGENTS_DOMAIN}", self.tenant)
        };
        Some(AgentAuthor {
            name: name.to_string(),
            email: format!("{local}@{domain}"),
        })
    }

    /// The scope of a module's own calls (its pruning, its jobs), not
    /// made for a caller: in `workspace`, by `module` as their author,
    /// within `deadline` from now, cancelled with `cancel`.
    #[must_use]
    pub fn own(
        module: &str,
        workspace: &str,
        deadline: Duration,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            tenant: String::new(),
            profile: String::new(),
            session_id: None,
            workspace_id: Some(workspace.to_string()),
            author: Some(module.to_string()),
            caller_module: None,
            call_depth: 0,
            turn: None,
            reach: None,
            role: None,
            read_only: false,
            person: None,
            deadline: Instant::now() + deadline,
            cancel,
        }
    }

    /// What is left of the deadline (zero once it passed).
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// The wire form, with the time left in milliseconds.
    #[must_use]
    pub fn to_wire(&self) -> wire::Scope {
        wire::Scope {
            tenant: self.tenant.clone(),
            profile: self.profile.clone(),
            session_id: self.session_id.clone().unwrap_or_default(),
            workspace_id: self.workspace_id.clone().unwrap_or_default(),
            author: self.author.clone().unwrap_or_default(),
            caller_module: self.caller_module.clone().unwrap_or_default(),
            call_depth: self.call_depth,
            turn: self.turn.clone().unwrap_or_default(),
            reach: self
                .reach
                .clone()
                .map(|workspaces| wire::Reach { workspaces }),
            role: self.role.unwrap_or(PersonRole::Unspecified).into(),
            read_only: self.read_only,
            person: self.person.clone().unwrap_or_default(),
            // A deadline past ~49 days saturates: the wire carries u32.
            deadline_ms: u32::try_from(self.remaining().as_millis()).unwrap_or(u32::MAX),
        }
    }

    /// Write the scope into a gRPC call's `metadata` (W23), replacing what
    /// it carried; an empty value is left out.
    ///
    /// # Errors
    /// `InvalidArgument` for a value that is not ASCII text.
    pub fn to_metadata(&self, metadata: &mut MetadataMap) -> Result<(), ModuleError> {
        for key in metadata::ALL {
            metadata.remove(key);
        }
        let depth = self.call_depth.to_string();
        let reach = self
            .reach
            .as_ref()
            .map(|workspaces| metadata::reach_value(workspaces));
        // A deadline past ~49 days saturates, as on the wire.
        let left = u32::try_from(self.remaining().as_millis())
            .unwrap_or(u32::MAX)
            .to_string();
        let values = [
            (metadata::TENANT, Some(self.tenant.as_str())),
            (metadata::PROFILE, Some(self.profile.as_str())),
            (metadata::SESSION, self.session_id.as_deref()),
            (metadata::WORKSPACE, self.workspace_id.as_deref()),
            (metadata::AUTHOR, self.author.as_deref()),
            (metadata::CALLER, self.caller_module.as_deref()),
            (metadata::DEPTH, Some(depth.as_str())),
            (metadata::TURN, self.turn.as_deref()),
            (metadata::DEADLINE_MS, Some(left.as_str())),
            (metadata::REACH, reach.as_deref()),
            (metadata::ROLE, self.role.map(metadata::role_value)),
            (
                metadata::READ_ONLY,
                self.read_only.then_some(metadata::READ_ONLY_VALUE),
            ),
            (metadata::PERSON, self.person.as_deref()),
        ];
        for (key, value) in values {
            let Some(value) = value.filter(|v| !v.is_empty()) else {
                continue;
            };
            let value = AsciiMetadataValue::try_from(value).map_err(|_| {
                ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("the scope's {key} is not ASCII text"),
                )
            })?;
            metadata.insert(key, value);
        }
        Ok(())
    }

    /// The scope a gRPC call's `metadata` carries (W23): the deadline counts
    /// from now (`default` when it names none), the cancellation is `cancel`.
    ///
    /// # Errors
    /// `InvalidArgument` for a value that is not text, or a depth or a
    /// deadline that is not a count.
    pub fn from_metadata(
        metadata: &MetadataMap,
        cancel: CancellationToken,
        default: Duration,
    ) -> Result<Self, ModuleError> {
        let text = |key: &str| -> Result<Option<String>, ModuleError> {
            match metadata.get(key) {
                None => Ok(None),
                Some(value) => value
                    .to_str()
                    .map(|v| (!v.is_empty()).then(|| v.to_string()))
                    .map_err(|_| {
                        ModuleError::new(
                            ModuleErrorKind::InvalidArgument,
                            format!("the call's {key} is not text"),
                        )
                    }),
            }
        };
        let count = |key: &str| -> Result<Option<u32>, ModuleError> {
            text(key)?
                .map(|v| {
                    v.parse::<u32>().map_err(|_| {
                        ModuleError::new(
                            ModuleErrorKind::InvalidArgument,
                            format!("the call's {key} is not a count"),
                        )
                    })
                })
                .transpose()
        };
        Ok(Self {
            tenant: text(metadata::TENANT)?.unwrap_or_default(),
            profile: text(metadata::PROFILE)?.unwrap_or_default(),
            session_id: text(metadata::SESSION)?,
            workspace_id: text(metadata::WORKSPACE)?,
            author: text(metadata::AUTHOR)?,
            caller_module: text(metadata::CALLER)?,
            call_depth: count(metadata::DEPTH)?.unwrap_or(0),
            turn: text(metadata::TURN)?,
            // Present, even empty, it limits: an empty one reaches no
            // workspace.
            reach: match metadata.get(metadata::REACH) {
                None => None,
                Some(_) => Some(metadata::reach_of(
                    &text(metadata::REACH)?.unwrap_or_default(),
                )),
            },
            role: text(metadata::ROLE)?
                .map(|name| {
                    role_named(&name).ok_or_else(|| {
                        ModuleError::new(
                            ModuleErrorKind::InvalidArgument,
                            format!(
                                "the call's {} is owner, admin, member or viewer, not {name}",
                                metadata::ROLE
                            ),
                        )
                    })
                })
                .transpose()?,
            read_only: match text(metadata::READ_ONLY)? {
                None => false,
                Some(value) if value == metadata::READ_ONLY_VALUE => true,
                Some(value) => {
                    return Err(ModuleError::new(
                        ModuleErrorKind::InvalidArgument,
                        format!(
                            "the call's {} is {}, not {value}",
                            metadata::READ_ONLY,
                            metadata::READ_ONLY_VALUE
                        ),
                    ));
                }
            },
            person: text(metadata::PERSON)?,
            deadline: Instant::now()
                + match count(metadata::DEADLINE_MS)? {
                    None | Some(0) => default,
                    Some(ms) => Duration::from_millis(u64::from(ms)),
                },
            cancel,
        })
    }

    /// The scope of `request`, a gRPC call a module serves (W23), as
    /// [`Scope::from_metadata`] with the default deadline.
    ///
    /// # Errors
    /// `InvalidArgument`, as [`Scope::from_metadata`].
    pub fn of<T>(request: &tonic::Request<T>) -> Result<Self, tonic::Status> {
        Self::from_metadata(
            request.metadata(),
            CancellationToken::new(),
            crate::DEFAULT_DEADLINE,
        )
        .map_err(tonic::Status::from)
    }

    /// The scope of a call that arrived over the wire: the deadline counts
    /// from now (`default` when the wire sets none), the cancellation is
    /// `cancel` (the RPC's or request's).
    #[must_use]
    pub fn from_wire(scope: wire::Scope, cancel: CancellationToken, default: Duration) -> Self {
        let text = |s: String| (!s.is_empty()).then_some(s);
        // A role this version does not know is none: the least.
        let role = Some(scope.role()).filter(|role| *role != PersonRole::Unspecified);
        Self {
            tenant: scope.tenant,
            profile: scope.profile,
            session_id: text(scope.session_id),
            workspace_id: text(scope.workspace_id),
            author: text(scope.author),
            caller_module: text(scope.caller_module),
            call_depth: scope.call_depth,
            turn: text(scope.turn),
            reach: scope.reach.map(|r| r.workspaces),
            role,
            read_only: scope.read_only,
            person: text(scope.person),
            deadline: Instant::now()
                + match scope.deadline_ms {
                    0 => default,
                    ms => Duration::from_millis(u64::from(ms)),
                },
            cancel,
        }
    }
}
