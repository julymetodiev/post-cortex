// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's tools: what each takes and what it answers.

use post_cortex_proto::module as wire;
use serde_json::{Map, Value};

use crate::{ModuleError, ModuleErrorKind, json};

pub use post_cortex_proto::module::Effect;

/// A tool a module offers (the `pcx.module.v1` `ToolSpec`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// Name, unique within the module.
    pub name: String,
    /// What it does, for a model.
    pub description: String,
    /// JSON Schema of its input.
    pub input_schema: Map<String, Value>,
    /// Who may call it.
    pub visibility: Visibility,
    /// What calling it does.
    pub effect: Effect,
}

/// The effect the wire's `effect` of RPC or tool `name` names.
///
/// # Errors
/// `InvalidArgument` for an effect that is unset or unknown.
pub fn effect_from_wire(effect: i32, name: &str) -> Result<Effect, ModuleError> {
    match Effect::try_from(effect) {
        Ok(Effect::Unspecified) | Err(_) => Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{name}: its effect is not set ({effect})"),
        )),
        Ok(effect) => Ok(effect),
    }
}

/// Who may call a tool (as MCP Apps): the host's tool lists for agents
/// leave out a tool an agent may not call; a module's views call those an
/// app may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// An agent and the module's views.
    Both,
    /// An agent only.
    Model,
    /// The module's views only.
    App,
}

/// Whether an agent may call a tool of the wire's `visibility` (W23, over
/// the generated `ToolVisibility` values): none set is both.
///
/// # Errors
/// `InvalidArgument` for a value that is unset or unknown.
pub fn agent_may_call(visibility: &[i32], tool: &str) -> Result<bool, ModuleError> {
    let mut model = visibility.is_empty();
    for named in visibility {
        match wire::ToolVisibility::try_from(*named) {
            Ok(wire::ToolVisibility::Model) => model = true,
            Ok(wire::ToolVisibility::App) => {}
            Ok(wire::ToolVisibility::Unspecified) | Err(_) => {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("tool {tool}: visibility {named} is unknown"),
                ));
            }
        }
    }
    Ok(model)
}

impl Visibility {
    /// Whether an agent may call it.
    #[must_use]
    pub fn model(self) -> bool {
        matches!(self, Self::Both | Self::Model)
    }

    /// The visibility the wire's `visibility` of tool `tool` names; none:
    /// both (as MCP Apps).
    ///
    /// # Errors
    /// `InvalidArgument` for a visibility that is unset or unknown.
    pub fn from_wire(visibility: &[i32], tool: &str) -> Result<Self, ModuleError> {
        let (mut model, mut app) = (visibility.is_empty(), visibility.is_empty());
        for named in visibility {
            match wire::ToolVisibility::try_from(*named) {
                Ok(wire::ToolVisibility::Model) => model = true,
                Ok(wire::ToolVisibility::App) => app = true,
                Ok(wire::ToolVisibility::Unspecified) | Err(_) => {
                    return Err(ModuleError::new(
                        ModuleErrorKind::InvalidArgument,
                        format!("tool {tool}: visibility {named} is unknown"),
                    ));
                }
            }
        }
        Ok(match (model, app) {
            (true, true) => Self::Both,
            (true, false) => Self::Model,
            (false, _) => Self::App,
        })
    }

    /// Its wire `visibility`: none for both (as MCP Apps).
    #[must_use]
    pub fn to_wire(self) -> Vec<i32> {
        match self {
            Self::Both => Vec::new(),
            Self::Model => vec![wire::ToolVisibility::Model.into()],
            Self::App => vec![wire::ToolVisibility::App.into()],
        }
    }
}

/// What a tool answers an agent: MCP's text and structured content.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// The text an agent reads.
    pub text: String,
    /// The structured result, when the tool has one.
    pub data: Option<Value>,
    /// The call failed yet answers with its data (a bulk write's per-item
    /// results).
    pub failed: bool,
}

impl ToolSpec {
    /// A tool agents and views may call, doing `effect`, whose input is an
    /// object of `properties` (a JSON object of JSON Schemas), the
    /// `required` ones named, and no other field.
    #[must_use]
    pub fn object(
        name: &str,
        description: &str,
        effect: Effect,
        properties: Value,
        required: &[&str],
    ) -> Self {
        let mut schema = Map::new();
        schema.insert("type".into(), Value::from("object"));
        schema.insert("properties".into(), properties);
        schema.insert("required".into(), Value::from(required.to_vec()));
        schema.insert("additionalProperties".into(), Value::Bool(false));
        Self {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: schema,
            visibility: Visibility::Both,
            effect,
        }
    }
}

impl ToolOutput {
    /// A tool's answer: `text` for an agent, `data` for a program.
    #[must_use]
    pub fn answer(text: String, data: Value) -> Self {
        Self {
            text,
            data: Some(data),
            failed: false,
        }
    }
}

impl From<ToolSpec> for wire::ToolSpec {
    /// Both is no visibility, as MCP Apps reads it.
    fn from(spec: ToolSpec) -> Self {
        let visibility = match spec.visibility {
            Visibility::Both => Vec::new(),
            Visibility::Model => vec![wire::ToolVisibility::Model.into()],
            Visibility::App => vec![wire::ToolVisibility::App.into()],
        };
        Self {
            name: spec.name,
            description: spec.description,
            input_schema: Some(json::object_to_wire(spec.input_schema)),
            visibility,
            effect: spec.effect.into(),
        }
    }
}

impl TryFrom<wire::ToolSpec> for ToolSpec {
    type Error = ModuleError;

    /// A tool without a schema takes any object; one without a visibility
    /// is for both.
    ///
    /// # Errors
    /// `InvalidArgument` for a visibility or an effect that is unset or
    /// unknown.
    fn try_from(spec: wire::ToolSpec) -> Result<Self, ModuleError> {
        let visibility = Visibility::from_wire(&spec.visibility, &spec.name)?;
        let effect = effect_from_wire(spec.effect, &spec.name)?;
        Ok(Self {
            name: spec.name,
            description: spec.description,
            input_schema: spec
                .input_schema
                .map(json::object_from_wire)
                .unwrap_or_default(),
            visibility,
            effect,
        })
    }
}
