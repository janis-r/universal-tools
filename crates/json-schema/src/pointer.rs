//! JSON Pointer walk that follows internal Reference Objects.
//!
//! This is the fragment behavior of the reference data validator: a pointer
//! such as `#/paths/~1users/get/parameters/0/schema` steps through an OpenAPI
//! `$ref` and keeps walking, instead of treating that `$ref` as the schema.

use serde_json::Value;

pub fn extract_fragment(schema: &Value, pointer: &str) -> Result<Value, String> {
    if pointer == "#/" {
        return Ok(schema.clone());
    }
    if !pointer.starts_with("#/") {
        return Err(format!("Invalid JSON Pointer: {pointer}"));
    }

    let segments: Vec<&str> = pointer
        .trim_start_matches("#/")
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    let mut visiting = Vec::new();
    let mut current = schema;

    for segment in &segments {
        current = follow_internal_refs(schema, current, pointer, &mut visiting)?;
        let unescaped = unescape(segment);
        current = step(current, pointer, &unescaped)?;
    }

    // The pointer can stop on a Reference Object. Follow that `$ref`, and any
    // further `$ref` it points at, until the node is the concrete schema.
    current = follow_internal_refs(schema, current, pointer, &mut visiting)?;

    if !current.is_object() {
        return Err(format!(
            "Invalid JSON Pointer: {pointer} - target is not an object"
        ));
    }
    Ok(current.clone())
}

fn step<'a>(current: &'a Value, pointer: &str, segment: &str) -> Result<&'a Value, String> {
    if current.is_null() {
        return Err(format!(
            "Invalid JSON Pointer: {pointer} - cannot navigate past null/undefined"
        ));
    }
    match current {
        Value::Object(map) => map.get(segment).ok_or_else(|| {
            format!("Invalid JSON Pointer: {pointer} - property \"{segment}\" not found")
        }),
        Value::Array(items) => {
            let index = segment.parse::<usize>().map_err(|_| {
                format!("Invalid JSON Pointer: {pointer} - property \"{segment}\" not found")
            })?;
            items.get(index).ok_or_else(|| {
                format!("Invalid JSON Pointer: {pointer} - property \"{segment}\" not found")
            })
        }
        other => Err(format!(
            "Invalid JSON Pointer: {pointer} - cannot navigate into {}",
            json_type_name(other)
        )),
    }
}

fn follow_internal_refs<'a>(
    root: &'a Value,
    mut current: &'a Value,
    context: &str,
    visiting: &mut Vec<String>,
) -> Result<&'a Value, String> {
    while let Some(reference) = internal_ref(current) {
        if !reference.starts_with("#/") {
            return Err(format!(
                "Invalid JSON Pointer: {context} - external $ref is not supported: {reference}"
            ));
        }
        if visiting.iter().any(|seen| seen == &reference) {
            return Err(format!(
                "Invalid JSON Pointer: {context} - circular $ref at {reference}"
            ));
        }
        visiting.push(reference.clone());
        current = navigate(root, &reference).map_err(|detail| {
            format!("Invalid JSON Pointer: {context} - $ref target {reference} not found ({detail})")
        })?;
    }
    Ok(current)
}

fn internal_ref(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let reference = object.get("$ref")?.as_str()?;
    Some(reference.to_owned())
}

fn navigate<'a>(root: &'a Value, pointer: &str) -> Result<&'a Value, String> {
    if pointer == "#/" {
        return Ok(root);
    }
    if !pointer.starts_with("#/") {
        return Err(format!("Invalid JSON Pointer: {pointer}"));
    }
    let mut current = root;
    for segment in pointer
        .trim_start_matches("#/")
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        let unescaped = unescape(segment);
        current = step(current, pointer, &unescaped)?;
    }
    Ok(current)
}

fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "object",
        Value::Object(_) => "object",
    }
}

pub const ROOT_URI: &str = "https://universal-tools.local/root.json";

/// Point document-absolute references at the registered root resource so a
/// fragment compiled on its own can still see `$defs` and `definitions`.
pub fn rewrite_document_refs(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                rewrite_document_refs(item);
            }
        }
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                    if let Some(reference) = child.as_str() {
                        if reference == "#" || reference.starts_with("#/") {
                            *child = Value::String(format!("{ROOT_URI}{reference}"));
                        }
                    }
                } else {
                    rewrite_document_refs(child);
                }
            }
        }
        _ => {}
    }
}
