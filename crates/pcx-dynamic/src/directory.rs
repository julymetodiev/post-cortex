// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A bundle read from a directory of the caller's `tools-fs`
//! (docs/modules/dynamic.md, what a bundle holds), so an author installs what
//! it tested, and no text passes through its model.

use async_trait::async_trait;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope, UsedTool};
use post_cortex_proto::dynamic::File;
use prost_reflect::{DynamicMessage, Value};

use crate::adapters::HostPorts;
use crate::domain;
use crate::ports::Directory;

/// The `tools-fs` RPCs a bundle's directory is read through.
const GLOB: &str = "/pcx.tools.fs.v1.Files/Glob";
const READ: &str = "/pcx.tools.fs.v1.Files/Read";

#[async_trait]
impl Directory for HostPorts {
    async fn files(&self, scope: &Scope, dir: &str) -> Result<Vec<File>, ModuleError> {
        let tools = self.host.tools(scope).await?;
        let tool = |path: &str| {
            tools.iter().find(|u| u.tool.path() == path).ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!("its author reaches no {path}: give the files in the call"),
                )
            })
        };
        let (glob, read) = (tool(GLOB)?, tool(READ)?);
        let mut asked = DynamicMessage::new(glob.tool.request().clone());
        asked.set_field_by_name("pattern", Value::String("**/*".to_string()));
        asked.set_field_by_name("path", Value::String(dir.to_string()));
        let found = glob
            .tool
            .call_message(self.channel.clone(), scope, &asked, None)
            .await?;
        let mut files = Vec::new();
        for path in strings(&found, "files") {
            let Some(beneath) = domain::beneath(dir, &path).filter(|p| domain::bundled(p)) else {
                continue;
            };
            if files.len() == domain::MAX_FILES {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    format!("{dir}: more than {} files", domain::MAX_FILES),
                ));
            }
            files.push(File {
                path: beneath.to_string(),
                text: self.read_all(scope, read, &path).await?,
            });
        }
        Ok(files)
    }
}

impl HostPorts {
    /// The whole text of the file at `path`, as many reads as it takes.
    async fn read_all(
        &self,
        scope: &Scope,
        read: &UsedTool,
        path: &str,
    ) -> Result<String, ModuleError> {
        let mut lines = Vec::new();
        loop {
            let mut asked = DynamicMessage::new(read.tool.request().clone());
            asked.set_field_by_name("path", Value::String(path.to_string()));
            asked.set_field_by_name("offset", Value::U64(lines.len() as u64 + 1));
            let answer = read
                .tool
                .call_message(self.channel.clone(), scope, &asked, None)
                .await?;
            let got = strings(&answer, "lines");
            let total = answer
                .get_field_by_name("total_lines")
                .and_then(|v| v.as_u64())
                .unwrap_or_default();
            let none = got.is_empty();
            lines.extend(got);
            if none || lines.len() as u64 >= total {
                break;
            }
        }
        Ok(lines
            .iter()
            .fold(String::new(), |text, line| text + line + "\n"))
    }
}

/// The list of strings in field `name` of `message`.
fn strings(message: &DynamicMessage, name: &str) -> Vec<String> {
    let Some(field) = message.get_field_by_name(name) else {
        return Vec::new();
    };
    field.as_list().map_or_else(Vec::new, |list| {
        list.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    })
}
