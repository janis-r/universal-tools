import assert from "node:assert/strict";
import { test } from "node:test";
import {
  compileSchema,
  DataValidator,
  detectContentType,
  execute,
  generateDataValidator,
  SchemaCompilationError,
  SchemaValidator,
  validateSchema,
} from "../src/index.ts";

test("detectContentType matches the shared rules", () => {
  assert.deepEqual(
    detectContentType({ $schema: "https://json-schema.org/draft/2020-12/schema" }),
    { type: "jsonschema", version: "2020-12" },
  );
  assert.deepEqual(detectContentType({ openapi: "3.1.0" }), { type: "openapi", version: "3.1.0" });
  assert.equal(detectContentType({ foo: "bar" }), null);
  assert.equal(detectContentType(null), null);
});

test("generateDataValidator stores errors on the function", () => {
  const validate = generateDataValidator({
    type: "object",
    properties: { name: { type: "string" }, email: { type: "string", format: "email" } },
    required: ["name", "email"],
  });

  assert.equal(validate({ name: "John", email: "john@example.com" }), true);
  assert.deepEqual(validate.errors, []);

  assert.equal(validate({ name: "John" }), false);
  assert.equal(validate.errors[0]?.type, "MISSING_REQUIRED_FIELD");
  assert.equal(validate({ name: 1, email: "john@example.com" }), false);
  assert.equal(validate.errors[0]?.type, "INVALID_TYPE");
});

test("DataValidator follows a fragment pointer and spawns another", () => {
  const schema = {
    definitions: {
      Address: {
        type: "object",
        properties: { zipCode: { type: "string", pattern: "^\\d{5}$" } },
        required: ["zipCode"],
      },
    },
    components: {
      schemas: {
        User: {
          type: "object",
          properties: { name: { type: "string" }, address: { $ref: "#/definitions/Address" } },
          required: ["name", "address"],
        },
      },
    },
  };
  const validator = new DataValidator(schema, "#/components/schemas/User");
  assert.deepEqual(validator.validate({
    name: "John",
    address: { zipCode: "12345" },
  }), []);

  const address = validator.spawnFragmentValidator("#/definitions/Address");
  assert.equal(address({ zipCode: "abc" }).length, 1);
  assert.equal(address({ zipCode: "abc" })[0]?.type, "PATTERN_MISMATCH");
});

test("spawn refuses an invalid schema and a missing pointer", () => {
  const broken = new DataValidator({ type: "invalid-type" });
  assert.equal(broken.validate({}).length, 1);
  assert.equal(broken.validate({})[0]?.type, "UNKNOWN_ERROR");
  assert.throws(
    () => broken.spawnFragmentValidator("#/"),
    /Validator initialization, cannot spawn fragment validator/,
  );

  const validator = new DataValidator({ type: "object", properties: { name: { type: "string" } } });
  assert.throws(
    () => validator.spawnFragmentValidator("#/nonexistent/path"),
    /property "nonexistent" not found/,
  );
});

test("SchemaValidator checks a draft-07 document", () => {
  const validator = new SchemaValidator("jsonschema", "draft-07");
  const valid = validator.validate({
    $schema: "http://json-schema.org/draft-07/schema#",
    type: "object",
    properties: { name: { type: "string" } },
    required: ["name"],
  });
  assert.deepEqual(valid, []);

  const invalid = validator.validate({
    $schema: "http://json-schema.org/draft-07/schema#",
    type: "invalid-type",
  });
  assert.ok(invalid.length > 0);
});

test("SchemaValidator checks an OpenAPI document against the bundled schema", () => {
  const validator = new SchemaValidator("openapi", "3.0");
  assert.deepEqual(validator.validate({
    openapi: "3.0.3",
    info: { title: "Test API", version: "1.0.0" },
    paths: {},
  }), []);

  const missing = validator.validate({ openapi: "3.0.0", paths: {} });
  assert.equal(missing[0]?.type, "MISSING_REQUIRED_FIELD");

  const mismatch = validator.validate({
    openapi: "3.1.0",
    info: { title: "Test API", version: "1.0.0" },
    paths: {},
  });
  assert.equal(mismatch[0]?.type, "SCHEMA_VIOLATION");
  assert.match(mismatch[0]?.message ?? "", /does not match requested version 3.0/);
});

test("validateSchema throws when the document family cannot be detected", () => {
  assert.throws(() => validateSchema({ title: "no markers" }), /Unknown document type/);
  const result = validateSchema({
    $schema: "http://json-schema.org/draft-07/schema#",
    type: "string",
  });
  assert.equal(result.success, true);
  assert.equal(result.type, "jsonschema");
  assert.equal(result.version, "draft-07");
});

test("compileSchema keeps the validator and sends later instances only", () => {
  const compiled = compileSchema({
    type: "object",
    properties: { name: { type: "string" } },
    required: ["name"],
  });
  const accepted = compiled.evaluate({ name: "Ada" });
  assert.equal(accepted.ok, true);
  assert.equal(accepted.operation, "validateData");
  assert.equal(accepted.output?.valid, true);
  assert.deepEqual(accepted.output?.errors, []);
  assert.equal(accepted.resources.withinBudget, true);
  assert.ok(accepted.resources.peakBytes > 0);
  assert.equal(accepted.error, undefined);

  const rejected = compiled.evaluate({});
  assert.equal(rejected.ok, true);
  assert.equal(rejected.output?.valid, false);
  assert.equal(rejected.output?.errors?.[0]?.type, "MISSING_REQUIRED_FIELD");

  const name = compiled.spawn("#/properties/name");
  assert.deepEqual(name.validate("Ada"), []);
  assert.equal(name.validate(1)[0]?.type, "INVALID_TYPE");
  name.free();
  assert.throws(() => name.validate("Ada"), /released/);

  compiled.free();
  compiled.free();
  assert.throws(() => compiled.validate({ name: "Ada" }), /released/);
});

test("compileSchema throws a structured initialization error", () => {
  assert.throws(() => compileSchema({ type: "invalid-type" }), SchemaCompilationError);
});

test("every call reports a resource sample inside the default budget", () => {
  const response = execute({
    operation: "validateData",
    schema: { type: "string", minLength: 1 },
    instance: "ok",
  });
  assert.equal(response.ok, true);
  assert.equal(response.resources.withinBudget, true);
  assert.ok(response.resources.peakBytes > 0);
  assert.ok(response.resources.elapsedMicros < response.resources.budget.maxElapsedMicros);
  assert.ok(response.resources.peakBytes <= response.resources.budget.maxPeakBytes);
});
