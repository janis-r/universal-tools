import unittest

import json_schema


class ApiTest(unittest.TestCase):
    def test_detect_content_type(self):
        self.assertEqual(
            json_schema.detect_content_type({"$schema": "https://json-schema.org/draft/2020-12/schema"}),
            {"type": "jsonschema", "version": "2020-12"},
        )
        self.assertEqual(
            json_schema.detect_content_type({"openapi": "3.1.0"}),
            {"type": "openapi", "version": "3.1.0"},
        )
        self.assertIsNone(json_schema.detect_content_type({"foo": "bar"}))
        self.assertIsNone(json_schema.detect_content_type(None))

    def test_generate_data_validator(self):
        validate = json_schema.generate_data_validator({
            "type": "object",
            "properties": {"name": {"type": "string"}, "email": {"type": "string", "format": "email"}},
            "required": ["name", "email"],
        })
        self.assertTrue(validate({"name": "John", "email": "john@example.com"}))
        self.assertEqual(validate.errors, [])
        self.assertFalse(validate({"name": "John"}))
        self.assertEqual(validate.errors[0]["type"], "MISSING_REQUIRED_FIELD")
        self.assertFalse(validate({"name": 1, "email": "john@example.com"}))
        self.assertEqual(validate.errors[0]["type"], "INVALID_TYPE")
        validate.free()

    def test_fragment_pointer_and_spawn(self):
        schema = {
            "definitions": {
                "Address": {
                    "type": "object",
                    "properties": {"zipCode": {"type": "string", "pattern": r"^\d{5}$"}},
                    "required": ["zipCode"],
                },
            },
            "components": {
                "schemas": {
                    "User": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "address": {"$ref": "#/definitions/Address"},
                        },
                        "required": ["name", "address"],
                    },
                },
            },
        }
        validator = json_schema.DataValidator(schema, "#/components/schemas/User")
        self.assertEqual(validator.validate({
            "name": "John",
            "address": {"zipCode": "12345"},
        }), [])
        address = validator.spawn_fragment_validator("#/definitions/Address")
        self.assertEqual(len(address({"zipCode": "abc"})), 1)
        self.assertEqual(address({"zipCode": "abc"})[0]["type"], "PATTERN_MISMATCH")
        address.free()
        validator.free()

    def test_spawn_refuses_invalid_schema_and_missing_pointer(self):
        broken = json_schema.DataValidator({"type": "invalid-type"})
        self.assertEqual(len(broken.validate({})), 1)
        self.assertEqual(broken.validate({})[0]["type"], "UNKNOWN_ERROR")
        with self.assertRaisesRegex(RuntimeError, "Validator initialization, cannot spawn fragment validator"):
            broken.spawn_fragment_validator("#/")

        validator = json_schema.DataValidator({
            "type": "object",
            "properties": {"name": {"type": "string"}},
        })
        with self.assertRaisesRegex(RuntimeError, 'property "nonexistent" not found'):
            validator.spawn_fragment_validator("#/nonexistent/path")
        validator.free()

    def test_schema_validator_draft_07(self):
        validator = json_schema.SchemaValidator("jsonschema", "draft-07")
        self.assertEqual(validator.validate({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        }), [])
        invalid = validator.validate({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "invalid-type",
        })
        self.assertGreater(len(invalid), 0)

    def test_schema_validator_openapi(self):
        validator = json_schema.SchemaValidator("openapi", "3.0")
        self.assertEqual(validator.validate({
            "openapi": "3.0.3",
            "info": {"title": "Test API", "version": "1.0.0"},
            "paths": {},
        }), [])
        missing = validator.validate({"openapi": "3.0.0", "paths": {}})
        self.assertEqual(missing[0]["type"], "MISSING_REQUIRED_FIELD")
        mismatch = validator.validate({
            "openapi": "3.1.0",
            "info": {"title": "Test API", "version": "1.0.0"},
            "paths": {},
        })
        self.assertEqual(mismatch[0]["type"], "SCHEMA_VIOLATION")
        self.assertIn("does not match requested version 3.0", mismatch[0]["message"])

    def test_validate_schema_detection(self):
        with self.assertRaisesRegex(RuntimeError, "Unknown document type"):
            json_schema.validate_schema({"title": "no markers"})
        result = json_schema.validate_schema({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "string",
        })
        self.assertTrue(result["success"])
        self.assertEqual(result["type"], "jsonschema")
        self.assertEqual(result["version"], "draft-07")

    def test_compile_schema_reuses_the_validator(self):
        compiled = json_schema.compile_schema({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        })
        accepted = compiled.evaluate({"name": "Ada"})
        self.assertTrue(accepted["ok"])
        self.assertEqual(accepted["operation"], "validateData")
        self.assertTrue(accepted["output"]["valid"])
        self.assertEqual(accepted["output"]["errors"], [])
        self.assertTrue(accepted["resources"]["withinBudget"])
        self.assertGreater(accepted["resources"]["peakBytes"], 0)
        self.assertNotIn("error", accepted)

        rejected = compiled.evaluate({})
        self.assertTrue(rejected["ok"])
        self.assertFalse(rejected["output"]["valid"])
        self.assertEqual(rejected["output"]["errors"][0]["type"], "MISSING_REQUIRED_FIELD")

        name = compiled.spawn("#/properties/name")
        self.assertEqual(name.validate("Ada"), [])
        self.assertEqual(name.validate(1)[0]["type"], "INVALID_TYPE")
        name.free()
        with self.assertRaisesRegex(RuntimeError, "released"):
            name.validate("Ada")

        compiled.free()
        compiled.free()
        with self.assertRaisesRegex(RuntimeError, "released"):
            compiled.validate({"name": "Ada"})

    def test_compile_schema_throws_initialization_error(self):
        with self.assertRaises(json_schema.SchemaCompilationError):
            json_schema.compile_schema({"type": "invalid-type"})

    def test_resource_sample(self):
        response = json_schema.execute({
            "operation": "validateData",
            "schema": {"type": "string", "minLength": 1},
            "instance": "ok",
        })
        self.assertTrue(response["ok"])
        self.assertTrue(response["resources"]["withinBudget"])
        self.assertGreater(response["resources"]["peakBytes"], 0)
        self.assertLess(
            response["resources"]["elapsedMicros"],
            response["resources"]["budget"]["maxElapsedMicros"],
        )
        self.assertLessEqual(
            response["resources"]["peakBytes"],
            response["resources"]["budget"]["maxPeakBytes"],
        )


if __name__ == "__main__":
    unittest.main()
