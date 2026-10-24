// Package jsonschema validates JSON Schema, OpenAPI, and AsyncAPI documents
// through the shared WebAssembly module at targets/wasm/json-schema.
// JavaScript and Python load that same file. This host supplies ut.now_micros.
package jsonschema

import (
	"context"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"time"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
)

type ToolResponse map[string]any

type ValidationError map[string]any

type SchemaCompilationError struct {
	InitError ValidationError
}

func (e *SchemaCompilationError) Error() string {
	if message, ok := e.InitError["message"].(string); ok && message != "" {
		return message
	}
	return "Initialization failed"
}

type Detected struct {
	Type    string `json:"type"`
	Version string `json:"version"`
}

type CompiledValidator struct {
	handle uint32
	Draft  string
	freed  bool
}

type SchemaValidator struct {
	SchemaType string
	Version    string
}

type DataValidator struct {
	compiled  *CompiledValidator
	initError ValidationError
}

type compileBody struct {
	Draft          string         `json:"draft"`
	Handle         *uint32        `json:"handle"`
	InitError      map[string]any `json:"initError"`
	FailureMessage string         `json:"failureMessage"`
}

type openedSchema struct {
	handle         *uint32
	initError      ValidationError
	failureMessage string
	draft          string
}

var (
	loadOnce sync.Once
	mod      api.Module
	loadErr  error
)

func wasmPath() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "..", "wasm", "json-schema", "ut_json_schema.wasm")
}

func load() error {
	loadOnce.Do(func() {
		ctx := context.Background()
		runtime := wazero.NewRuntime(ctx)
		origin := time.Now()
		_, loadErr = runtime.NewHostModuleBuilder("ut").
			NewFunctionBuilder().
			WithFunc(func() int64 { return time.Since(origin).Microseconds() }).
			Export("now_micros").
			Instantiate(ctx)
		if loadErr != nil {
			return
		}
		bytes, err := os.ReadFile(wasmPath())
		if err != nil {
			loadErr = fmt.Errorf("missing %s; run scripts/build-wasm.ps1 or sh ./scripts/build-wasm.sh: %w", wasmPath(), err)
			return
		}
		mod, loadErr = runtime.Instantiate(ctx, bytes)
	})
	return loadErr
}

func CompileSchema(schema any, pointer string, draft string) (*CompiledValidator, error) {
	if pointer == "" {
		pointer = "#/"
	}
	opened, err := openSchema(schema, pointer, draft)
	if err != nil {
		return nil, err
	}
	return finishCompile(opened)
}

func (v *CompiledValidator) Validate(instance any) ([]ValidationError, error) {
	response, err := v.Evaluate(instance)
	if err != nil {
		return nil, err
	}
	return errorsFrom(response)
}

func (v *CompiledValidator) Evaluate(instance any) (ToolResponse, error) {
	handle, err := v.current()
	if err != nil {
		return nil, err
	}
	encoded, err := json.Marshal(instance)
	if err != nil {
		return nil, err
	}
	raw, err := callValidate(handle, string(encoded))
	if err != nil {
		return nil, err
	}
	return decodeResponse(raw)
}

func (v *CompiledValidator) Spawn(pointer string) (*CompiledValidator, error) {
	handle, err := v.current()
	if err != nil {
		return nil, err
	}
	raw, err := callSpawn(handle, pointer)
	if err != nil {
		return nil, err
	}
	return finishCompile(parseCompile(raw))
}

func (v *CompiledValidator) Free() {
	if v == nil || v.freed {
		return
	}
	v.freed = true
	runtime.SetFinalizer(v, nil)
	_ = callFree(v.handle)
}

func (v *CompiledValidator) current() (uint32, error) {
	if v == nil || v.freed {
		return 0, fmt.Errorf("compiled validator was released")
	}
	return v.handle, nil
}

func ExecuteRaw(request string) (ToolResponse, error) {
	raw, err := callExecute(request)
	if err != nil {
		return nil, err
	}
	return decodeResponse(raw)
}

func Execute(request any) (ToolResponse, error) {
	encoded, err := json.Marshal(request)
	if err != nil {
		return nil, err
	}
	return ExecuteRaw(string(encoded))
}

func DetectContentType(content any) (*Detected, error) {
	response, err := Execute(map[string]any{"operation": "detect", "schema": content})
	if err != nil {
		return nil, err
	}
	if ok, _ := response["ok"].(bool); !ok || response["output"] == nil {
		return nil, fmt.Errorf("%s", errorMessage(response, "detect failed"))
	}
	output, _ := response["output"].(map[string]any)
	detected, _ := output["detected"].(map[string]any)
	if detected == nil {
		return nil, nil
	}
	return &Detected{
		Type:    stringField(detected, "type"),
		Version: stringField(detected, "version"),
	}, nil
}

func (v SchemaValidator) Validate(content any) ([]ValidationError, error) {
	request := map[string]any{
		"operation":  "validateSchema",
		"schema":     content,
		"schemaType": v.SchemaType,
		"version":    v.Version,
	}
	if v.SchemaType == "jsonschema" {
		request["draft"] = v.Version
	}
	response, err := Execute(request)
	if err != nil {
		return nil, err
	}
	return errorsFrom(response)
}

func ValidateSchema(content map[string]any) (map[string]any, error) {
	metadata, err := DetectContentType(content)
	if err != nil {
		return nil, err
	}
	if metadata == nil {
		return nil, fmt.Errorf("Unknown document type - unable to validate")
	}
	errors, err := (SchemaValidator{SchemaType: metadata.Type, Version: metadata.Version}).Validate(content)
	if err != nil {
		return nil, err
	}
	return map[string]any{
		"success": len(errors) == 0,
		"type":    metadata.Type,
		"version": metadata.Version,
		"errors":  errors,
	}, nil
}

func NewDataValidator(schema any, pointer string) (*DataValidator, error) {
	if pointer == "" {
		pointer = "#/"
	}
	opened, err := openSchema(schema, pointer, "")
	if err != nil {
		return nil, err
	}
	validator := &DataValidator{}
	if opened.failureMessage != "" {
		discard(opened.handle)
		return nil, fmt.Errorf("%s", opened.failureMessage)
	}
	if opened.handle == nil || opened.initError != nil {
		discard(opened.handle)
		if opened.initError != nil {
			validator.initError = opened.initError
		} else {
			validator.initError = ValidationError{"type": "UNKNOWN_ERROR", "message": "Initialization failed"}
		}
		return validator, nil
	}
	validator.compiled = newCompiled(*opened.handle, opened.draft)
	return validator, nil
}

func (v *DataValidator) Validate(data any) ([]ValidationError, error) {
	if v.initError != nil {
		return []ValidationError{v.initError}, nil
	}
	if v.compiled == nil {
		return nil, fmt.Errorf("compiled validator was released")
	}
	return v.compiled.Validate(data)
}

type FragmentValidator struct {
	child *CompiledValidator
}

func (f FragmentValidator) Validate(data any) ([]ValidationError, error) {
	return f.child.Validate(data)
}

func (f FragmentValidator) Free() {
	f.child.Free()
}

func (v *DataValidator) SpawnFragmentValidator(pointer string) (FragmentValidator, error) {
	if v.initError != nil {
		return FragmentValidator{}, fmt.Errorf("Validator initialization, cannot spawn fragment validator")
	}
	if v.compiled == nil {
		return FragmentValidator{}, fmt.Errorf("compiled validator was released")
	}
	child, err := v.compiled.Spawn(pointer)
	if err != nil {
		if compileErr, ok := err.(*SchemaCompilationError); ok {
			return FragmentValidator{}, fmt.Errorf("%s", compileErr.Error())
		}
		return FragmentValidator{}, err
	}
	return FragmentValidator{child: child}, nil
}

func (v *DataValidator) Free() {
	if v.compiled != nil {
		v.compiled.Free()
		v.compiled = nil
	}
}

func newCompiled(handle uint32, draft string) *CompiledValidator {
	validator := &CompiledValidator{handle: handle, Draft: draft}
	runtime.SetFinalizer(validator, func(validator *CompiledValidator) {
		validator.Free()
	})
	return validator
}

func openSchema(schema any, pointer string, draft string) (openedSchema, error) {
	encoded, err := json.Marshal(schema)
	if err != nil {
		return openedSchema{}, err
	}
	raw, err := callCompile(string(encoded), pointer, draft)
	if err != nil {
		return openedSchema{}, err
	}
	return parseCompile(raw), nil
}

func parseCompile(raw string) openedSchema {
	var body compileBody
	if err := json.Unmarshal([]byte(raw), &body); err != nil {
		return openedSchema{
			initError: ValidationError{"type": "UNKNOWN_ERROR", "message": err.Error()},
			draft:     "draft-07",
		}
	}
	opened := openedSchema{
		handle:         body.Handle,
		failureMessage: body.FailureMessage,
		draft:          body.Draft,
	}
	if body.InitError != nil {
		opened.initError = body.InitError
	}
	if opened.draft == "" {
		opened.draft = "draft-07"
	}
	return opened
}

func finishCompile(opened openedSchema) (*CompiledValidator, error) {
	if opened.failureMessage != "" {
		discard(opened.handle)
		return nil, fmt.Errorf("%s", opened.failureMessage)
	}
	if opened.handle == nil || opened.initError != nil {
		discard(opened.handle)
		initError := opened.initError
		if initError == nil {
			initError = ValidationError{"type": "UNKNOWN_ERROR", "message": "Initialization failed"}
		}
		return nil, &SchemaCompilationError{InitError: initError}
	}
	return newCompiled(*opened.handle, opened.draft), nil
}

func discard(handle *uint32) {
	if handle != nil {
		_ = callFree(*handle)
	}
}

func errorsFrom(response ToolResponse) ([]ValidationError, error) {
	if ok, _ := response["ok"].(bool); !ok {
		return nil, fmt.Errorf("%s", errorMessage(response, "validation failed"))
	}
	output, _ := response["output"].(map[string]any)
	rawErrors, ok := output["errors"].([]any)
	if !ok {
		return nil, fmt.Errorf("%s", errorMessage(response, "validation failed"))
	}
	errors := make([]ValidationError, 0, len(rawErrors))
	for _, item := range rawErrors {
		if encoded, ok := item.(map[string]any); ok {
			errors = append(errors, encoded)
		}
	}
	return errors, nil
}

func errorMessage(response ToolResponse, fallback string) string {
	if encoded, ok := response["error"].(map[string]any); ok {
		if message, ok := encoded["message"].(string); ok && message != "" {
			return message
		}
	}
	return fallback
}

func stringField(value map[string]any, key string) string {
	text, _ := value[key].(string)
	return text
}

func decodeResponse(raw string) (ToolResponse, error) {
	var response ToolResponse
	if err := json.Unmarshal([]byte(raw), &response); err != nil {
		return nil, err
	}
	return response, nil
}

func callExecute(request string) (string, error) {
	return call1("ut_execute", request)
}

func callValidate(handle uint32, instance string) (string, error) {
	if err := load(); err != nil {
		return "", err
	}
	ptr, length, err := writeText(instance)
	if err != nil {
		return "", err
	}
	defer freeText(ptr, length)
	out, err := mod.ExportedFunction("ut_validate").Call(context.Background(), uint64(handle), uint64(ptr), uint64(length))
	if err != nil {
		return "", err
	}
	return readResult(uint32(out[0]))
}

func callSpawn(handle uint32, pointer string) (string, error) {
	if err := load(); err != nil {
		return "", err
	}
	ptr, length, err := writeText(pointer)
	if err != nil {
		return "", err
	}
	defer freeText(ptr, length)
	out, err := mod.ExportedFunction("ut_spawn").Call(context.Background(), uint64(handle), uint64(ptr), uint64(length))
	if err != nil {
		return "", err
	}
	return readResult(uint32(out[0]))
}

func callCompile(schema string, pointer string, draft string) (string, error) {
	if err := load(); err != nil {
		return "", err
	}
	parts := []string{schema, pointer, draft}
	written := make([][2]uint32, 0, len(parts))
	defer func() {
		for i := len(written) - 1; i >= 0; i-- {
			freeText(written[i][0], written[i][1])
		}
	}()
	for _, part := range parts {
		ptr, length, err := writeText(part)
		if err != nil {
			return "", err
		}
		written = append(written, [2]uint32{ptr, length})
	}
	out, err := mod.ExportedFunction("ut_compile").Call(
		context.Background(),
		uint64(written[0][0]), uint64(written[0][1]),
		uint64(written[1][0]), uint64(written[1][1]),
		uint64(written[2][0]), uint64(written[2][1]),
	)
	if err != nil {
		return "", err
	}
	return readResult(uint32(out[0]))
}

func callFree(handle uint32) error {
	if err := load(); err != nil {
		return err
	}
	_, err := mod.ExportedFunction("ut_free").Call(context.Background(), uint64(handle))
	return err
}

func call1(name string, text string) (string, error) {
	if err := load(); err != nil {
		return "", err
	}
	ptr, length, err := writeText(text)
	if err != nil {
		return "", err
	}
	defer freeText(ptr, length)
	out, err := mod.ExportedFunction(name).Call(context.Background(), uint64(ptr), uint64(length))
	if err != nil {
		return "", err
	}
	return readResult(uint32(out[0]))
}

func writeText(text string) (uint32, uint32, error) {
	if text == "" {
		return 0, 0, nil
	}
	data := []byte(text)
	out, err := mod.ExportedFunction("ut_alloc").Call(context.Background(), uint64(len(data)))
	if err != nil {
		return 0, 0, err
	}
	ptr := uint32(out[0])
	if ptr == 0 {
		return 0, 0, fmt.Errorf("wasm allocation failed")
	}
	if !mod.Memory().Write(ptr, data) {
		return 0, 0, fmt.Errorf("wasm memory write failed")
	}
	return ptr, uint32(len(data)), nil
}

func freeText(ptr uint32, length uint32) {
	if length == 0 {
		return
	}
	_, _ = mod.ExportedFunction("ut_dealloc").Call(context.Background(), uint64(ptr), uint64(length))
}

func readResult(ptr uint32) (string, error) {
	if ptr == 0 {
		return "", fmt.Errorf("wasm result allocation failed")
	}
	header, ok := mod.Memory().Read(ptr, 4)
	if !ok {
		return "", fmt.Errorf("wasm result header is out of range")
	}
	length := binary.LittleEndian.Uint32(header)
	payload, ok := mod.Memory().Read(ptr+4, length)
	if !ok {
		return "", fmt.Errorf("wasm result payload is out of range")
	}
	text := string(payload)
	_, _ = mod.ExportedFunction("ut_free_result").Call(context.Background(), uint64(ptr))
	return text, nil
}
