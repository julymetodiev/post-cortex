// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A preset checked (docs/modules/http.md, presets): its names, its
//! method and path, its parameters (the path's places are its `path`
//! parameters, each type parses, a body field is named as CEL names one),
//! its response's fields, the connection form it fills.

use pcx_expr::Type;
use pcx_expr::schema;
use pcx_module_api::name::is_slug;
use post_cortex_proto::http::{Auth, Place, Preset};

use crate::domain::request::Method;
use crate::domain::request::headers::header_name;
use crate::domain::request::path::{Piece, pieces};
use crate::domain::{Broken, base_url};

/// Check `preset`; `built_in`: the names of the built-in presets, which no
/// other may take.
///
/// # Errors
/// The first field at fault and why.
pub fn check(preset: &Preset, built_in: &[String]) -> Result<(), Broken> {
    if !is_slug(&preset.name) {
        return Err(Broken::new("name", "is 1 to 64 of a-z, 0-9 and '-'"));
    }
    if built_in.contains(&preset.name) {
        return Err(Broken::new(
            "name",
            format!("{} is a built-in preset's", preset.name),
        ));
    }
    if !is_slug(&preset.service) {
        return Err(Broken::new("service", "is 1 to 64 of a-z, 0-9 and '-'"));
    }
    if !preset.connection.is_empty() && !is_slug(&preset.connection) {
        return Err(Broken::new("connection", "is a connection's name"));
    }
    if !preset.base_url.is_empty() {
        base_url(&preset.base_url, false).map_err(|why| Broken::new("base_url", why))?;
    }
    match Auth::try_from(preset.auth) {
        Ok(Auth::Header | Auth::Query) if preset.auth_name.trim().is_empty() => {
            return Err(Broken::new(
                "auth_name",
                "is required by its authentication",
            ));
        }
        Ok(_) => {}
        Err(_) => {
            return Err(Broken::new(
                "auth",
                format!("{} is no authentication", preset.auth),
            ));
        }
    }
    for (i, h) in preset.headers.iter().enumerate() {
        header_name(&h.name.to_ascii_lowercase())
            .map_err(|why| Broken::new(&format!("headers[{i}].name"), why))?;
    }
    Method::parse(&preset.method)?;
    let places: Vec<&str> = pieces(&preset.path)?
        .into_iter()
        .filter_map(|p| match p {
            Piece::Place(name) => Some(name),
            Piece::Text(_) => None,
        })
        .collect();
    params(preset, &places)?;
    if let Some(response) = &preset.response {
        schema::fields(
            response
                .fields
                .iter()
                .map(|f| (f.name.as_str(), f.r#type.as_str(), f.required)),
        )
        .map_err(|(i, part, why)| Broken::new(&format!("response.fields[{i}].{part}"), why))?;
    } else if preset.response_list {
        return Err(Broken::new("response_list", "is given with response"));
    }
    Ok(())
}

/// Check the parameters of `preset`, whose path has `places`.
fn params(preset: &Preset, places: &[&str]) -> Result<(), Broken> {
    let mut seen: Vec<(i32, &str)> = Vec::new();
    for (i, p) in preset.params.iter().enumerate() {
        let at = |part: &str| format!("params[{i}].{part}");
        let place = match Place::try_from(p.r#in) {
            Ok(place) if place != Place::Unspecified => place,
            _ => return Err(Broken::new(&at("in"), "is path, query, header or body")),
        };
        if p.name.is_empty() {
            return Err(Broken::new(&at("name"), "is required"));
        }
        if seen.contains(&(p.r#in, p.name.as_str())) {
            return Err(Broken::new(
                &at("name"),
                format!("{} is a parameter already", p.name),
            ));
        }
        seen.push((p.r#in, p.name.as_str()));
        Type::parse(&p.r#type).map_err(|why| Broken::new(&at("type"), why))?;
        match place {
            Place::Path if !places.contains(&p.name.as_str()) => {
                return Err(Broken::new(&at("name"), "names no {place} of the path"));
            }
            Place::Body => {
                schema::fields([(p.name.as_str(), p.r#type.as_str(), p.required)])
                    .map_err(|(_, part, why)| Broken::new(&at(part), why))?;
            }
            Place::Header => {
                header_name(&p.name.to_ascii_lowercase())
                    .map_err(|why| Broken::new(&at("name"), why))?;
            }
            _ => {}
        }
    }
    if let Some(missing) = places.iter().find(|name| {
        !preset
            .params
            .iter()
            .any(|p| p.r#in == i32::from(Place::Path) && p.name == **name)
    }) {
        return Err(Broken::new(
            "path",
            format!("{{{missing}}} is no path parameter of the preset"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
