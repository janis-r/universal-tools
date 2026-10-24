package jsonschema

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
)

func TestSharedFixtures(t *testing.T) {
	_, file, _, _ := runtime.Caller(0)
	dir := filepath.Join(filepath.Dir(file), "..", "..", "..", "fixtures", "json-schema")
	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	count := 0
	var failures []string
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(dir, entry.Name()))
		if err != nil {
			t.Fatal(err)
		}
		var cases []struct {
			Name       string          `json:"name"`
			Request    json.RawMessage `json:"request"`
			RequestRaw string          `json:"requestRaw"`
			Expect     map[string]any  `json:"expect"`
		}
		if err := json.Unmarshal(raw, &cases); err != nil {
			t.Fatal(err)
		}
		for _, item := range cases {
			count++
			var response ToolResponse
			if item.RequestRaw != "" {
				response, err = ExecuteRaw(item.RequestRaw)
			} else {
				response, err = ExecuteRaw(string(item.Request))
			}
			if err != nil {
				failures = append(failures, item.Name+": "+err.Error())
				continue
			}
			if message := check(item.Name, item.Expect, response); message != "" {
				encoded, _ := json.Marshal(response)
				failures = append(failures, message+"\nresponse: "+string(encoded))
			}
		}
	}
	if count == 0 {
		t.Fatal("no fixtures")
	}
	if len(failures) > 0 {
		t.Fatal(strings.Join(failures, "\n"))
	}
}

func check(name string, expect map[string]any, response ToolResponse) string {
	if value, ok := expect["ok"]; ok && response["ok"] != value {
		return name + ": ok mismatch"
	}
	errorObject, _ := response["error"].(map[string]any)
	if value, ok := expect["errorCode"]; ok && errorObject["code"] != value {
		return name + ": error code mismatch"
	}
	output, _ := response["output"].(map[string]any)
	if output == nil {
		output = map[string]any{}
	}
	if value, ok := expect["valid"]; ok && output["valid"] != value {
		return name + ": valid mismatch"
	}
	if value, ok := expect["compiled"]; ok && output["compiled"] != value {
		return name + ": compiled mismatch"
	}
	resources, _ := response["resources"].(map[string]any)
	if value, ok := expect["withinBudget"]; ok && resources["withinBudget"] != value {
		return name + ": withinBudget mismatch"
	}
	if value, ok := expect["detected"]; ok && !jsonEqual(output["detected"], value) {
		return name + ": detected mismatch"
	}
	if value, ok := expect["draft"]; ok && output["draft"] != value {
		return name + ": draft mismatch"
	}
	messages := allMessages(errorObject, output)
	if value, ok := expect["messageContains"].(string); ok {
		found := false
		for _, message := range messages {
			if strings.Contains(message, value) {
				found = true
				break
			}
		}
		if !found {
			return name + ": no message contains " + value
		}
	}
	types := errorTypes(output)
	if value, ok := numberField(expect, "minErrors"); ok && len(types) < value {
		return name + ": too few errors"
	}
	if value, ok := numberField(expect, "maxErrors"); ok && len(types) > value {
		return name + ": too many errors"
	}
	if raw, ok := expect["errorTypes"].([]any); ok {
		expected := make([]string, len(raw))
		for i, item := range raw {
			expected[i], _ = item.(string)
		}
		if exact, _ := expect["exactTypes"].(bool); exact {
			left := append([]string(nil), expected...)
			right := append([]string(nil), types...)
			slices.Sort(left)
			slices.Sort(right)
			if !slices.Equal(left, right) {
				return name + ": error types mismatch"
			}
		} else {
			for _, kind := range expected {
				if !slices.Contains(types, kind) {
					return name + ": missing error type " + kind
				}
			}
		}
	}
	return ""
}

func allMessages(errorObject map[string]any, output map[string]any) []string {
	var messages []string
	if message, ok := errorObject["message"].(string); ok {
		messages = append(messages, message)
	}
	for _, item := range errorTypesRaw(output) {
		if message, ok := item["message"].(string); ok {
			messages = append(messages, message)
		}
	}
	if initError, ok := output["initError"].(map[string]any); ok {
		if message, ok := initError["message"].(string); ok {
			messages = append(messages, message)
		}
	}
	return messages
}

func errorTypes(output map[string]any) []string {
	items := errorTypesRaw(output)
	types := make([]string, 0, len(items))
	for _, item := range items {
		if kind, ok := item["type"].(string); ok {
			types = append(types, kind)
		}
	}
	return types
}

func errorTypesRaw(output map[string]any) []map[string]any {
	raw, _ := output["errors"].([]any)
	items := make([]map[string]any, 0, len(raw))
	for _, item := range raw {
		if encoded, ok := item.(map[string]any); ok {
			items = append(items, encoded)
		}
	}
	return items
}

func numberField(expect map[string]any, key string) (int, bool) {
	value, ok := expect[key].(float64)
	if !ok {
		return 0, false
	}
	return int(value), true
}

func jsonEqual(left any, right any) bool {
	a, _ := json.Marshal(left)
	b, _ := json.Marshal(right)
	var decodedLeft any
	var decodedRight any
	_ = json.Unmarshal(a, &decodedLeft)
	_ = json.Unmarshal(b, &decodedRight)
	a, _ = json.Marshal(decodedLeft)
	b, _ = json.Marshal(decodedRight)
	return string(a) == string(b)
}
