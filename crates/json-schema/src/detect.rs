//! Document sniffing. The rules match the reference `detectContentType`
//! helper: OpenAPI and AsyncAPI win over `$schema`, and JSON Schema versions
//! are read from `json-schema.org` URIs.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub schema_type: String,
    pub version: String,
}

pub fn detect(content: &Value) -> Option<Detected> {
    let object = content.as_object()?;

    if let Some(version) = object.get("openapi").and_then(Value::as_str) {
        return Some(Detected {
            schema_type: "openapi".to_owned(),
            version: version.to_owned(),
        });
    }

    if let Some(version) = object.get("asyncapi").and_then(Value::as_str) {
        return Some(Detected {
            schema_type: "asyncapi".to_owned(),
            version: version.to_owned(),
        });
    }

    let schema_uri = object.get("$schema").and_then(Value::as_str)?;
    if !schema_uri.contains("json-schema.org") {
        return None;
    }

    if let Some(version) = capture_after(schema_uri, "draft-") {
        if version.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
            let digits: String = version.chars().take_while(|ch| ch.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return Some(Detected {
                    schema_type: "jsonschema".to_owned(),
                    version: format!("draft-{digits}"),
                });
            }
        }
    }

    if let Some(rest) = capture_after(schema_uri, "draft/") {
        let version: String = rest.chars().take(7).collect();
        if is_year_month(&version) {
            return Some(Detected {
                schema_type: "jsonschema".to_owned(),
                version,
            });
        }
    }

    None
}

fn capture_after<'a>(haystack: &'a str, marker: &str) -> Option<&'a str> {
    let index = haystack.find(marker)?;
    Some(&haystack[index + marker.len()..])
}

fn is_year_month(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

/// Draft used to compile a document when the caller does not override it.
pub fn draft_for(content: &Value, override_draft: Option<&str>) -> String {
    if let Some(draft) = override_draft {
        return draft.to_owned();
    }
    match detect(content) {
        Some(detected) if detected.schema_type == "jsonschema" => detected.version,
        Some(detected) if detected.schema_type == "openapi" && detected.version.starts_with("3.1") => {
            "2020-12".to_owned()
        }
        _ => "draft-07".to_owned(),
    }
}

pub fn skips_json_schema_meta(content: &Value) -> bool {
    matches!(
        detect(content).as_ref().map(|detected| detected.schema_type.as_str()),
        Some("openapi" | "asyncapi")
    )
}
