use serde_json::json;
use ut_json_schema::execute_json;

#[test]
fn validates_a_string_and_reports_resources() {
    let response = execute(json!({
        "operation": "validateData",
        "schema": { "type": "string", "minLength": 2 },
        "instance": "hi"
    }));
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["output"]["valid"], true, "{response}");
    assert_eq!(response["resources"]["withinBudget"], true, "{response}");
    assert!(response["resources"]["peakBytes"].as_u64().unwrap() > 0);
}

#[test]
fn rejects_a_request_outside_the_input_contract() {
    let response = execute(json!({ "operation": "nope" }));
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "INVALID_INPUT");
    assert_eq!(response["resources"]["withinBudget"], true);
}

#[test]
fn memory_budget_trips_when_the_ceiling_is_tiny() {
    let response = execute(json!({
        "operation": "validateData",
        "schema": { "type": "string" },
        "instance": "hi",
        "budget": { "maxElapsedMicros": 5000000, "maxPeakBytes": 1 }
    }));
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error"]["code"], "RESOURCE_LIMIT");
    assert_eq!(response["resources"]["withinBudget"], false);
}

fn execute(request: serde_json::Value) -> serde_json::Value {
    let raw = execute_json(&request.to_string());
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{error}: {raw}"))
}
