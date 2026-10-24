import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { execute, executeRaw } from "../src/index.ts";
import type { ToolRequest, ToolResponse } from "../src/types.ts";

const fixturesDir = join(dirname(fileURLToPath(import.meta.url)), "../../../../fixtures/json-schema");

interface Expectation {
  ok?: boolean;
  errorCode?: string;
  valid?: boolean;
  compiled?: boolean;
  withinBudget?: boolean;
  detected?: unknown;
  draft?: string;
  messageContains?: string;
  minErrors?: number;
  maxErrors?: number;
  errorTypes?: string[];
  exactTypes?: boolean;
}

interface Case {
  name: string;
  request?: ToolRequest;
  requestRaw?: string;
  expect: Expectation;
}

test("JavaScript runs the same JSON Schema cases as Rust", () => {
  const files = readdirSync(fixturesDir).filter((name) => name.endsWith(".json")).sort();
  assert.ok(files.length > 0);
  const failures: string[] = [];
  let count = 0;

  for (const file of files) {
    const cases = JSON.parse(readFileSync(join(fixturesDir, file), "utf8")) as Case[];
    for (const item of cases) {
      count += 1;
      const response: ToolResponse = item.requestRaw
        ? executeRaw(item.requestRaw)
        : execute(item.request as ToolRequest);
      const message = check(item.name, item.expect, response);
      if (message) {
        failures.push(`${message}\nresponse: ${JSON.stringify(response)}`);
      }
    }
  }

  assert.ok(count > 0);
  assert.deepEqual(failures, []);
});

function check(name: string, expect: Expectation, response: ToolResponse): string | null {
  if (expect.ok !== undefined && response.ok !== expect.ok) {
    return `${name}: ok is not ${expect.ok}`;
  }
  if (expect.errorCode !== undefined && response.error?.code !== expect.errorCode) {
    return `${name}: error code ${response.error?.code} != ${expect.errorCode}`;
  }
  if (expect.valid !== undefined && response.output?.valid !== expect.valid) {
    return `${name}: valid is not ${expect.valid}`;
  }
  if (expect.compiled !== undefined && response.output?.compiled !== expect.compiled) {
    return `${name}: compiled is not ${expect.compiled}`;
  }
  if (expect.withinBudget !== undefined && response.resources.withinBudget !== expect.withinBudget) {
    return `${name}: withinBudget is not ${expect.withinBudget}`;
  }
  if ("detected" in expect && JSON.stringify(response.output?.detected ?? null) !== JSON.stringify(expect.detected)) {
    return `${name}: detected mismatch`;
  }
  if (expect.draft !== undefined && response.output?.draft !== expect.draft) {
    return `${name}: draft is not ${expect.draft}`;
  }
  const messages = allMessages(response);
  if (expect.messageContains && !messages.some((message) => message.includes(expect.messageContains as string))) {
    return `${name}: no message contains ${expect.messageContains}`;
  }
  const types = (response.output?.errors ?? []).map((error) => error.type);
  if (expect.minErrors !== undefined && types.length < expect.minErrors) {
    return `${name}: expected at least ${expect.minErrors} errors, got ${types.length}`;
  }
  if (expect.maxErrors !== undefined && types.length > expect.maxErrors) {
    return `${name}: expected at most ${expect.maxErrors} errors, got ${types.length}`;
  }
  if (expect.errorTypes) {
    if (expect.exactTypes) {
      const expected = [...expect.errorTypes].sort();
      const actual = [...types].sort();
      if (JSON.stringify(expected) !== JSON.stringify(actual)) {
        return `${name}: error types ${actual.join(",")} != ${expected.join(",")}`;
      }
    } else if (expect.errorTypes.some((kind) => !types.includes(kind))) {
      return `${name}: missing error type from ${expect.errorTypes.join(",")}; got ${types.join(",")}`;
    }
  }
  return null;
}

function allMessages(response: ToolResponse): string[] {
  const found: string[] = [];
  if (response.error?.message) found.push(response.error.message);
  for (const error of response.output?.errors ?? []) found.push(error.message);
  if (response.output?.initError?.message) found.push(response.output.initError.message);
  return found;
}
