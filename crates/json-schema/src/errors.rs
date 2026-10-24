//! Map engine diagnostics onto the stable error vocabulary from the contract.

use serde_json::{json, Value};

pub fn map_validation_error(error: &jsonschema::ValidationError<'_>) -> Value {
    let schema_path = error.schema_path().to_string();
    let instance_path = error.instance_path().to_string();
    let keyword = keyword_from_schema_path(&schema_path);
    let mut mapped = json!({
        "type": error_type(&keyword),
        "message": error.to_string(),
        "details": {
            "keyword": keyword,
            "schemaPath": schema_path,
        }
    });
    if !instance_path.is_empty() {
        mapped["path"] = Value::String(instance_path);
    }
    mapped
}

pub fn unknown_error(message: impl Into<String>) -> Value {
    json!({
        "type": "UNKNOWN_ERROR",
        "message": message.into(),
    })
}

pub fn init_error(message: impl AsRef<str>) -> Value {
    unknown_error(format!("Initialization failed: {}", message.as_ref()))
}

fn error_type(keyword: &str) -> &'static str {
    match keyword {
        "required" | "dependentRequired" | "dependencies" => "MISSING_REQUIRED_FIELD",
        "type" => "INVALID_TYPE",
        "format" => "INVALID_FORMAT",
        "pattern" => "PATTERN_MISMATCH",
        "minLength" | "maxLength" | "minItems" | "maxItems" | "minimum" | "maximum"
        | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" | "minProperties"
        | "maxProperties" => "CONSTRAINT_VIOLATION",
        "enum" | "const" => "ENUM_MISMATCH",
        "$ref" | "ref" => "INVALID_REFERENCE",
        _ => "SCHEMA_VIOLATION",
    }
}

fn keyword_from_schema_path(path: &str) -> String {
    for segment in path.rsplit('/') {
        if segment.is_empty() {
            continue;
        }
        let decoded = segment.replace("~1", "/").replace("~0", "~");
        if decoded.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        return decoded;
    }
    "unknown".to_owned()
}
