import { describe, expect, it } from "vitest";
import { apiErrorCode } from "./errors";
import { expectOk, unwrap } from "./result";

const response = (status: number) => new Response(null, { status });

describe("unwrap", () => {
  it("returns the body", async () => {
    await expect(
      unwrap(Promise.resolve({ data: { a: 1 }, response: response(200) })),
    ).resolves.toEqual({ a: 1 });
  });

  it("throws the envelope's code", async () => {
    const error = { error: { code: "task_closed", message: "", details: {} } };
    await expect(
      unwrap(Promise.resolve({ error, response: response(409) })),
    ).rejects.toSatisfy((e: unknown) => apiErrorCode(e) === "task_closed");
  });

  it("throws a coded error when the answer has neither body nor envelope", async () => {
    await expect(
      unwrap(Promise.resolve({ response: response(204) })),
    ).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "daemon_unavailable",
    );
  });
});

describe("expectOk", () => {
  it("accepts an answer with no body", async () => {
    await expect(
      expectOk(Promise.resolve({ response: response(204) })),
    ).resolves.toBeUndefined();
  });

  it("throws the envelope's code, or a coded error for a failed answer without one", async () => {
    const error = {
      error: { code: "dependency_not_found", message: "", details: {} },
    };
    await expect(
      expectOk(Promise.resolve({ error, response: response(404) })),
    ).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "dependency_not_found",
    );
    await expect(
      expectOk(Promise.resolve({ response: response(502) })),
    ).rejects.toSatisfy(
      (e: unknown) => apiErrorCode(e) === "daemon_unavailable",
    );
  });
});
