export type SchemaType = "openapi" | "asyncapi" | "jsonschema";

export type Draft = "draft-04" | "draft-06" | "draft-07" | "2019-09" | "2020-12";

export type ValidationErrorType =
  | "MISSING_REQUIRED_FIELD"
  | "INVALID_TYPE"
  | "INVALID_FORMAT"
  | "CONSTRAINT_VIOLATION"
  | "PATTERN_MISMATCH"
  | "ENUM_MISMATCH"
  | "INVALID_REFERENCE"
  | "SCHEMA_VIOLATION"
  | "UNKNOWN_ERROR";

/**
 * One schema mismatch.
 *
 * `type` is the stable vocabulary. `path` is an RFC 6901 JSON Pointer to the
 * instance location and is omitted for errors that have no instance location.
 * `details.keyword` is the schema keyword that failed, and `details.schemaPath`
 * is the pointer to that keyword.
 */
export interface ValidationError {
  type: ValidationErrorType;
  message: string;
  path?: string;
  details?: Record<string, unknown>;
}

export interface Detected {
  type: SchemaType;
  version: string;
}

export interface ResourceBudget {
  maxElapsedMicros: number;
  maxPeakBytes: number;
}

export interface ResourceReport {
  elapsedMicros: number;
  peakBytes: number;
  allocatedBytes: number;
  withinBudget: boolean;
  budget: ResourceBudget;
}

export type Operation = "detect" | "compile" | "validateData" | "validateSchema" | "describe" | "burn";

export interface ToolRequest {
  operation: Operation;
  schema?: unknown;
  instance?: unknown;
  pointer?: string;
  draft?: Draft;
  schemaType?: SchemaType;
  /** Requested specification version. Compared with the version detected on the document. */
  version?: string;
  burnMicros?: number;
  budget?: Partial<ResourceBudget>;
}

export interface ToolOutput {
  valid?: boolean;
  errors?: ValidationError[];
  detected?: Detected | null;
  draft?: string;
  compiled?: boolean;
  initError?: ValidationError | null;
  description?: Record<string, unknown>;
}

/**
 * The tool failed before it could report a validation result.
 *
 * `INVALID_INPUT` means the request or instance was not usable.
 * `RESOURCE_LIMIT` means this call crossed the CPU or memory budget.
 * `CONTRACT_VIOLATION` means the tool built a response outside its output schema.
 * `INTERNAL` means the engine panicked or could not check a contract.
 */
export interface ToolError {
  code: "INVALID_INPUT" | "RESOURCE_LIMIT" | "CONTRACT_VIOLATION" | "INTERNAL";
  message: string;
  errors?: ValidationError[];
}

/**
 * One black-box call.
 *
 * `ok` means the call stayed inside its contract and resource budget. It does
 * not mean the instance matched the schema. Read `output.valid` for that.
 * A validation failure is `ok: true` with `output.valid: false` and
 * `output.errors`. A tool failure is `ok: false` with `error` set.
 */
export interface ToolResponse {
  ok: boolean;
  operation: string;
  output?: ToolOutput;
  error?: ToolError;
  resources: ResourceReport;
}

export interface ValidationResult {
  success: boolean;
  type: SchemaType;
  version: string;
  errors: ValidationError[];
}

/** Type guard returned by `generateDataValidator`. `errors` is from the latest call. */
export type ValidateFunction<T> = ((data: unknown) => data is T) & {
  errors: ValidationError[];
  /** Release the compiled schema stored in the WebAssembly module. */
  free(): void;
};

/** Function returned by `DataValidator.spawnFragmentValidator`. */
export type FragmentValidator = ((data: unknown) => ValidationError[]) & {
  /** Release the compiled fragment stored in the WebAssembly module. */
  free(): void;
};
