//! Shared black-box cases. The same JSON files are executed by the JS target.

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use ut_json_schema::execute_json;

#[test]
fn shared_cases_match_the_contract() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/json-schema");
    let mut files: Vec<_> = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("missing fixtures at {}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "expected fixture files in {}", directory.display());

    let mut failures = Vec::new();
    let mut count = 0;
    for file in files {
        let cases: Vec<Value> = serde_json::from_str(&fs::read_to_string(&file).unwrap())
            .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for case in cases {
            count += 1;
            let name = case["name"].as_str().unwrap_or("<unnamed>").to_owned();
            let response = if let Some(raw) = case.get("requestRaw").and_then(Value::as_str) {
                let text = execute_json(raw);
                serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
            } else {
                execute(&case["request"])
            };
            if let Err(message) = check(&name, &case["expect"], &response) {
                failures.push(format!("{name}\n{message}\nresponse: {response}"));
            }
        }
    }
    assert!(count > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn described_budget_matches_the_manifest() {
    let response = execute(&json!({ "operation": "describe" }));
    assert_eq!(response["ok"], true, "{response}");
    let manifest: Value = serde_json::from_str(include_str!("../../../contracts/json-schema/manifest.json")).unwrap();
    assert_eq!(response["output"]["description"]["name"], manifest["name"]);
    assert_eq!(response["output"]["description"]["specifications"]["openapi"][0], "3.0");
    assert_eq!(
        response["output"]["description"]["defaultBudget"]["maxElapsedMicros"],
        manifest["defaultBudget"]["maxElapsedMicros"]
    );
    assert_eq!(
        response["output"]["description"]["defaultBudget"]["maxPeakBytes"],
        manifest["defaultBudget"]["maxPeakBytes"]
    );
    assert!(response["resources"]["withinBudget"].as_bool().unwrap());
    assert!(response["resources"]["peakBytes"].as_u64().unwrap() > 0);
}

fn execute(request: &Value) -> Value {
    let raw = execute_json(&request.to_string());
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{error}: {raw}"))
}

fn check(name: &str, expect: &Value, response: &Value) -> Result<(), String> {
    if let Some(ok) = expect.get("ok").and_then(Value::as_bool) {
        if response.get("ok").and_then(Value::as_bool) != Some(ok) {
            return Err(format!("{name}: ok is not {ok}"));
        }
    }
    if let Some(code) = expect.get("errorCode").and_then(Value::as_str) {
        let actual = response["error"]["code"].as_str().unwrap_or("");
        if actual != code {
            return Err(format!("{name}: error code {actual} != {code}"));
        }
    }
    if let Some(valid) = expect.get("valid").and_then(Value::as_bool) {
        if response["output"]["valid"].as_bool() != Some(valid) {
            return Err(format!("{name}: valid is not {valid}"));
        }
    }
    if let Some(compiled) = expect.get("compiled").and_then(Value::as_bool) {
        if response["output"]["compiled"].as_bool() != Some(compiled) {
            return Err(format!("{name}: compiled is not {compiled}"));
        }
    }
    if let Some(within) = expect.get("withinBudget").and_then(Value::as_bool) {
        if response["resources"]["withinBudget"].as_bool() != Some(within) {
            return Err(format!("{name}: withinBudget is not {within}"));
        }
    }
    if expect.get("detected").is_some() {
        if &response["output"]["detected"] != &expect["detected"] {
            return Err(format!(
                "{name}: detected {} != {}",
                response["output"]["detected"], expect["detected"]
            ));
        }
    }
    if let Some(draft) = expect.get("draft").and_then(Value::as_str) {
        if response["output"]["draft"].as_str() != Some(draft) {
            return Err(format!("{name}: draft is not {draft}"));
        }
    }
    let messages = messages(response);
    if let Some(needle) = expect.get("messageContains").and_then(Value::as_str) {
        if !messages.iter().any(|message| message.contains(needle)) {
            return Err(format!("{name}: no message contains {needle:?}; got {messages:?}"));
        }
    }
    let types = error_types(response);
    if let Some(min) = expect.get("minErrors").and_then(Value::as_u64) {
        if (types.len() as u64) < min {
            return Err(format!("{name}: expected at least {min} errors, got {}", types.len()));
        }
    }
    if let Some(max) = expect.get("maxErrors").and_then(Value::as_u64) {
        if (types.len() as u64) > max {
            return Err(format!("{name}: expected at most {max} errors, got {}", types.len()));
        }
    }
    if let Some(expected_types) = expect.get("errorTypes").and_then(Value::as_array) {
        let mut expected: Vec<_> = expected_types
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        let mut actual = types.clone();
        if expect.get("exactTypes").and_then(Value::as_bool).unwrap_or(false) {
            expected.sort();
            actual.sort();
            if expected != actual {
                return Err(format!("{name}: error types {actual:?} != {expected:?}"));
            }
        } else {
            for kind in expected {
                if !actual.iter().any(|actual| actual == &kind) {
                    return Err(format!("{name}: missing error type {kind}; got {actual:?}"));
                }
            }
        }
    }
    Ok(())
}

fn error_types(response: &Value) -> Vec<String> {
    response["output"]["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| error.get("type").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn messages(response: &Value) -> Vec<String> {
    let mut found = Vec::new();
    if let Some(message) = response["error"]["message"].as_str() {
        found.push(message.to_owned());
    }
    if let Some(errors) = response["output"]["errors"].as_array() {
        for error in errors {
            if let Some(message) = error.get("message").and_then(Value::as_str) {
                found.push(message.to_owned());
            }
        }
    }
    if let Some(message) = response["output"]["initError"]["message"].as_str() {
        found.push(message.to_owned());
    }
    found
}
