//! Compile and evaluate JSON Schema documents.
//!
//! Callers never see this module. `execute` in the crate root is the supported
//! entry point; swapping the engine later is safe as long as that JSON contract
//! stays intact.

use std::sync::{Arc, OnceLock};

use jsonschema::{Retrieve, Validator};
use serde_json::Value;

use crate::detect::{self, Detected};
use crate::errors::{self, map_validation_error};
use crate::pointer::{self, ROOT_URI};

pub struct Evaluation {
    pub draft: String,
    pub detected: Option<Detected>,
    pub init_error: Option<Value>,
    pub errors: Vec<Value>,
}

/// A schema compiled once and held for later instances.
///
/// `validate_instance` does not see the original document again. `spawn` builds
/// another compiled schema for a pointer into the document stored here.
#[derive(Debug)]
pub(crate) struct Prepared {
    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    document: Arc<Value>,
    pub draft: String,
    pub detected: Option<Detected>,
    validator: Validator,
}

#[derive(Debug)]
pub(crate) struct PrepareFailure {
    pub draft: String,
    pub detected: Option<Detected>,
    pub error: Value,
}

impl Prepared {
    pub(crate) fn validate_instance(&self, instance: &Value) -> Vec<Value> {
        self.validator
            .iter_errors(instance)
            .map(|error| map_validation_error(&error))
            .collect()
    }

    #[cfg_attr(not(any(test, target_arch = "wasm32")), allow(dead_code))]
    pub(crate) fn spawn(&self, pointer: &str) -> Result<Prepared, PrepareFailure> {
        prepare(self.document.as_ref(), pointer, Some(self.draft.as_str()))
    }
}

struct RootDocument(Value);

impl Retrieve for RootDocument {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let without_fragment = uri.as_str().split('#').next().unwrap_or("");
        if without_fragment == ROOT_URI {
            return Ok(self.0.clone());
        }
        Err(format!("unresolved schema reference: {}", uri.as_str()).into())
    }
}

pub fn compile_and_validate(
    schema: &Value,
    pointer: &str,
    instance: Option<&Value>,
    draft_override: Option<&str>,
) -> Evaluation {
    match prepare(schema, pointer, draft_override) {
        Ok(prepared) => {
            let errors = match instance {
                Some(instance) => prepared.validate_instance(instance),
                None => Vec::new(),
            };
            Evaluation {
                draft: prepared.draft,
                detected: prepared.detected,
                init_error: None,
                errors,
            }
        }
        Err(failure) => Evaluation {
            draft: failure.draft,
            detected: failure.detected,
            init_error: Some(failure.error),
            errors: Vec::new(),
        },
    }
}

pub(crate) fn prepare(
    schema: &Value,
    pointer: &str,
    draft_override: Option<&str>,
) -> Result<Prepared, PrepareFailure> {
    let detected = detect::detect(schema);
    let draft = detect::draft_for(schema, draft_override);

    if !detect::skips_json_schema_meta(schema) {
        match meta_errors(&draft, schema) {
            Ok(errors) if !errors.is_empty() => {
                let detail = errors
                    .iter()
                    .filter_map(|error| error.get("message").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(fail(
                    draft,
                    detected,
                    errors::init_error(format!("Invalid schema definition: {detail}")),
                ));
            }
            Err(message) => return Err(fail(draft, detected, errors::init_error(message))),
            Ok(_) => {}
        }
    }

    let fragment = match pointer::extract_fragment(schema, pointer) {
        Ok(fragment) => fragment,
        Err(message) => return Err(fail(draft, detected, errors::init_error(message))),
    };

    let is_root = pointer == "#/" || pointer == "#";
    let mut executable = fragment;
    if !is_root {
        pointer::rewrite_document_refs(&mut executable);
    }

    let document = Arc::new(schema.clone());
    match build_validator(&draft, &executable, schema.clone()) {
        Ok(validator) => Ok(Prepared {
            document,
            draft,
            detected,
            validator,
        }),
        Err(message) => Err(fail(draft, detected, errors::init_error(message))),
    }
}

fn fail(draft: String, detected: Option<Detected>, error: Value) -> PrepareFailure {
    PrepareFailure {
        draft,
        detected,
        error,
    }
}

pub fn schema_document_errors(schema: &Value, draft: &str) -> Result<Vec<Value>, String> {
    meta_errors(draft, schema)
}

fn build_validator(draft: &str, schema: &Value, document: Value) -> Result<Validator, String> {
    build_with(draft, schema, RootDocument(document))
}

/// Compile a specification schema whose external `$ref`s point at schemas
/// bundled in its own `definitions` object.
pub(crate) fn compile_bundled(schema: &Value) -> Result<(String, Validator), String> {
    let draft = detect::draft_for(schema, None);
    let validator = build_with(&draft, schema, BundledDocument(schema.clone()))?;
    Ok((draft, validator))
}

struct BundledDocument(Value);

impl Retrieve for BundledDocument {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let raw = uri.as_str();
        let base = raw.split('#').next().unwrap_or(raw);
        bundled_definition(&self.0, base)
            .ok_or_else(|| format!("unresolved schema reference: {raw}").into())
    }
}

fn bundled_definition(root: &Value, uri: &str) -> Option<Value> {
    let defs = root.get("definitions").or_else(|| root.get("$defs"))?.as_object()?;
    if let Some(schema) = defs.get(uri) {
        return Some(schema.clone());
    }
    let wanted = uri.trim_end_matches('#');
    defs.values().find(|schema| {
        schema
            .get("$id")
            .or_else(|| schema.get("id"))
            .and_then(Value::as_str)
            .is_some_and(|id| id.trim_end_matches('#') == wanted)
    }).cloned()
}

fn build_with(draft: &str, schema: &Value, retriever: impl Retrieve + 'static) -> Result<Validator, String> {
    let result = match draft {
        "draft-04" => jsonschema::draft4::options()
            .with_retriever(retriever)
            .should_validate_formats(true)
            .should_ignore_unknown_formats(true)
            .build(schema),
        "draft-06" => jsonschema::draft6::options()
            .with_retriever(retriever)
            .should_validate_formats(true)
            .should_ignore_unknown_formats(true)
            .build(schema),
        "draft-07" => jsonschema::draft7::options()
            .with_retriever(retriever)
            .should_validate_formats(true)
            .should_ignore_unknown_formats(true)
            .build(schema),
        "2019-09" => jsonschema::draft201909::options()
            .with_retriever(retriever)
            .should_validate_formats(true)
            .should_ignore_unknown_formats(true)
            .build(schema),
        "2020-12" => jsonschema::draft202012::options()
            .with_retriever(retriever)
            .should_validate_formats(true)
            .should_ignore_unknown_formats(true)
            .build(schema),
        other => return Err(format!("unsupported JSON Schema draft: {other}")),
    };
    result.map_err(|error| error.to_string())
}

fn meta_errors(draft: &str, schema: &Value) -> Result<Vec<Value>, String> {
    // The jsonschema crate rebuilds an owned meta-validator on every wasm call.
    // Keep one compiled copy per draft for the life of the module.
    let validator = match draft {
        "draft-04" => cached_meta(&DRAFT4_META, jsonschema::draft4::meta::validator),
        "draft-06" => cached_meta(&DRAFT6_META, jsonschema::draft6::meta::validator),
        "draft-07" => cached_meta(&DRAFT7_META, jsonschema::draft7::meta::validator),
        "2019-09" => cached_meta(&DRAFT201909_META, jsonschema::draft201909::meta::validator),
        "2020-12" => cached_meta(&DRAFT202012_META, jsonschema::draft202012::meta::validator),
        other => return Err(format!("unsupported JSON Schema draft: {other}")),
    };
    Ok(collect_meta(validator, schema))
}

fn cached_meta(
    slot: &'static OnceLock<jsonschema::meta::MetaValidator<'static>>,
    build: fn() -> jsonschema::meta::MetaValidator<'static>,
) -> &'static jsonschema::meta::MetaValidator<'static> {
    slot.get_or_init(build)
}

static DRAFT4_META: OnceLock<jsonschema::meta::MetaValidator<'static>> = OnceLock::new();
static DRAFT6_META: OnceLock<jsonschema::meta::MetaValidator<'static>> = OnceLock::new();
static DRAFT7_META: OnceLock<jsonschema::meta::MetaValidator<'static>> = OnceLock::new();
static DRAFT201909_META: OnceLock<jsonschema::meta::MetaValidator<'static>> = OnceLock::new();
static DRAFT202012_META: OnceLock<jsonschema::meta::MetaValidator<'static>> = OnceLock::new();

fn collect_meta(validator: &jsonschema::meta::MetaValidator<'_>, schema: &Value) -> Vec<Value> {
    validator
        .iter_errors(schema)
        .map(|error| map_validation_error(&error))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_compiled_schema_validates_later_instances() {
        let schema = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });
        let prepared = prepare(&schema, "#/", None).expect("schema compiles");
        drop(schema);

        assert!(prepared.validate_instance(&json!({ "name": "Ada" })).is_empty());
        let errors = prepared.validate_instance(&json!({}));
        assert_eq!(errors[0]["type"], "MISSING_REQUIRED_FIELD");
    }

    #[test]
    fn spawn_compiles_a_fragment_from_the_stored_document() {
        let prepared = {
            let schema = json!({
                "definitions": {
                    "Name": { "type": "string", "minLength": 2 }
                },
                "type": "object"
            });
            prepare(&schema, "#/", None).expect("schema compiles")
        };

        let name = prepared.spawn("#/definitions/Name").expect("fragment compiles");
        assert!(!name.validate_instance(&json!("a")).is_empty());
        assert!(name.validate_instance(&json!("Ada")).is_empty());
    }

    #[test]
    fn spawn_follows_a_ref_chain_to_the_concrete_schema() {
        let prepared = prepare(
            &json!({
                "openapi": "3.1.0",
                "paths": {
                    "/users/{id}": {
                        "get": {
                            "parameters": [{ "$ref": "#/components/parameters/UserIdAlias" }]
                        }
                    }
                },
                "components": {
                    "parameters": {
                        "UserIdAlias": { "$ref": "#/components/parameters/UserId" },
                        "UserId": {
                            "name": "id",
                            "in": "path",
                            "schema": { "$ref": "#/components/schemas/UserId" }
                        }
                    },
                    "schemas": {
                        "UserId": { "$ref": "#/components/schemas/IdFormat" },
                        "IdFormat": { "type": "string", "minLength": 3 }
                    }
                }
            }),
            "#/",
            None,
        )
        .expect("document compiles");

        let parameter = prepared
            .spawn("#/paths/~1users~1{id}/get/parameters/0/schema")
            .expect("pointer follows the ref chain");
        assert!(parameter.validate_instance(&json!("abc")).is_empty());
        assert!(!parameter.validate_instance(&json!("ab")).is_empty());
    }

    #[test]
    fn an_invalid_schema_is_not_compiled() {
        let failure = prepare(&json!({ "type": "invalid-type" }), "#/", None).expect_err("init fails");
        assert_eq!(failure.error["type"], "UNKNOWN_ERROR");
        assert!(failure.error["message"].as_str().unwrap().contains("Initialization failed"));
    }
}
