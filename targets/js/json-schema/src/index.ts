import { compile, execute as wasmExecute, free as freeHandle, spawn, validate } from "./wasm.ts";
import type {
  Detected,
  Draft,
  FragmentValidator,
  SchemaType,
  ToolRequest,
  ToolResponse,
  ValidateFunction,
  ValidationError,
  ValidationResult,
} from "./types.ts";

export type {
  Detected,
  Draft,
  FragmentValidator,
  ResourceBudget,
  ResourceReport,
  SchemaType,
  ToolError,
  ToolOutput,
  ToolRequest,
  ToolResponse,
  ValidateFunction,
  ValidationError,
  ValidationErrorType,
  ValidationResult,
} from "./types.ts";

/**
 * The schema could not be compiled. `initError` is the same object
 * `DataValidator.validate` returns for an invalid schema.
 */
export class SchemaCompilationError extends Error {
  readonly initError: ValidationError;

  constructor(initError: ValidationError) {
    super(initError.message);
    this.name = "SchemaCompilationError";
    this.initError = initError;
  }
}

/**
 * A schema compiled once inside the WebAssembly module.
 *
 * Construct it with `compileSchema`. `validate` and `evaluate` send only the
 * instance. `spawn` compiles a fragment from the document already stored in
 * the module. Call `free`, or use `using`, when the schema is no longer needed.
 * Garbage collection also releases it.
 */
const releasedHandles = new FinalizationRegistry((handle: number) => {
  freeHandle(handle);
});

export class CompiledValidator {
  private handle: number | null;
  readonly draft: string;

  private constructor(handle: number, draft: string) {
    this.handle = handle;
    this.draft = draft;
    releasedHandles.register(this, handle, this);
  }

  /** @internal */
  static fromWasm(handle: number, draft: string): CompiledValidator {
    return new CompiledValidator(handle, draft);
  }

  /**
   * Check one instance.
   *
   * Returns the schema mismatches. An empty array means the instance is valid.
   * Throws when the call itself fails, including a resource-budget failure.
   * The thrown message is `error.message` from the tool response.
   */
  validate(instance: unknown): ValidationError[] {
    return errorsFrom(this.evaluate(instance));
  }

  /**
   * Check one instance and return the full tool response, including `resources`.
   * The schema compiled by this object is not sent again.
   */
  evaluate(instance: unknown): ToolResponse {
    const response = JSON.parse(validate(this.current(), encodeJson(instance))) as ToolResponse;
    return response;
  }

  /** Compile `pointer` against the document stored with this schema. */
  spawn(pointer: string): CompiledValidator {
    const opened = readOutcome(spawn(this.current(), pointer));
    return finishCompile(opened);
  }

  /** Drop the compiled schema inside the WebAssembly module. */
  free(): void {
    if (this.handle === null) {
      return;
    }
    releasedHandles.unregister(this);
    const handle = this.handle;
    this.handle = null;
    freeHandle(handle);
  }

  [Symbol.dispose](): void {
    this.free();
  }

  private current(): number {
    if (this.handle === null) {
      throw new Error("compiled validator was released");
    }
    return this.handle;
  }
}

/**
 * Compile `schema` once and keep the validator in the WebAssembly module.
 *
 * `pointer` selects a fragment. It defaults to the document root. `draft`
 * overrides the draft chosen from `$schema`. Throws `SchemaCompilationError`
 * when the schema cannot be compiled.
 */
export function compileSchema(schema: unknown, pointer = "#/", draft?: Draft): CompiledValidator {
  return finishCompile(openSchema(schema, pointer, draft));
}

/** JSON text in, parsed response out. Invalid JSON is reported by the tool. */
export function executeRaw(request: string): ToolResponse {
  return JSON.parse(wasmExecute(request)) as ToolResponse;
}

/**
 * Stateless JSON-in, JSON-out call.
 *
 * The whole request, including `schema`, is sent on every call and compiled
 * again. Use `compileSchema` or `DataValidator` when one schema checks many
 * instances. `ok` is not the validation result; see `ToolResponse`.
 */
export function execute(request: ToolRequest): ToolResponse {
  return executeRaw(JSON.stringify(request));
}

export function detectContentType(content: unknown): Detected | null {
  const response = execute({ operation: "detect", schema: content });
  if (!response.ok || !response.output) {
    throw new Error(response.error?.message ?? "detect failed");
  }
  return response.output.detected ?? null;
}

/**
 * Check that a document is a valid schema of one family and version.
 *
 * Each call sends that document, because the document is the instance. The
 * JSON Schema, OpenAPI, and AsyncAPI specification schemas are compiled once
 * inside the module and then reused. When `version` does not agree with the
 * version detected on the document, the result is a `SCHEMA_VIOLATION`.
 */
export class SchemaValidator {
  private readonly schemaType: SchemaType;
  private readonly version: string;

  constructor(schemaType: SchemaType, version: string) {
    this.schemaType = schemaType;
    this.version = version;
  }

  /** Schema mismatches. An empty array means the document is a valid schema. */
  validate(content: unknown): ValidationError[] {
    const response = execute({
      operation: "validateSchema",
      schema: content,
      schemaType: this.schemaType,
      version: this.version,
      draft: this.schemaType === "jsonschema" ? (this.version as Draft) : undefined,
    });
    if (!response.ok || !response.output?.errors) {
      throw new Error(response.error?.message ?? "schema validation failed");
    }
    return response.output.errors;
  }
}

/**
 * Detect the document family and validate it as a schema.
 *
 * Returns `{ success, type, version, errors }`. Throws when no family can be
 * detected. `success` is `errors.length === 0`.
 */
export function validateSchema(content: Record<string, unknown>): ValidationResult {
  const metadata = detectContentType(content);
  if (!metadata) {
    throw new Error("Unknown document type - unable to validate");
  }
  const validator = new SchemaValidator(metadata.type, metadata.version);
  const errors = validator.validate(content);
  return {
    success: errors.length === 0,
    type: metadata.type,
    version: metadata.version,
    errors,
  };
}

/**
 * Compile a data schema once and validate many instances.
 *
 * An invalid schema does not throw here. `validate` returns that initialization
 * error, and `spawnFragmentValidator` throws
 * `Validator initialization, cannot spawn fragment validator`.
 */
export class DataValidator {
  private compiled: CompiledValidator | null;
  private readonly initError: ValidationError | null;

  constructor(schema: unknown, pointer = "#/") {
    const opened = openSchema(schema, pointer);
    if (opened.failureMessage) {
      discard(opened.schema);
      throw new Error(opened.failureMessage);
    }
    if (opened.schema === null || opened.initError) {
      discard(opened.schema);
      this.compiled = null;
      this.initError = opened.initError ?? {
        type: "UNKNOWN_ERROR",
        message: "Initialization failed",
      };
      return;
    }
    this.compiled = CompiledValidator.fromWasm(opened.schema, opened.draft);
    this.initError = null;
  }

  /**
   * Schema mismatches for `data`. Empty means valid.
   * After a failed initialization, this returns that one error on every call.
   */
  validate(data: unknown): ValidationError[] {
    if (this.initError) {
      return [this.initError];
    }
    if (!this.compiled) {
      throw new Error("compiled validator was released");
    }
    return this.compiled.validate(data);
  }

  /**
   * Compile another fragment of the same document.
   * The document is not sent again. The returned function validates with that
   * compiled fragment and releases it through `free`.
   */
  spawnFragmentValidator(pointer: string): FragmentValidator {
    if (this.initError) {
      throw new Error("Validator initialization, cannot spawn fragment validator");
    }
    if (!this.compiled) {
      throw new Error("compiled validator was released");
    }
    let child: CompiledValidator;
    try {
      child = this.compiled.spawn(pointer);
    } catch (error) {
      if (error instanceof SchemaCompilationError) {
        throw new Error(error.initError.message);
      }
      throw error;
    }
    const validate = ((data: unknown) => child.validate(data)) as FragmentValidator;
    validate.free = () => {
      child.free();
    };
    return validate;
  }

  /** Drop the compiled schema inside the WebAssembly module. */
  free(): void {
    this.compiled?.free();
    this.compiled = null;
  }

  [Symbol.dispose](): void {
    this.free();
  }
}

/**
 * Compile `schema` once and return a type guard.
 *
 * `validate(data)` is `true` when the instance matches. The latest mismatches
 * are on `validate.errors`. `validate.free()` releases the compiled schema.
 */
export function generateDataValidator<T>(schema: unknown, pointer = "#/"): ValidateFunction<T> {
  const validator = new DataValidator(schema, pointer);
  const validate = ((data: unknown): data is T => {
    validate.errors = validator.validate(data);
    return validate.errors.length === 0;
  }) as ValidateFunction<T>;
  validate.errors = [];
  validate.free = () => {
    validator.free();
  };
  return validate;
}

interface OpenedSchema {
  schema: number | null;
  initError: ValidationError | null;
  failureMessage: string | null;
  draft: string;
}

interface CompileBody {
  draft: string;
  handle?: number;
  initError?: ValidationError;
  failureMessage?: string;
}

function openSchema(schema: unknown, pointer: string, draft?: Draft): OpenedSchema {
  return readOutcome(compile(encodeJson(schema), pointer, draft ?? ""));
}

function readOutcome(raw: string): OpenedSchema {
  const body = JSON.parse(raw) as CompileBody;
  return {
    schema: body.handle ?? null,
    initError: body.initError ?? null,
    failureMessage: body.failureMessage ?? null,
    draft: body.draft,
  };
}

function discard(handle: number | null): void {
  if (handle !== null) {
    freeHandle(handle);
  }
}

function finishCompile(opened: OpenedSchema): CompiledValidator {
  if (opened.failureMessage) {
    discard(opened.schema);
    throw new Error(opened.failureMessage);
  }
  if (opened.schema === null || opened.initError) {
    discard(opened.schema);
    throw new SchemaCompilationError(opened.initError ?? {
      type: "UNKNOWN_ERROR",
      message: "Initialization failed",
    });
  }
  return CompiledValidator.fromWasm(opened.schema, opened.draft);
}

function errorsFrom(response: ToolResponse): ValidationError[] {
  if (!response.ok || !response.output?.errors) {
    throw new Error(response.error?.message ?? "validation failed");
  }
  return response.output.errors;
}

function encodeJson(value: unknown): string {
  const encoded = JSON.stringify(value);
  if (typeof encoded !== "string") {
    throw new Error("value is not JSON");
  }
  return encoded;
}
