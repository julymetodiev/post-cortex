// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx_module._core`: the Rust core of a pcx module for Python
//! (docs/modules/dynamic.md, the bindings' API). The core serves on its own
//! runtime; Python's worker threads take each call (`Core::next_call`),
//! run its handler and answer it, without the GIL (free-threaded CPython).
//! Only bytes and plain values cross; `pcx_module` (Python) is the layer a
//! module's author writes against.

use pcx_module_core::handoff::{self, Call as Handed, Declaration, Declared, Emitted, Handoff};
use pcx_module_core::{ModuleError as Failure, Scope};
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

create_exception!(
    _core,
    ModuleError,
    PyException,
    "A failed call: `args` are its kind (`not_found`, `denied`, ...) and its message."
);

/// `failure` as the Python exception.
fn raised(failure: Failure) -> PyErr {
    ModuleError::new_err((failure.kind.name(), failure.message))
}

/// A failure of the core itself (`internal`).
fn broken(e: impl std::fmt::Display) -> PyErr {
    ModuleError::new_err(("internal", e.to_string()))
}

/// An RPC as Python declares it: method, tool, effect, visibility.
type Rpc = (String, String, String, String);

/// An event as Python declares it: name, message type, description.
type Event = (String, String, String);

/// What `pcx_module.Module` declares, a dict of these keys.
#[derive(FromPyObject)]
#[pyo3(from_item_all)]
struct Declaring {
    id: String,
    version: String,
    description: String,
    services: Vec<String>,
    descriptors: Vec<u8>,
    rpcs: Vec<Rpc>,
    uses: Vec<String>,
    emits: Vec<Event>,
    follows: Vec<String>,
    config_type: String,
}

impl Declaring {
    fn declaration(self) -> PyResult<Declaration> {
        let rpcs = self
            .rpcs
            .into_iter()
            .map(|(method, tool, effect, visibility)| {
                Ok(Declared {
                    method,
                    tool,
                    effect: handoff::effect(&effect).map_err(raised)?,
                    visibility: handoff::visibility(&visibility).map_err(raised)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        let emits = self
            .emits
            .into_iter()
            .map(|(name, message, description)| Emitted {
                name,
                message,
                description,
            })
            .collect();
        Ok(Declaration {
            id: self.id,
            version: self.version,
            description: self.description,
            services: self.services,
            descriptors: self.descriptors,
            rpcs,
            uses: self.uses,
            emits,
            follows: self.follows,
            config_type: self.config_type,
        })
    }
}

/// The core of one module process.
#[pyclass(frozen)]
struct Core {
    handoff: Handoff,
}

#[pymethods]
impl Core {
    /// The core of the module `declaring` declares: its manifest is made
    /// from it.
    #[new]
    fn new(declaring: Declaring) -> PyResult<Self> {
        let handoff = Handoff::new(declaring.declaration()?).map_err(broken)?;
        Ok(Self { handoff })
    }

    /// Serve on the host's socket until it stops the module.
    fn serve(&self, py: Python<'_>) -> PyResult<()> {
        let handoff = self.handoff.clone();
        py.detach(move || handoff.runtime().block_on(handoff.serve()))
            .map_err(broken)
    }

    /// The next call; none once the module stopped.
    fn next_call(&self, py: Python<'_>) -> Option<Call> {
        let handoff = self.handoff.clone();
        py.detach(move || handoff.runtime().block_on(handoff.next_call()))
            .map(Call::from)
    }

    /// Whether call `id` still waits for its answer (false: its caller
    /// gave up).
    fn waits(&self, id: u64) -> bool {
        self.handoff.waits(id)
    }

    /// Answer call `id` with the response's bytes; false when nothing
    /// waits for it any more (its deadline passed).
    fn complete(&self, id: u64, body: Vec<u8>) -> bool {
        self.handoff.complete(id, body).is_ok()
    }

    /// Fail call `id` with a failure of `kind` (as `ModuleError`'s);
    /// false when nothing waits for it any more.
    fn fail(&self, id: u64, kind: &str, message: String) -> bool {
        self.handoff
            .fail(id, handoff::failure(kind, message))
            .is_ok()
    }

    /// Call `path` of a module this one uses, in call `id`'s scope: the
    /// answer's bytes.
    fn host_call<'py>(
        &self,
        py: Python<'py>,
        id: u64,
        path: String,
        body: Vec<u8>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let handoff = self.handoff.clone();
        let answer = py
            .detach(move || {
                handoff
                    .runtime()
                    .block_on(handoff.host_call(id, &path, body))
            })
            .map_err(raised)?;
        Ok(PyBytes::new(py, &answer))
    }

    /// Emit the event `name`, a `message` of `body`, in call `id`'s scope:
    /// its id.
    fn publish(
        &self,
        py: Python<'_>,
        id: u64,
        name: String,
        message: String,
        body: Vec<u8>,
    ) -> PyResult<String> {
        let handoff = self.handoff.clone();
        py.detach(move || {
            handoff
                .runtime()
                .block_on(handoff.publish(id, &name, &message, body))
        })
        .map_err(raised)
    }

    /// The configuration's message type and bytes (empty: none).
    fn config<'py>(&self, py: Python<'py>) -> PyResult<(String, Bound<'py, PyBytes>)> {
        let (message, body) = self.handoff.config().map_err(raised)?;
        Ok((message, PyBytes::new(py, &body)))
    }
}

/// A call handed to Python.
#[pyclass(frozen)]
struct Call {
    #[pyo3(get)]
    id: u64,
    #[pyo3(get)]
    method: String,
    body: Vec<u8>,
    scope: Scope,
}

impl From<Handed> for Call {
    fn from(call: Handed) -> Self {
        Self {
            id: call.id,
            method: call.method,
            body: call.body,
            scope: call.scope,
        }
    }
}

#[pymethods]
impl Call {
    /// The request's bytes.
    #[getter]
    fn body<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.body)
    }

    /// Who calls (a person, an agent, a module); none: unknown.
    #[getter]
    fn author(&self) -> Option<String> {
        self.scope.author.clone()
    }

    /// The workspace it is for; none: the caller's default.
    #[getter]
    fn workspace(&self) -> Option<String> {
        self.scope.workspace_id.clone()
    }

    /// The module that calls; none: a person or an agent.
    #[getter]
    fn caller(&self) -> Option<String> {
        self.scope.caller_module.clone()
    }

    /// Seconds left before its deadline.
    #[getter]
    fn remaining(&self) -> f64 {
        self.scope.remaining().as_secs_f64()
    }
}

#[pymodule(gil_used = false)]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Core>()?;
    m.add_class::<Call>()?;
    m.add("ModuleError", m.py().get_type::<ModuleError>())?;
    Ok(())
}
