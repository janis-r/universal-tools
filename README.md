# universal-tools

Small tools with one calling shape. A tool is a JSON request, a JSON response, and a resource budget. Those three are the interface. The Rust implementation can be replaced without changing them.

`json-schema` 0.1.0 is the first tool. It validates JSON Schema, OpenAPI, and AsyncAPI. Rust compiles it to one WebAssembly module. JavaScript, Python, and Go load that file and run the same cases in `fixtures/json-schema/`.

## Layout

```
crates/ut-contract/          resource meter shared by every tool
crates/json-schema/          this tool
contracts/json-schema/       input schema, output schema, manifest
fixtures/json-schema/        cases every host must match
targets/wasm/json-schema/    ut_json_schema.wasm
targets/js/json-schema/      JavaScript wrapper
targets/python/json-schema/  Python wrapper
targets/go/json-schema/      Go wrapper
```

A further tool copies that shape:

```
crates/<name>/
contracts/<name>/input.schema.json
contracts/<name>/output.schema.json
contracts/<name>/manifest.json
fixtures/<name>/*.json
targets/wasm/<name>/
targets/js/<name>/
targets/python/<name>/
targets/go/<name>/
```

Measurement stays in `crates/ut-contract`. A host is accepted when its run of `fixtures/<name>/` matches the `expect` object on every case.

## json-schema

| | |
| --- | --- |
| Name | `json-schema` |
| Version | `0.1.0` |
| Module | `targets/wasm/json-schema/ut_json_schema.wasm` |
| Request | `contracts/json-schema/input.schema.json` |
| Response | `contracts/json-schema/output.schema.json` |
| Budget | `contracts/json-schema/manifest.json` |
| Cases | `fixtures/json-schema/*.json` |

Each case is `{ "name", "request" or "requestRaw", "expect" }`. `expect` may set `ok`, `errorCode`, `valid`, `compiled`, `withinBudget`, `detected`, `draft`, `messageContains`, `minErrors`, `maxErrors`, `errorTypes`, and `exactTypes`.

### Which call

| Situation | Call |
| --- | --- |
| One instance | `execute` with `operation: "validateData"` |
| Many instances, one schema | compile once, then send the instance only |
| A fragment of a schema already compiled | spawn with a JSON Pointer |
| The document is itself a schema | `validateSchema` |
| The family is unknown | `detect`, then `validateSchema` |
| Name, drafts, bundled specifications, budget | `describe` |
| Prove the time budget | `burn` |

`validateData` checks an instance against a schema, or against the fragment at `pointer`. `compile` prepares a schema and reports initialization errors. `validateSchema` checks OpenAPI 3.0 and 3.1, AsyncAPI 2.0.0, 2.1.0, 2.2.0, 2.3.0, 2.4.0, 2.5.0, 2.6.0, and 3.0.0, and JSON Schema drafts draft-04, draft-06, draft-07, 2019-09, and 2020-12. The official schemas live in `crates/json-schema/specs/`, and each version is compiled once. `detect` reads `openapi`, `asyncapi`, or `$schema`.

### Rules a caller can rely on

- `ok` means the call stayed inside the contract and the budget. `output.valid` means the instance matched. A mismatch is `ok: true` and `output.valid: false`.
- `execute` sends the schema on every call and compiles it again. A compiled validator stays in the module. Later calls send the instance only.
- A pointer is followed through internal `#/` references. When the pointer stops on a `$ref`, that chain is followed until the node is a schema, including an OpenAPI parameter that points at another parameter that points at a schema.
- A document with no `$schema` uses draft-07.
- Formats such as `email`, `date-time`, and `uuid` are checked. An unknown format is ignored.
- `version` is compared with the version read from the document. OpenAPI compares `major.minor`, so `3.0.3` agrees with `3.0`. AsyncAPI compares the full version string. JSON Schema compares the draft string (`version`, or `draft` when `version` is absent). A disagreement is `SCHEMA_VIOLATION`, and the other specification is not applied. An unsupported OpenAPI or AsyncAPI version is also `SCHEMA_VIOLATION`; the message starts with `unsupported`.
- Every response includes `resources`. The default ceiling is 5000000 microseconds and 134217728 bytes. A `budget` on the request replaces it. Past the ceiling, `ok` is false and `error.code` is `RESOURCE_LIMIT`. `burnMicros` is capped at 2000000.
- The request is checked against `input.schema.json` and the response against `output.schema.json` before it is returned.

## Request

`additionalProperties` is false. Every call requires `operation`.

| `operation` | Also required |
| --- | --- |
| `validateData` | `schema`, `instance` |
| `compile` | `schema` |
| `validateSchema` | `schema` |
| `detect` | `schema` |
| `describe` | nothing else |
| `burn` | `burnMicros` |

Optional fields: `pointer` (the fragment; the root is `#/`), `draft` (`draft-04`, `draft-06`, `draft-07`, `2019-09`, `2020-12`), `schemaType` (`jsonschema`, `openapi`, `asyncapi`), `version`, `budget.maxElapsedMicros`, `budget.maxPeakBytes`.

```json
{
  "operation": "validateData",
  "schema": { "type": "string", "minLength": 1 },
  "instance": "ok",
  "pointer": "#/",
  "budget": { "maxElapsedMicros": 5000000, "maxPeakBytes": 134217728 }
}
```

```json
{
  "operation": "validateSchema",
  "schemaType": "openapi",
  "version": "3.0",
  "schema": {
    "openapi": "3.0.3",
    "info": { "title": "Test API", "version": "1.0.0" },
    "paths": {}
  }
}
```

## Module

Hosts instantiate `targets/wasm/json-schema/ut_json_schema.wasm`. `scripts/build-wasm.ps1` and `scripts/build-wasm.sh` write that file. It is not committed.

The module imports one function. The host supplies a monotonic clock:

| Import module | Name | Signature |
| --- | --- | --- |
| `ut` | `now_micros` | `() -> i64` microseconds |

| Export | Role |
| --- | --- |
| `memory` | Linear memory for the byte ranges below |
| `ut_alloc`, `ut_dealloc` | Input bytes the caller owns |
| `ut_execute` | JSON request in, JSON response out |
| `ut_compile` | Schema, pointer, and draft bytes in; JSON result out |
| `ut_validate` | Handle and instance JSON in; tool response out |
| `ut_spawn` | Handle and pointer in; same JSON shape as compile |
| `ut_free` | Drop a handle. A second call does nothing |
| `ut_free_result` | Drop a returned buffer |

A string argument is a pointer and a byte length in `memory`. A returned buffer starts with a little-endian `u32` length, then UTF-8. Read those bytes, then call `ut_free_result`.

`ut_compile` and `ut_spawn` return one JSON object:

| Result | Fields |
| --- | --- |
| Compiled | `draft`, `handle` |
| Schema rejected | `draft`, `initError` |
| Compilation crossed the budget | `draft`, `failureMessage` |

`handle` is a nonzero integer. `ut_validate` returns the tool response documented under Response shapes.

## Hosts

JavaScript, Python, and Go load that module. The table is the same surface. The sections after it are the worked calls.

| Need | JavaScript | Python | Go |
| --- | --- | --- | --- |
| Compile and keep | `compileSchema` | `compile_schema` | `CompileSchema` |
| Mismatches only | `validate` | `validate` | `Validate` |
| Full response | `evaluate` | `evaluate` | `Evaluate` |
| Fragment of the stored document | `spawn` | `spawn` | `Spawn` |
| Initialization failure returned as data | `DataValidator` | `DataValidator` | `NewDataValidator` |
| Schema document | `SchemaValidator`, `validateSchema` | `SchemaValidator`, `validate_schema` | `SchemaValidator`, `ValidateSchema` |
| Any operation, stateless | `execute` | `execute` | `Execute` |
| Document family | `detectContentType` | `detect_content_type` | `DetectContentType` |
| Release the compiled schema | `free` | `free` | `Free` |

## JavaScript

Import from `targets/js/json-schema/src/index.ts`. The package has no npm dependencies. It loads `targets/wasm/json-schema/ut_json_schema.wasm`, so build that module before importing.

Compile a schema once when it will check more than one instance. The compiled validator stays in the WebAssembly module. Later calls send only the instance JSON.

```typescript
import { compileSchema, generateDataValidator } from "./targets/js/json-schema/src/index.ts";

const compiled = compileSchema({
  type: "object",
  properties: { name: { type: "string" }, email: { type: "string", format: "email" } },
  required: ["name", "email"],
});

const mismatches = compiled.validate({ name: "Ada" });
// [{ type: "MISSING_REQUIRED_FIELD", message: "...", path: "", details: { keyword: "required", schemaPath: "..." } }]

const response = compiled.evaluate({ name: "Ada", email: "ada@example.com" });
// response.ok === true
// response.output.valid === true
// response.output.errors is []
// response.resources.withinBudget === true

const email = compiled.spawn("#/properties/email");
email.validate("not-an-email");
email.free();
compiled.free();
```

`compileSchema(schema, pointer?, draft?)` throws `SchemaCompilationError` when the schema cannot be compiled. `error.initError` is the initialization error below. `pointer` defaults to `#/`. `draft` overrides the draft taken from `$schema`.

`generateDataValidator` and `DataValidator` use the same compiled validator. An invalid schema does not throw from the constructor: `validate` returns the initialization error, and `spawnFragmentValidator` throws `Validator initialization, cannot spawn fragment validator`.

```typescript
const validateUser = generateDataValidator<{ name: string }>({
  type: "object",
  properties: { name: { type: "string" } },
  required: ["name"],
});

if (!validateUser({ name: "Ada" })) {
  console.error(validateUser.errors);
}
validateUser.free();
```

`validateUser(data)` is a type guard. The latest mismatches are on `validateUser.errors`. Call `free()` when finished. `using` also works on `compileSchema` and `DataValidator` results, and garbage collection releases a validator that was not freed.

`SchemaValidator` and `validateSchema` check that a document is a valid JSON Schema, OpenAPI, or AsyncAPI document. The document is sent on each call because it is the instance. The specification schema for that version is compiled once inside the module and reused. Pass the version you expect as the `SchemaValidator` constructor argument; it is checked against the version detected on the document.

```typescript
const openapi = new SchemaValidator("openapi", "3.0");
openapi.validate({
  openapi: "3.0.3",
  info: { title: "Test API", version: "1.0.0" },
  paths: {},
});
```

`validateSchema` detects the family itself. It returns `{ success, type, version, errors }` and throws `Unknown document type - unable to validate` when the family cannot be detected.

`execute` is the stateless call. It sends the whole request, including the schema, and compiles it again every time. Use it for a single check. `detectContentType` uses it.

`compiled.validate`, `DataValidator.validate`, and `SchemaValidator.validate` return the `errors` array from the response object below. `compiled.evaluate` and `execute` return that whole object. Those methods throw `Error` with `error.message` when the call itself fails. They do not throw when an instance simply fails the schema.

## Python

Import `json_schema` from `targets/python/json-schema/json_schema.py`. Install `wasmtime` with `pip install -r targets/python/json-schema/requirements.txt`. The module loads `targets/wasm/json-schema/ut_json_schema.wasm`, so build that module before importing. Add `targets/python/json-schema` to `PYTHONPATH`, or run Python with that directory as the working directory.

Compile a schema once when it will check more than one instance. The compiled validator stays in the WebAssembly module. Later calls send only the instance JSON.

```python
from json_schema import compile_schema, generate_data_validator

compiled = compile_schema({
    "type": "object",
    "properties": {"name": {"type": "string"}, "email": {"type": "string", "format": "email"}},
    "required": ["name", "email"],
})

mismatches = compiled.validate({"name": "Ada"})
# [{"type": "MISSING_REQUIRED_FIELD", "message": "...", "path": "", "details": {"keyword": "required", "schemaPath": "..."}}]

response = compiled.evaluate({"name": "Ada", "email": "ada@example.com"})
# response["ok"] is True
# response["output"]["valid"] is True
# response["output"]["errors"] is []
# response["resources"]["withinBudget"] is True

email = compiled.spawn("#/properties/email")
email.validate("not-an-email")
email.free()
compiled.free()
```

`compile_schema(schema, pointer="#/", draft=None)` raises `SchemaCompilationError` when the schema cannot be compiled. `error.init_error` is the initialization error below. `pointer` defaults to `#/`. `draft` overrides the draft taken from `$schema`.

`generate_data_validator` and `DataValidator` use the same compiled validator. An invalid schema does not raise from the constructor: `validate` returns the initialization error, and `spawn_fragment_validator` raises `Validator initialization, cannot spawn fragment validator`.

```python
validate_user = generate_data_validator({
    "type": "object",
    "properties": {"name": {"type": "string"}},
    "required": ["name"],
})

if not validate_user({"name": "Ada"}):
    print(validate_user.errors)
validate_user.free()
```

`validate_user(data)` is true when the instance matches. The latest mismatches are on `validate_user.errors`. Call `free()` when finished. A `with compile_schema(...)` block frees the validator on exit, and garbage collection releases a validator that was not freed.

`SchemaValidator` and `validate_schema` check that a document is a valid JSON Schema, OpenAPI, or AsyncAPI document. The document is sent on each call because it is the instance. The specification schema for that version is compiled once inside the module and reused. Pass the version you expect to `SchemaValidator`; it is checked against the version detected on the document.

```python
from json_schema import SchemaValidator

openapi = SchemaValidator("openapi", "3.0")
openapi.validate({
    "openapi": "3.0.3",
    "info": {"title": "Test API", "version": "1.0.0"},
    "paths": {},
})
```

`validate_schema` detects the family itself. It returns `{"success", "type", "version", "errors"}` and raises `Unknown document type - unable to validate` when the family cannot be detected.

`execute` is the stateless call. It sends the whole request, including the schema, and compiles it again every time. Use it for a single check. `detect_content_type` uses it. `execute_raw` sends JSON text through unchanged.

`validate` returns the `errors` list from the response object below. `evaluate` and `execute` return that whole object. Those calls raise `RuntimeError` with `error.message` when the call itself fails. They do not raise when an instance simply fails the schema.

## Go

Import `universal.tools/jsonschema`. The module lives in `targets/go/json-schema` and depends on `wazero`. It loads `targets/wasm/json-schema/ut_json_schema.wasm`, so build that module before running. `go.mod` selects toolchain 1.25.5.

Compile a schema once when it will check more than one instance. The compiled validator stays in the WebAssembly module. Later calls send only the instance JSON.

```go
compiled, err := jsonschema.CompileSchema(map[string]any{
    "type": "object",
    "properties": map[string]any{
        "name":  map[string]any{"type": "string"},
        "email": map[string]any{"type": "string", "format": "email"},
    },
    "required": []any{"name", "email"},
}, "#/", "")
if err != nil {
    return err
}
defer compiled.Free()

mismatches, err := compiled.Validate(map[string]any{"name": "Ada"})
// mismatches[0]["type"] == "MISSING_REQUIRED_FIELD"

response, err := compiled.Evaluate(map[string]any{"name": "Ada", "email": "ada@example.com"})
// response["ok"] == true
// response["output"].(map[string]any)["valid"] == true
// response["resources"].(map[string]any)["withinBudget"] == true

email, err := compiled.Spawn("#/properties/email")
if err != nil {
    return err
}
defer email.Free()
_, err = email.Validate("not-an-email")
```

`CompileSchema(schema, pointer, draft)` returns `*SchemaCompilationError` when the schema cannot be compiled. `InitError` is the initialization error below. `pointer` `#/` is the document root, and an empty pointer means the same thing. An empty `draft` keeps the draft taken from `$schema`.

`NewDataValidator` uses the same compiled validator. An invalid schema does not return an error from `NewDataValidator`: `Validate` returns the initialization error, and `SpawnFragmentValidator` returns `Validator initialization, cannot spawn fragment validator`.

```go
validator, err := jsonschema.NewDataValidator(map[string]any{
    "type":       "object",
    "properties": map[string]any{"name": map[string]any{"type": "string"}},
    "required":   []any{"name"},
}, "#/")
if err != nil {
    return err
}
defer validator.Free()

errors, err := validator.Validate(map[string]any{"name": "Ada"})
name, err := validator.SpawnFragmentValidator("#/properties/name")
defer name.Free()
```

Call `Free` when finished. Garbage collection also releases a validator that was not freed.

`SchemaValidator` and `ValidateSchema` check that a document is a valid JSON Schema, OpenAPI, or AsyncAPI document. The document is sent on each call because it is the instance. The specification schema for that version is compiled once inside the module and reused. Set `Version` to the version you expect; it is checked against the version detected on the document.

```go
openapi := jsonschema.SchemaValidator{SchemaType: "openapi", Version: "3.0"}
errors, err := openapi.Validate(map[string]any{
    "openapi": "3.0.3",
    "info":    map[string]any{"title": "Test API", "version": "1.0.0"},
    "paths":   map[string]any{},
})
```

`ValidateSchema` detects the family itself. It returns a map with `success`, `type`, `version`, and `errors`, and returns `Unknown document type - unable to validate` when the family cannot be detected.

`Execute` is the stateless call. It sends the whole request, including the schema, and compiles it again every time. Use it for a single check. `DetectContentType` uses it. `ExecuteRaw` sends JSON text through unchanged.

`Validate` returns the `errors` slice from the response object below. `Evaluate` and `Execute` return that whole object. Those calls return an `error` whose text is `error.message` when the call itself fails. A schema mismatch returns a nil error and the mismatch list.

## Response shapes

`evaluate` and `execute` return one object. `validate` returns `output.errors` from that object.

```json
{
  "ok": true,
  "operation": "validateData",
  "output": {
    "valid": false,
    "errors": [
      {
        "type": "INVALID_TYPE",
        "message": "\"Ada\" is not of type \"number\"",
        "path": "/age",
        "details": { "keyword": "type", "schemaPath": "/properties/age/type" }
      }
    ],
    "draft": "draft-07",
    "detected": null
  },
  "resources": {
    "elapsedMicros": 120,
    "peakBytes": 4096,
    "allocatedBytes": 8192,
    "withinBudget": true,
    "budget": { "maxElapsedMicros": 5000000, "maxPeakBytes": 134217728 }
  }
}
```

`ok: true` means the call stayed inside the contract and the resource budget. The validation result is `output.valid`. An instance that does not match is still `ok: true`, with `output.valid: false` and one or more `output.errors`. Each host's `validate` returns that `errors` array. An empty array means valid.

`path` is a JSON Pointer into the instance. It is omitted when the error has no instance location. `details.keyword` is the schema keyword that failed.

Error `type` is one of `MISSING_REQUIRED_FIELD`, `INVALID_TYPE`, `INVALID_FORMAT`, `CONSTRAINT_VIOLATION`, `PATTERN_MISMATCH`, `ENUM_MISMATCH`, `INVALID_REFERENCE`, `SCHEMA_VIOLATION`, or `UNKNOWN_ERROR`. A schema that cannot be compiled uses `UNKNOWN_ERROR`, and `message` starts with `Initialization failed:`.

A failed call omits a successful validation result and sets `error`:

```json
{
  "ok": false,
  "operation": "validateData",
  "error": {
    "code": "RESOURCE_LIMIT",
    "message": "execution used 40000us and 128 peak bytes; budget is 10000us and 134217728 bytes"
  },
  "resources": { "withinBudget": false }
}
```

`error.code` is `INVALID_INPUT`, `RESOURCE_LIMIT`, `CONTRACT_VIOLATION`, or `INTERNAL`. For that failure, JavaScript throws `Error`, Python raises `RuntimeError`, and Go returns an `error`. The text is `error.message`. A schema mismatch stays in `output.errors`.

## Build and test

Run these from the repository root, in this order. Host tests load `targets/wasm/json-schema/ut_json_schema.wasm` from the last build. They do not rebuild it.

1. Rust tests. These call the crate in-process and do not load the module.
2. Build the module. A fresh clone has no wasm file until this step.
3. JavaScript, Python, and Go. Each runs `fixtures/json-schema/` and the wrapper calls.

`scripts/test.ps1` and `scripts/test.sh` run step 1 and step 3. They skip step 2.

Requirements: Rust 1.85 or newer, and the `wasm32-unknown-unknown` target from `rust-toolchain.toml`. JavaScript tests need Node.js 22 or newer and have no npm dependencies. Python tests need Python 3.11 or newer and `pip install -r targets/python/json-schema/requirements.txt` (`wasmtime`). Go tests need Go 1.22 or newer. `targets/go/json-schema/go.mod` selects toolchain 1.25.5, so the first `go test` downloads that toolchain when the installed one is older, and downloads `wazero`. Go 1.25.1 on Windows ARM64 fails while compiling its own standard library.

```sh
rustup target add wasm32-unknown-unknown
```

WSL is a separate Linux environment. Build the module in the environment that will run the host tests. The build script needs Cargo and the wasm target. Install Node, Python, or Go in that environment for the host you want to run.

Windows needs a linker. Visual Studio Build Tools supplies `link.exe` for the MSVC toolchain. An ARM64 Windows machine without MSVC uses `aarch64-pc-windows-gnullvm` and [llvm-mingw](https://github.com/mstorsjo/llvm-mingw). On that machine, `cargo test` with no toolchain override stops with `linker link.exe not found`. That message is the missing linker. The PowerShell scripts select the GNU LLVM toolchain when it is installed and prepend llvm-mingw to `PATH`. Linux, macOS, and other Unix systems use the host linker through `cargo`.

### Test the Rust crates

This compiles a normal binary for the host and calls the Rust function in-process. It does not build or load WebAssembly.

Windows, when the ARM64 GNU LLVM toolchain is installed:

```powershell
$env:PATH = "$env:USERPROFILE\.local\llvm-mingw\llvm-mingw-20261020-ucrt-aarch64\bin;$env:PATH"
cargo +stable-aarch64-pc-windows-gnullvm test --workspace
```

Linux, macOS, and other Unix systems:

```sh
cargo test --workspace
```

That runs the meter tests in `crates/ut-contract`, `crates/json-schema/tests/smoke.rs`, and `crates/json-schema/tests/fixtures.rs`. The fixture test reads every file in `fixtures/json-schema/` and checks the JSON response.

### Build the WebAssembly module

Windows:

```powershell
.\scripts\build-wasm.ps1
```

Linux, macOS, and other Unix systems:

```sh
sh ./scripts/build-wasm.sh
```

The script compiles `ut-json-schema` for `wasm32-unknown-unknown` and copies `ut_json_schema.wasm` to `targets/wasm/json-schema/`. That file is generated and is not committed, so a fresh clone needs this script once before any host test.

### Test that module through each host

JavaScript:

```sh
cd targets/js/json-schema
npm test
```

Python, from the repository root:

```sh
python -m unittest discover -s targets/python/json-schema -v
```

Go:

```sh
cd targets/go/json-schema
go test ./...
```

On Windows the same commands work in PowerShell. Each host loads `targets/wasm/json-schema/ut_json_schema.wasm` and runs `fixtures/json-schema/` plus the wrapper API. The JavaScript package has no npm dependencies.

To run the Rust tests and every host together:

```powershell
.\scripts\test.ps1
```

```sh
sh ./scripts/test.sh
```
