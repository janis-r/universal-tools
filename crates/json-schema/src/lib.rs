//! JSON Schema tool.
//!
//! The supported interface is [`execute_json`]: one JSON request in, one JSON
//! response out. The request and response are checked against the contracts in
//! `contracts/json-schema/`. Resource use is reported on every response and
//! compared with the budget in the manifest.

#[cfg(target_arch = "wasm32")]
mod abi;
mod detect;
mod engine;
mod errors;
mod pointer;
mod specs;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use serde_json::{json, Value};
use ut_contract::{burn, measure, ResourceBudget, ResourceSample, DEFAULT_MAX_ELAPSED_MICROS, DEFAULT_MAX_PEAK_BYTES};

const INPUT_SCHEMA: &str = include_str!("../../../contracts/json-schema/input.schema.json");
const OUTPUT_SCHEMA: &str = include_str!("../../../contracts/json-schema/output.schema.json");
const MANIFEST: &str = include_str!("../../../contracts/json-schema/manifest.json");

const KEYWORDS: &[&str] = &[
    "type",
    "enum",
    "const",
    "multipleOf",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "maxLength",
    "minLength",
    "pattern",
    "maxItems",
    "minItems",
    "uniqueItems",
    "maxContains",
    "minContains",
    "maxProperties",
    "minProperties",
    "required",
    "dependentRequired",
    "dependencies",
    "dependentSchemas",
    "properties",
    "patternProperties",
    "additionalProperties",
    "propertyNames",
    "items",
    "prefixItems",
    "additionalItems",
    "contains",
    "unevaluatedItems",
    "unevaluatedProperties",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "format",
    "$ref",
    "$defs",
    "definitions",
    "$anchor",
    "$dynamicRef",
    "$dynamicAnchor",
    "$recursiveRef",
    "$recursiveAnchor",
];

/// Run one black-box request. The returned string is always JSON.
pub fn execute_json(request: &str) -> String {
    match catch_unwind(AssertUnwindSafe(|| execute_value(request))) {
        Ok(value) => value.to_string(),
        Err(payload) => fallback_response(&panic_message(payload)).to_string(),
    }
}

fn execute_value(request: &str) -> Value {
    let (mut response, budget) = measure_dispatch(request);
    let sample = response
        .get("sample")
        .cloned()
        .expect("meter attaches a sample");
    let sample = decode_sample(&sample);
    response.as_object_mut().expect("response object").remove("sample");
    seal(response, sample, budget)
}

fn seal(mut response: Value, sample: ResourceSample, budget: ResourceBudget) -> Value {
    let within = sample.within_budget(budget);
    if !within {
        response["ok"] = Value::Bool(false);
        response["error"] = json!({
            "code": "RESOURCE_LIMIT",
            "message": format!(
                "execution used {}us and {} peak bytes; budget is {}us and {} bytes",
                sample.elapsed_micros, sample.peak_bytes, budget.max_elapsed_micros, budget.max_peak_bytes
            )
        });
    }
    response["resources"] = resources_json(sample, budget, within);

    if !output_is_valid(&response) {
        return contract_violation(&response, sample, budget);
    }
    response
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn validate_prepared(prepared: &engine::Prepared, instance_json: &str) -> Value {
    let budget = ResourceBudget::default();
    let (response, sample) = measure(|| match serde_json::from_str::<Value>(instance_json) {
        Ok(instance) => {
            let errors = prepared.validate_instance(&instance);
            success(
                "validateData",
                json!({
                    "valid": errors.is_empty(),
                    "errors": errors,
                    "draft": prepared.draft,
                    "detected": detected_json(prepared.detected.as_ref()),
                }),
            )
        }
        Err(error) => failure(
            "validateData",
            "INVALID_INPUT",
            format!("instance is not JSON: {error}"),
            None,
        ),
    });
    seal(response, sample, budget)
}

fn measure_dispatch(request: &str) -> (Value, ResourceBudget) {
    if let Ok(parsed) = serde_json::from_str::<Value>(request) {
        warm_specification(&parsed);
    }
    let mut budget = ResourceBudget::default();
    let (response, sample) = measure(|| {
        let (response, parsed_budget) = dispatch(request);
        budget = parsed_budget;
        response
    });
    let mut response = response;
    response["sample"] = encode_sample(sample);
    (response, budget)
}

fn dispatch(request: &str) -> (Value, ResourceBudget) {
    let parsed = match serde_json::from_str::<Value>(request) {
        Ok(value) => value,
        Err(error) => {
            return (
                failure("unknown", "INVALID_INPUT", format!("request is not JSON: {error}"), None),
                ResourceBudget::default(),
            );
        }
    };
    let budget = budget_from(&parsed);
    let operation = parsed
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();

    let input_errors = match validate_contract(input_validator(), &parsed) {
        Ok(errors) => errors,
        Err(message) => {
            return (
                failure(&operation, "INTERNAL", message, None),
                budget,
            );
        }
    };
    if !input_errors.is_empty() {
        return (
            failure(
                &operation,
                "INVALID_INPUT",
                "request does not match the input contract",
                Some(input_errors),
            ),
            budget,
        );
    }

    let response = match operation.as_str() {
        "detect" => op_detect(&parsed),
        "compile" => op_compile(&parsed),
        "validateData" => op_validate_data(&parsed),
        "validateSchema" => op_validate_schema(&parsed),
        "describe" => op_describe(),
        "burn" => op_burn(&parsed),
        other => failure(other, "INVALID_INPUT", format!("unsupported operation: {other}"), None),
    };
    (response, budget)
}

fn op_detect(request: &Value) -> Value {
    let detected = detect::detect(&request["schema"]);
    success("detect", json!({ "detected": detected_json(detected.as_ref()) }))
}

fn op_compile(request: &Value) -> Value {
    let evaluation = engine::compile_and_validate(
        &request["schema"],
        request.get("pointer").and_then(Value::as_str).unwrap_or("#/"),
        None,
        request.get("draft").and_then(Value::as_str),
    );
    success(
        "compile",
        json!({
            "compiled": evaluation.init_error.is_none(),
            "initError": evaluation.init_error,
            "draft": evaluation.draft,
            "detected": detected_json(evaluation.detected.as_ref()),
        }),
    )
}

fn op_validate_data(request: &Value) -> Value {
    let evaluation = engine::compile_and_validate(
        &request["schema"],
        request.get("pointer").and_then(Value::as_str).unwrap_or("#/"),
        Some(&request["instance"]),
        request.get("draft").and_then(Value::as_str),
    );
    if let Some(init_error) = evaluation.init_error {
        return success(
            "validateData",
            json!({
                "valid": false,
                "errors": [init_error],
                "draft": evaluation.draft,
                "detected": detected_json(evaluation.detected.as_ref()),
            }),
        );
    }
    success(
        "validateData",
        json!({
            "valid": evaluation.errors.is_empty(),
            "errors": evaluation.errors,
            "draft": evaluation.draft,
            "detected": detected_json(evaluation.detected.as_ref()),
        }),
    )
}

fn op_validate_schema(request: &Value) -> Value {
    let schema = &request["schema"];
    let detected = detect::detect(schema);
    let requested_type = request.get("schemaType").and_then(Value::as_str);
    let requested_version = text_field(request, "version");
    let family = requested_type.or_else(|| detected.as_ref().map(|item| item.schema_type.as_str()));

    if let Some(message) = version_mismatch(family, detected.as_ref(), requested_type, requested_version, request) {
        return schema_violation(request, &detected, message);
    }

    if matches!(family, Some("openapi" | "asyncapi")) {
        let family = family.unwrap_or("openapi");
        let version = requested_version.or_else(|| detected.as_ref().map(|item| item.version.as_str()));
        let Some(version) = version else {
            return schema_violation(
                request,
                &detected,
                format!("{family} version was not detected"),
            );
        };
        return match specs::validate_document(family, version, schema) {
            Ok((draft, errors)) => success(
                "validateSchema",
                json!({
                    "valid": errors.is_empty(),
                    "errors": errors,
                    "draft": draft,
                    "detected": detected_json(detected.as_ref()),
                }),
            ),
            Err(message) if message.starts_with("unsupported ") => schema_violation(request, &detected, message),
            Err(message) => failure("validateSchema", "INTERNAL", message, None),
        };
    }

    let override_draft = requested_version.or_else(|| text_field(request, "draft"));
    let draft = detect::draft_for(schema, override_draft);
    match engine::schema_document_errors(schema, &draft) {
        Ok(errors) => success(
            "validateSchema",
            json!({
                "valid": errors.is_empty(),
                "errors": errors,
                "draft": draft,
                "detected": detected_json(detected.as_ref()),
            }),
        ),
        Err(message) => failure("validateSchema", "INTERNAL", message, None),
    }
}

fn warm_specification(request: &Value) {
    if request.get("operation").and_then(Value::as_str) != Some("validateSchema") {
        return;
    }
    let Some(schema) = request.get("schema") else {
        return;
    };
    let detected = detect::detect(schema);
    let requested_type = request.get("schemaType").and_then(Value::as_str);
    let requested_version = text_field(request, "version");
    let family = requested_type.or_else(|| detected.as_ref().map(|item| item.schema_type.as_str()));
    if version_mismatch(family, detected.as_ref(), requested_type, requested_version, request).is_some() {
        return;
    }
    if !matches!(family, Some("openapi" | "asyncapi")) {
        return;
    }
    let version = requested_version.or_else(|| detected.as_ref().map(|item| item.version.as_str()));
    if let (Some(family), Some(version)) = (family, version) {
        specs::warm(family, version);
    }
}

fn version_mismatch(
    family: Option<&str>,
    detected: Option<&detect::Detected>,
    requested_type: Option<&str>,
    requested_version: Option<&str>,
    request: &Value,
) -> Option<String> {
    let detected_type = detected.map(|item| item.schema_type.as_str());
    let detected_version = detected.map(|item| item.version.as_str());
    if let (Some(requested_type), Some(detected_type)) = (requested_type, detected_type) {
        if requested_type != detected_type {
            return Some(format!(
                "detected {detected_type} does not match requested type {requested_type}"
            ));
        }
    }
    let family = family?;
    let requested = if family == "jsonschema" {
        requested_version.or_else(|| text_field(request, "draft"))
    } else {
        requested_version
    }?;
    let detected_version = detected_version?;
    let agrees = if family == "jsonschema" {
        requested == detected_version
    } else {
        specs::same_version(family, detected_version, requested)
    };
    if agrees {
        None
    } else {
        Some(format!(
            "detected {family} version {detected_version} does not match requested version {requested}"
        ))
    }
}

fn schema_violation(request: &Value, detected: &Option<detect::Detected>, message: String) -> Value {
    success(
        "validateSchema",
        json!({
            "valid": false,
            "errors": [{
                "type": "SCHEMA_VIOLATION",
                "message": message,
            }],
            "draft": text_field(request, "draft").unwrap_or("draft-07"),
            "detected": detected_json(detected.as_ref()),
        }),
    )
}

fn text_field<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request.get(name).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn op_describe() -> Value {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("tool manifest is JSON");
    success(
        "describe",
        json!({
            "description": {
                "name": manifest["name"],
                "version": manifest["version"],
                "drafts": manifest["drafts"],
                "keywords": KEYWORDS,
                "specifications": specs::specifications(),
                "defaultBudget": {
                    "maxElapsedMicros": DEFAULT_MAX_ELAPSED_MICROS,
                    "maxPeakBytes": DEFAULT_MAX_PEAK_BYTES,
                }
            }
        }),
    )
}

fn op_burn(request: &Value) -> Value {
    let micros = request
        .get("burnMicros")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    burn(micros);
    success("burn", json!({ "description": { "burnedMicros": micros } }))
}

fn success(operation: &str, output: Value) -> Value {
    json!({
        "ok": true,
        "operation": operation,
        "output": output,
    })
}

fn failure(operation: &str, code: &str, message: impl Into<String>, errors: Option<Vec<Value>>) -> Value {
    let mut error = json!({
        "code": code,
        "message": message.into(),
    });
    if let Some(errors) = errors {
        error["errors"] = Value::Array(errors);
    }
    json!({
        "ok": false,
        "operation": operation,
        "error": error,
    })
}

fn detected_json(detected: Option<&detect::Detected>) -> Value {
    match detected {
        Some(detected) => json!({
            "type": detected.schema_type,
            "version": detected.version,
        }),
        None => Value::Null,
    }
}

fn budget_from(request: &Value) -> ResourceBudget {
    let budget = request.get("budget");
    ResourceBudget::merge(
        budget
            .and_then(|value| value.get("maxElapsedMicros"))
            .and_then(Value::as_u64),
        budget
            .and_then(|value| value.get("maxPeakBytes"))
            .and_then(Value::as_u64),
    )
}

fn encode_sample(sample: ResourceSample) -> Value {
    json!({
        "elapsedMicros": sample.elapsed_micros.min(u128::from(u64::MAX)) as u64,
        "peakBytes": sample.peak_bytes,
        "allocatedBytes": sample.allocated_bytes,
    })
}

fn decode_sample(value: &Value) -> ResourceSample {
    ResourceSample {
        elapsed_micros: value.get("elapsedMicros").and_then(Value::as_u64).unwrap_or(0).into(),
        peak_bytes: value.get("peakBytes").and_then(Value::as_u64).unwrap_or(0),
        allocated_bytes: value.get("allocatedBytes").and_then(Value::as_u64).unwrap_or(0),
    }
}

fn resources_json(sample: ResourceSample, budget: ResourceBudget, within: bool) -> Value {
    json!({
        "elapsedMicros": sample.elapsed_micros.min(u128::from(u64::MAX)) as u64,
        "peakBytes": sample.peak_bytes,
        "allocatedBytes": sample.allocated_bytes,
        "withinBudget": within,
        "budget": {
            "maxElapsedMicros": budget.max_elapsed_micros,
            "maxPeakBytes": budget.max_peak_bytes,
        }
    })
}

fn input_validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(INPUT_SCHEMA).expect("input contract is JSON");
        jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&schema)
            .expect("input contract compiles")
    })
}

fn output_validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(OUTPUT_SCHEMA).expect("output contract is JSON");
        jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&schema)
            .expect("output contract compiles")
    })
}

fn validate_contract(validator: &jsonschema::Validator, value: &Value) -> Result<Vec<Value>, String> {
    Ok(validator
        .iter_errors(value)
        .map(|error| errors::map_validation_error(&error))
        .collect())
}

fn output_is_valid(response: &Value) -> bool {
    output_validator().is_valid(response)
}

fn contract_violation(response: &Value, sample: ResourceSample, budget: ResourceBudget) -> Value {
    let errors = validate_contract(output_validator(), response).unwrap_or_default();
    let operation = response
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut violation = failure(
        operation,
        "CONTRACT_VIOLATION",
        "response does not match the output contract",
        Some(errors),
    );
    violation["resources"] = resources_json(sample, budget, sample.within_budget(budget));
    if output_validator().is_valid(&violation) {
        violation
    } else {
        fallback_response("response does not match the output contract")
    }
}

fn fallback_response(message: &str) -> Value {
    json!({
        "ok": false,
        "operation": "unknown",
        "error": {
            "code": "INTERNAL",
            "message": message,
        },
        "resources": {
            "elapsedMicros": 0,
            "peakBytes": 0,
            "allocatedBytes": 0,
            "withinBudget": true,
            "budget": {
                "maxElapsedMicros": DEFAULT_MAX_ELAPSED_MICROS,
                "maxPeakBytes": DEFAULT_MAX_PEAK_BYTES,
            }
        }
    })
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "validation engine panicked".to_owned()
}
