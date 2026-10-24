//! Official OpenAPI and AsyncAPI schemas, compiled once per process.
//!
//! The JSON files in `specs/` are the schemas the reference validator stored
//! under `schema/cached`. A document is checked against the compiled schema for
//! its family and version. JSON Schema fragments inside those documents stay
//! on the data-validator path.

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::{json, Value};

use crate::engine;
use crate::errors;

struct CatalogEntry {
    family: &'static str,
    version: &'static str,
    schema: &'static str,
}

const CATALOG: &[CatalogEntry] = &[
    CatalogEntry { family: "openapi", version: "3.0", schema: include_str!("../specs/openapi-3.0.json") },
    CatalogEntry { family: "openapi", version: "3.1", schema: include_str!("../specs/openapi-3.1.json") },
    CatalogEntry { family: "asyncapi", version: "2.0.0", schema: include_str!("../specs/asyncapi-2.0.0.json") },
    CatalogEntry { family: "asyncapi", version: "2.1.0", schema: include_str!("../specs/asyncapi-2.1.0.json") },
    CatalogEntry { family: "asyncapi", version: "2.2.0", schema: include_str!("../specs/asyncapi-2.2.0.json") },
    CatalogEntry { family: "asyncapi", version: "2.3.0", schema: include_str!("../specs/asyncapi-2.3.0.json") },
    CatalogEntry { family: "asyncapi", version: "2.4.0", schema: include_str!("../specs/asyncapi-2.4.0.json") },
    CatalogEntry { family: "asyncapi", version: "2.5.0", schema: include_str!("../specs/asyncapi-2.5.0.json") },
    CatalogEntry { family: "asyncapi", version: "2.6.0", schema: include_str!("../specs/asyncapi-2.6.0.json") },
    CatalogEntry { family: "asyncapi", version: "3.0.0", schema: include_str!("../specs/asyncapi-3.0.0.json") },
];

struct CompiledSpec {
    draft: String,
    validator: Validator,
}

pub fn specifications() -> Value {
    json!({
        "openapi": ["3.0", "3.1"],
        "asyncapi": ["2.0.0", "2.1.0", "2.2.0", "2.3.0", "2.4.0", "2.5.0", "2.6.0", "3.0.0"],
    })
}

/// Catalog key for a detected or requested version. OpenAPI groups `3.0.3`
/// with `3.0`. AsyncAPI versions match the published schema exactly.
pub fn canonical<'a>(family: &str, version: &'a str) -> Option<&'a str> {
    match family {
        "openapi" => openapi_line(version),
        "asyncapi" if CATALOG.iter().any(|entry| entry.family == "asyncapi" && entry.version == version) => {
            Some(version)
        }
        _ => None,
    }
}

pub fn same_version(family: &str, detected: &str, requested: &str) -> bool {
    match (canonical(family, detected), canonical(family, requested)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Compile the specification schema if this request is going to use one.
/// Running this before the per-request meter keeps the one-time compile out of
/// the caller's CPU and memory budget.
pub fn warm(family: &str, version: &str) {
    let _ = ensure(family, version);
}

pub fn validate_document(family: &str, version: &str, document: &Value) -> Result<(String, Vec<Value>), String> {
    let compiled = ensure(family, version)?;
    let errors = compiled
        .validator
        .iter_errors(document)
        .map(|error| errors::map_validation_error(&error))
        .collect();
    Ok((compiled.draft.clone(), errors))
}

fn ensure(family: &str, version: &str) -> Result<&'static CompiledSpec, String> {
    let key = canonical(family, version).ok_or_else(|| format!("unsupported {family} version: {version}"))?;
    let entry = CATALOG
        .iter()
        .find(|entry| entry.family == family && entry.version == key)
        .expect("canonical version is in the catalog");
    let slot = slot(entry.family, entry.version).expect("catalog entry has a cache slot");
    let compiled = slot.get_or_init(|| {
        let schema: Value = serde_json::from_str(entry.schema).map_err(|error| error.to_string())?;
        let (draft, validator) = engine::compile_bundled(&schema)?;
        Ok(CompiledSpec { draft, validator })
    });
    match compiled {
        Ok(spec) => Ok(spec),
        Err(message) => Err(message.clone()),
    }
}

fn slot(family: &str, version: &str) -> Option<&'static OnceLock<Result<CompiledSpec, String>>> {
    Some(match (family, version) {
        ("openapi", "3.0") => &OPENAPI_30,
        ("openapi", "3.1") => &OPENAPI_31,
        ("asyncapi", "2.0.0") => &ASYNCAPI_200,
        ("asyncapi", "2.1.0") => &ASYNCAPI_210,
        ("asyncapi", "2.2.0") => &ASYNCAPI_220,
        ("asyncapi", "2.3.0") => &ASYNCAPI_230,
        ("asyncapi", "2.4.0") => &ASYNCAPI_240,
        ("asyncapi", "2.5.0") => &ASYNCAPI_250,
        ("asyncapi", "2.6.0") => &ASYNCAPI_260,
        ("asyncapi", "3.0.0") => &ASYNCAPI_300,
        _ => return None,
    })
}

fn openapi_line(version: &str) -> Option<&str> {
    if version == "3.0" || version.starts_with("3.0.") {
        Some("3.0")
    } else if version == "3.1" || version.starts_with("3.1.") {
        Some("3.1")
    } else {
        None
    }
}

static OPENAPI_30: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static OPENAPI_31: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_200: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_210: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_220: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_230: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_240: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_250: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_260: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();
static ASYNCAPI_300: OnceLock<Result<CompiledSpec, String>> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_patch_versions_share_a_schema() {
        assert_eq!(canonical("openapi", "3.0"), Some("3.0"));
        assert_eq!(canonical("openapi", "3.0.3"), Some("3.0"));
        assert!(same_version("openapi", "3.0.3", "3.0"));
        assert!(!same_version("openapi", "3.1.0", "3.0"));
        assert_eq!(canonical("openapi", "2.0"), None);
    }

    #[test]
    fn asyncapi_versions_are_exact() {
        assert!(same_version("asyncapi", "2.6.0", "2.6.0"));
        assert!(!same_version("asyncapi", "2.6.0", "2.6"));
        assert!(!same_version("asyncapi", "3.0.0", "2.6.0"));
    }
}
