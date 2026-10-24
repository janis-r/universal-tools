import json
import unittest
from pathlib import Path

import json_schema

FIXTURES = Path(__file__).resolve().parents[3] / "fixtures" / "json-schema"


def check(name, expect, response):
    if "ok" in expect and response.get("ok") is not expect["ok"]:
        return f"{name}: ok is not {expect['ok']}"
    error = response.get("error") or {}
    if "errorCode" in expect and error.get("code") != expect["errorCode"]:
        return f"{name}: error code {error.get('code')} != {expect['errorCode']}"
    output = response.get("output") or {}
    if "valid" in expect and output.get("valid") is not expect["valid"]:
        return f"{name}: valid is not {expect['valid']}"
    if "compiled" in expect and output.get("compiled") is not expect["compiled"]:
        return f"{name}: compiled is not {expect['compiled']}"
    resources = response.get("resources") or {}
    if "withinBudget" in expect and resources.get("withinBudget") is not expect["withinBudget"]:
        return f"{name}: withinBudget is not {expect['withinBudget']}"
    if "detected" in expect and json.dumps(output.get("detected"), sort_keys=True) != json.dumps(expect["detected"], sort_keys=True):
        return f"{name}: detected mismatch"
    if "draft" in expect and output.get("draft") != expect["draft"]:
        return f"{name}: draft is not {expect['draft']}"
    messages = []
    if error.get("message"):
        messages.append(error["message"])
    for item in output.get("errors") or []:
        if item.get("message"):
            messages.append(item["message"])
    init_error = output.get("initError") or {}
    if init_error.get("message"):
        messages.append(init_error["message"])
    if expect.get("messageContains") and not any(expect["messageContains"] in message for message in messages):
        return f"{name}: no message contains {expect['messageContains']}"
    types = [item.get("type") for item in output.get("errors") or []]
    if "minErrors" in expect and len(types) < expect["minErrors"]:
        return f"{name}: expected at least {expect['minErrors']} errors, got {len(types)}"
    if "maxErrors" in expect and len(types) > expect["maxErrors"]:
        return f"{name}: expected at most {expect['maxErrors']} errors, got {len(types)}"
    if "errorTypes" in expect:
        if expect.get("exactTypes"):
            if sorted(expect["errorTypes"]) != sorted(types):
                return f"{name}: error types {types} != {expect['errorTypes']}"
        elif any(kind not in types for kind in expect["errorTypes"]):
            return f"{name}: missing error type from {expect['errorTypes']}; got {types}"
    return None


class FixtureTest(unittest.TestCase):
    def test_shared_cases(self):
        files = sorted(FIXTURES.glob("*.json"))
        self.assertGreater(len(files), 0)
        failures = []
        count = 0
        for path in files:
            cases = json.loads(path.read_text(encoding="utf-8"))
            for item in cases:
                count += 1
                if "requestRaw" in item:
                    response = json_schema.execute_raw(item["requestRaw"])
                else:
                    response = json_schema.execute(item["request"])
                message = check(item["name"], item["expect"], response)
                if message:
                    failures.append(f"{message}\nresponse: {json.dumps(response)}")
        self.assertGreater(count, 0)
        self.assertEqual(failures, [])


if __name__ == "__main__":
    unittest.main()
