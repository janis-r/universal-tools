import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * The module JavaScript, Python, and Go all instantiate.
 * Build it with scripts/build-wasm.ps1 or scripts/build-wasm.sh.
 */
const wasmPath = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../../wasm/json-schema/ut_json_schema.wasm",
);

interface Exports {
  memory: WebAssembly.Memory;
  ut_alloc: (len: number) => number;
  ut_dealloc: (ptr: number, len: number) => void;
  ut_free_result: (ptr: number) => void;
  ut_execute: (ptr: number, len: number) => number;
  ut_compile: (
    schemaPtr: number,
    schemaLen: number,
    pointerPtr: number,
    pointerLen: number,
    draftPtr: number,
    draftLen: number,
  ) => number;
  ut_validate: (handle: number, ptr: number, len: number) => number;
  ut_spawn: (handle: number, ptr: number, len: number) => number;
  ut_free: (handle: number) => void;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

const wasm = load();

function load(): Exports {
  if (!existsSync(wasmPath)) {
    throw new Error(`missing ${wasmPath}; run scripts/build-wasm.ps1 or sh ./scripts/build-wasm.sh`);
  }
  const bytes = readFileSync(wasmPath);
  const compiled = new WebAssembly.Module(bytes);
  const imports = WebAssembly.Module.imports(compiled);
  const unexpected = imports.filter((item) => item.module !== "ut" || item.name !== "now_micros");
  if (unexpected.length > 0) {
    const names = unexpected.map((item) => `${item.module}.${item.name}`).join(", ");
    throw new Error(`shared wasm module imports a host-specific symbol: ${names}`);
  }
  const instance = new WebAssembly.Instance(compiled, {
    ut: {
      now_micros: () => BigInt(Math.round(performance.now() * 1000)),
    },
  });
  const exports = instance.exports as unknown as Exports;
  const required = [
    "memory",
    "ut_alloc",
    "ut_dealloc",
    "ut_free_result",
    "ut_execute",
    "ut_compile",
    "ut_validate",
    "ut_spawn",
    "ut_free",
  ] as const;
  for (const name of required) {
    if (exports[name] === undefined) {
      throw new Error(`shared wasm module is missing export ${name}`);
    }
  }
  return exports;
}

function writeText(text: string): { ptr: number; len: number } {
  const bytes = encoder.encode(text);
  if (bytes.length === 0) {
    return { ptr: 0, len: 0 };
  }
  const ptr = wasm.ut_alloc(bytes.length);
  if (ptr === 0) {
    throw new Error("wasm allocation failed");
  }
  new Uint8Array(wasm.memory.buffer).set(bytes, ptr);
  return { ptr, len: bytes.length };
}

function freeText(written: { ptr: number; len: number }): void {
  if (written.len > 0) {
    wasm.ut_dealloc(written.ptr, written.len);
  }
}

function readResult(ptr: number): string {
  if (ptr === 0) {
    throw new Error("wasm result allocation failed");
  }
  const view = new DataView(wasm.memory.buffer);
  const len = view.getUint32(ptr, true);
  const bytes = new Uint8Array(wasm.memory.buffer, ptr + 4, len);
  const text = decoder.decode(bytes);
  wasm.ut_free_result(ptr);
  return text;
}

/** JSON request in, JSON response out. The call compiles any schema on the request again. */
export function execute(request: string): string {
  const written = writeText(request);
  try {
    return readResult(wasm.ut_execute(written.ptr, written.len));
  } finally {
    freeText(written);
  }
}

/**
 * Compile a schema inside the module.
 * The returned JSON is `{ draft, handle }` or `{ draft, initError }` or `{ draft, failureMessage }`.
 */
export function compile(schemaJson: string, pointer: string, draft: string): string {
  const schema = writeText(schemaJson);
  const pointerBytes = writeText(pointer);
  const draftBytes = writeText(draft);
  try {
    return readResult(wasm.ut_compile(
      schema.ptr,
      schema.len,
      pointerBytes.ptr,
      pointerBytes.len,
      draftBytes.ptr,
      draftBytes.len,
    ));
  } finally {
    freeText(draftBytes);
    freeText(pointerBytes);
    freeText(schema);
  }
}

/** Validate one instance with a compiled handle. Returns the tool response JSON. */
export function validate(handle: number, instanceJson: string): string {
  const written = writeText(instanceJson);
  try {
    return readResult(wasm.ut_validate(handle, written.ptr, written.len));
  } finally {
    freeText(written);
  }
}

/** Compile a fragment of the document stored for `handle`. Same JSON shape as `compile`. */
export function spawn(handle: number, pointer: string): string {
  const written = writeText(pointer);
  try {
    return readResult(wasm.ut_spawn(handle, written.ptr, written.len));
  } finally {
    freeText(written);
  }
}

/** Drop a compiled schema. A second call for the same handle does nothing. */
export function free(handle: number): void {
  wasm.ut_free(handle);
}
