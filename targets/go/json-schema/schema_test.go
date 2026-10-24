package jsonschema

import (
	"strings"
	"testing"
)

func TestDetectContentType(t *testing.T) {
	detected, err := DetectContentType(map[string]any{"$schema": "https://json-schema.org/draft/2020-12/schema"})
	if err != nil {
		t.Fatal(err)
	}
	if detected.Type != "jsonschema" || detected.Version != "2020-12" {
		t.Fatalf("detected %+v", detected)
	}
	detected, err = DetectContentType(map[string]any{"openapi": "3.1.0"})
	if err != nil {
		t.Fatal(err)
	}
	if detected.Type != "openapi" || detected.Version != "3.1.0" {
		t.Fatalf("detected %+v", detected)
	}
	detected, err = DetectContentType(map[string]any{"foo": "bar"})
	if err != nil || detected != nil {
		t.Fatalf("detected %+v err %v", detected, err)
	}
	detected, err = DetectContentType(nil)
	if err != nil || detected != nil {
		t.Fatalf("detected %+v err %v", detected, err)
	}
}

func TestDataValidatorFragment(t *testing.T) {
	schema := map[string]any{
		"definitions": map[string]any{
			"Address": map[string]any{
				"type": "object",
				"properties": map[string]any{
					"zipCode": map[string]any{"type": "string", "pattern": `^\d{5}$`},
				},
				"required": []any{"zipCode"},
			},
		},
		"components": map[string]any{
			"schemas": map[string]any{
				"User": map[string]any{
					"type": "object",
					"properties": map[string]any{
						"name":    map[string]any{"type": "string"},
						"address": map[string]any{"$ref": "#/definitions/Address"},
					},
					"required": []any{"name", "address"},
				},
			},
		},
	}
	validator, err := NewDataValidator(schema, "#/components/schemas/User")
	if err != nil {
		t.Fatal(err)
	}
	defer validator.Free()
	errors, err := validator.Validate(map[string]any{
		"name":    "John",
		"address": map[string]any{"zipCode": "12345"},
	})
	if err != nil || len(errors) != 0 {
		t.Fatalf("errors %v %v", errors, err)
	}
	address, err := validator.SpawnFragmentValidator("#/definitions/Address")
	if err != nil {
		t.Fatal(err)
	}
	defer address.Free()
	errors, err = address.Validate(map[string]any{"zipCode": "abc"})
	if err != nil || len(errors) != 1 || errors[0]["type"] != "PATTERN_MISMATCH" {
		t.Fatalf("errors %v %v", errors, err)
	}
}

func TestSpawnRefusesInvalidSchema(t *testing.T) {
	broken, err := NewDataValidator(map[string]any{"type": "invalid-type"}, "")
	if err != nil {
		t.Fatal(err)
	}
	errors, err := broken.Validate(map[string]any{})
	if err != nil || len(errors) != 1 || errors[0]["type"] != "UNKNOWN_ERROR" {
		t.Fatalf("errors %v %v", errors, err)
	}
	_, err = broken.SpawnFragmentValidator("#/")
	if err == nil || !strings.Contains(err.Error(), "Validator initialization, cannot spawn fragment validator") {
		t.Fatal(err)
	}

	validator, err := NewDataValidator(map[string]any{
		"type":       "object",
		"properties": map[string]any{"name": map[string]any{"type": "string"}},
	}, "")
	if err != nil {
		t.Fatal(err)
	}
	defer validator.Free()
	_, err = validator.SpawnFragmentValidator("#/nonexistent/path")
	if err == nil || !strings.Contains(err.Error(), `property "nonexistent" not found`) {
		t.Fatal(err)
	}
}

func TestSchemaValidatorOpenAPI(t *testing.T) {
	validator := SchemaValidator{SchemaType: "openapi", Version: "3.0"}
	errors, err := validator.Validate(map[string]any{
		"openapi": "3.0.3",
		"info":    map[string]any{"title": "Test API", "version": "1.0.0"},
		"paths":   map[string]any{},
	})
	if err != nil || len(errors) != 0 {
		t.Fatalf("errors %v %v", errors, err)
	}
	errors, err = validator.Validate(map[string]any{"openapi": "3.0.0", "paths": map[string]any{}})
	if err != nil || errors[0]["type"] != "MISSING_REQUIRED_FIELD" {
		t.Fatalf("errors %v %v", errors, err)
	}
	errors, err = validator.Validate(map[string]any{
		"openapi": "3.1.0",
		"info":    map[string]any{"title": "Test API", "version": "1.0.0"},
		"paths":   map[string]any{},
	})
	if err != nil || errors[0]["type"] != "SCHEMA_VIOLATION" {
		t.Fatalf("errors %v %v", errors, err)
	}
	if !strings.Contains(errors[0]["message"].(string), "does not match requested version 3.0") {
		t.Fatal(errors[0]["message"])
	}
}

func TestCompileSchemaReusesTheValidator(t *testing.T) {
	compiled, err := CompileSchema(map[string]any{
		"type":       "object",
		"properties": map[string]any{"name": map[string]any{"type": "string"}},
		"required":   []any{"name"},
	}, "", "")
	if err != nil {
		t.Fatal(err)
	}
	accepted, err := compiled.Evaluate(map[string]any{"name": "Ada"})
	if err != nil {
		t.Fatal(err)
	}
	if accepted["ok"] != true || accepted["operation"] != "validateData" {
		t.Fatalf("%v", accepted)
	}
	output := accepted["output"].(map[string]any)
	if output["valid"] != true || len(output["errors"].([]any)) != 0 {
		t.Fatalf("%v", output)
	}
	resources := accepted["resources"].(map[string]any)
	if resources["withinBudget"] != true || resources["peakBytes"].(float64) <= 0 {
		t.Fatalf("%v", resources)
	}

	rejected, err := compiled.Evaluate(map[string]any{})
	if err != nil {
		t.Fatal(err)
	}
	output = rejected["output"].(map[string]any)
	errors := output["errors"].([]any)
	if output["valid"] != false || errors[0].(map[string]any)["type"] != "MISSING_REQUIRED_FIELD" {
		t.Fatalf("%v", output)
	}

	name, err := compiled.Spawn("#/properties/name")
	if err != nil {
		t.Fatal(err)
	}
	mismatches, err := name.Validate("Ada")
	if err != nil || len(mismatches) != 0 {
		t.Fatalf("%v %v", mismatches, err)
	}
	mismatches, err = name.Validate(1)
	if err != nil || mismatches[0]["type"] != "INVALID_TYPE" {
		t.Fatalf("%v %v", mismatches, err)
	}
	name.Free()
	if _, err = name.Validate("Ada"); err == nil || !strings.Contains(err.Error(), "released") {
		t.Fatal(err)
	}
	compiled.Free()
	compiled.Free()
	if _, err = compiled.Validate(map[string]any{"name": "Ada"}); err == nil || !strings.Contains(err.Error(), "released") {
		t.Fatal(err)
	}
}

func TestCompileSchemaInitializationError(t *testing.T) {
	_, err := CompileSchema(map[string]any{"type": "invalid-type"}, "", "")
	if _, ok := err.(*SchemaCompilationError); !ok {
		t.Fatalf("%T %v", err, err)
	}
}
