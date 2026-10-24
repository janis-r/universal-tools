"""JSON Schema tool backed by the shared WebAssembly module.

The module is ``targets/wasm/json-schema/ut_json_schema.wasm``. JavaScript and
Go load that same file. This host supplies the ``ut.now_micros`` clock.
"""

from __future__ import annotations

import json
import time
import weakref
from pathlib import Path
from typing import Any, Callable

from wasmtime import FuncType, Linker, Module, Store, ValType

WASM_PATH = Path(__file__).resolve().parents[2] / "wasm" / "json-schema" / "ut_json_schema.wasm"


class SchemaCompilationError(Exception):
    """The schema could not be compiled. ``init_error`` matches ``DataValidator``."""

    def __init__(self, init_error: dict[str, Any]):
        super().__init__(init_error.get("message", "Initialization failed"))
        self.init_error = init_error


class _Bindings:
    def __init__(self) -> None:
        if not WASM_PATH.is_file():
            raise FileNotFoundError(
                f"missing {WASM_PATH}; run scripts/build-wasm.ps1 or sh ./scripts/build-wasm.sh"
            )
        self.store = Store()
        linker = Linker(self.store.engine)
        linker.define_func(
            "ut",
            "now_micros",
            FuncType([], [ValType.i64()]),
            lambda: time.monotonic_ns() // 1000,
        )
        module = Module.from_file(self.store.engine, str(WASM_PATH))
        unexpected = [
            item
            for item in module.imports
            if not (item.module == "ut" and item.name == "now_micros")
        ]
        if unexpected:
            names = ", ".join(f"{item.module}.{item.name}" for item in unexpected)
            raise RuntimeError(f"shared wasm module imports a host-specific symbol: {names}")
        self.instance = linker.instantiate(self.store, module)
        exports = self.instance.exports(self.store)
        self.memory = exports["memory"]
        self.alloc = exports["ut_alloc"]
        self.dealloc = exports["ut_dealloc"]
        self.free_result = exports["ut_free_result"]
        self.execute = exports["ut_execute"]
        self.compile = exports["ut_compile"]
        self.validate = exports["ut_validate"]
        self.spawn = exports["ut_spawn"]
        self.free = exports["ut_free"]

    def write_text(self, text: str) -> tuple[int, int]:
        data = text.encode()
        if not data:
            return 0, 0
        ptr = int(self.alloc(self.store, len(data)))
        if ptr == 0:
            raise RuntimeError("wasm allocation failed")
        self.memory.write(self.store, data, ptr)
        return ptr, len(data)

    def free_text(self, ptr: int, length: int) -> None:
        if length:
            self.dealloc(self.store, ptr, length)

    def read_result(self, ptr: int) -> str:
        if ptr == 0:
            raise RuntimeError("wasm result allocation failed")
        header = self.memory.read(self.store, ptr, ptr + 4)
        length = int.from_bytes(header, "little")
        payload = self.memory.read(self.store, ptr + 4, ptr + 4 + length)
        self.free_result(self.store, ptr)
        return payload.decode()

    def call_execute(self, request: str) -> str:
        ptr, length = self.write_text(request)
        try:
            return self.read_result(int(self.execute(self.store, ptr, length)))
        finally:
            self.free_text(ptr, length)

    def call_compile(self, schema: str, pointer: str, draft: str) -> str:
        written = [self.write_text(schema), self.write_text(pointer), self.write_text(draft)]
        try:
            result = self.compile(
                self.store,
                written[0][0],
                written[0][1],
                written[1][0],
                written[1][1],
                written[2][0],
                written[2][1],
            )
            return self.read_result(int(result))
        finally:
            for ptr, length in reversed(written):
                self.free_text(ptr, length)

    def call_validate(self, handle: int, instance: str) -> str:
        ptr, length = self.write_text(instance)
        try:
            return self.read_result(int(self.validate(self.store, handle, ptr, length)))
        finally:
            self.free_text(ptr, length)

    def call_spawn(self, handle: int, pointer: str) -> str:
        ptr, length = self.write_text(pointer)
        try:
            return self.read_result(int(self.spawn(self.store, handle, ptr, length)))
        finally:
            self.free_text(ptr, length)

    def call_free(self, handle: int) -> None:
        self.free(self.store, handle)


_BINDINGS: _Bindings | None = None


def _wasm() -> _Bindings:
    global _BINDINGS
    if _BINDINGS is None:
        _BINDINGS = _Bindings()
    return _BINDINGS


def _release(handle: int) -> None:
    _wasm().call_free(handle)


class CompiledValidator:
    """A schema compiled once inside the WebAssembly module."""

    def __init__(self, handle: int, draft: str):
        self._handle = handle
        self.draft = draft
        self._finalizer = weakref.finalize(self, _release, handle)

    def validate(self, instance: Any) -> list[dict[str, Any]]:
        """Schema mismatches. An empty list means the instance is valid."""
        return _errors_from(self.evaluate(instance))

    def evaluate(self, instance: Any) -> dict[str, Any]:
        """Full tool response. The compiled schema is not sent again."""
        return json.loads(_wasm().call_validate(self._current(), _encode(instance)))

    def spawn(self, pointer: str) -> CompiledValidator:
        """Compile ``pointer`` against the document stored with this schema."""
        return _finish(_open_raw(_wasm().call_spawn(self._current(), pointer)))

    def free(self) -> None:
        """Drop the compiled schema inside the WebAssembly module."""
        if self._finalizer.alive:
            self._finalizer()
        self._handle = None

    def __enter__(self) -> CompiledValidator:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.free()

    def _current(self) -> int:
        if self._handle is None:
            raise RuntimeError("compiled validator was released")
        return self._handle


def compile_schema(schema: Any, pointer: str = "#/", draft: str | None = None) -> CompiledValidator:
    """Compile ``schema`` once and keep the validator in the WebAssembly module."""
    return _finish(_open(schema, pointer, draft))


def execute_raw(request: str) -> dict[str, Any]:
    """JSON text in, parsed response out. Invalid JSON is reported by the tool."""
    return json.loads(_wasm().call_execute(request))


def execute(request: dict[str, Any]) -> dict[str, Any]:
    """Stateless JSON-in, JSON-out call. ``ok`` is not the validation result."""
    return execute_raw(json.dumps(request))


def detect_content_type(content: Any) -> dict[str, Any] | None:
    response = execute({"operation": "detect", "schema": content})
    if not response.get("ok") or response.get("output") is None:
        raise RuntimeError((response.get("error") or {}).get("message", "detect failed"))
    return response["output"].get("detected")


class SchemaValidator:
    """Check that a document is a valid schema of one family and version."""

    def __init__(self, schema_type: str, version: str):
        self.schema_type = schema_type
        self.version = version

    def validate(self, content: Any) -> list[dict[str, Any]]:
        request: dict[str, Any] = {
            "operation": "validateSchema",
            "schema": content,
            "schemaType": self.schema_type,
            "version": self.version,
        }
        if self.schema_type == "jsonschema":
            request["draft"] = self.version
        response = execute(request)
        output = response.get("output") or {}
        if not response.get("ok") or output.get("errors") is None:
            raise RuntimeError((response.get("error") or {}).get("message", "schema validation failed"))
        return output["errors"]


def validate_schema(content: dict[str, Any]) -> dict[str, Any]:
    """Detect the document family and validate it as a schema."""
    metadata = detect_content_type(content)
    if metadata is None:
        raise RuntimeError("Unknown document type - unable to validate")
    validator = SchemaValidator(metadata["type"], metadata["version"])
    errors = validator.validate(content)
    return {
        "success": len(errors) == 0,
        "type": metadata["type"],
        "version": metadata["version"],
        "errors": errors,
    }


class DataValidator:
    """Compile a data schema once and validate many instances."""

    def __init__(self, schema: Any, pointer: str = "#/"):
        opened = _open(schema, pointer, None)
        if opened["failure_message"]:
            _discard(opened["handle"])
            raise RuntimeError(opened["failure_message"])
        if opened["handle"] is None or opened["init_error"]:
            _discard(opened["handle"])
            self._compiled = None
            self._init_error = opened["init_error"] or {
                "type": "UNKNOWN_ERROR",
                "message": "Initialization failed",
            }
            return
        self._compiled = CompiledValidator(opened["handle"], opened["draft"])
        self._init_error = None

    def validate(self, data: Any) -> list[dict[str, Any]]:
        if self._init_error is not None:
            return [self._init_error]
        if self._compiled is None:
            raise RuntimeError("compiled validator was released")
        return self._compiled.validate(data)

    def spawn_fragment_validator(self, pointer: str) -> Callable[..., Any]:
        if self._init_error is not None:
            raise RuntimeError("Validator initialization, cannot spawn fragment validator")
        if self._compiled is None:
            raise RuntimeError("compiled validator was released")
        try:
            child = self._compiled.spawn(pointer)
        except SchemaCompilationError as error:
            raise RuntimeError(error.init_error["message"]) from error

        def validate(data: Any) -> list[dict[str, Any]]:
            return child.validate(data)

        validate.free = child.free  # type: ignore[attr-defined]
        return validate

    def free(self) -> None:
        if self._compiled is not None:
            self._compiled.free()
            self._compiled = None


def generate_data_validator(schema: Any, pointer: str = "#/") -> Callable[..., Any]:
    """Compile ``schema`` once. The callable is true when the instance matches."""
    validator = DataValidator(schema, pointer)

    def validate(data: Any) -> bool:
        validate.errors = validator.validate(data)
        return len(validate.errors) == 0

    validate.errors = []  # type: ignore[attr-defined]
    validate.free = validator.free  # type: ignore[attr-defined]
    return validate


def _open(schema: Any, pointer: str, draft: str | None) -> dict[str, Any]:
    return _open_raw(_wasm().call_compile(_encode(schema), pointer, draft or ""))


def _open_raw(raw: str) -> dict[str, Any]:
    body = json.loads(raw)
    return {
        "handle": body.get("handle"),
        "init_error": body.get("initError"),
        "failure_message": body.get("failureMessage"),
        "draft": body.get("draft", "draft-07"),
    }


def _finish(opened: dict[str, Any]) -> CompiledValidator:
    if opened["failure_message"]:
        _discard(opened["handle"])
        raise RuntimeError(opened["failure_message"])
    if opened["handle"] is None or opened["init_error"]:
        _discard(opened["handle"])
        init_error = opened["init_error"] or {
            "type": "UNKNOWN_ERROR",
            "message": "Initialization failed",
        }
        raise SchemaCompilationError(init_error)
    return CompiledValidator(opened["handle"], opened["draft"])


def _discard(handle: int | None) -> None:
    if handle is not None:
        _release(handle)


def _errors_from(response: dict[str, Any]) -> list[dict[str, Any]]:
    output = response.get("output") or {}
    if not response.get("ok") or output.get("errors") is None:
        raise RuntimeError((response.get("error") or {}).get("message", "validation failed"))
    return output["errors"]


def _encode(value: Any) -> str:
    encoded = json.dumps(value)
    if not isinstance(encoded, str):
        raise RuntimeError("value is not JSON")
    return encoded
