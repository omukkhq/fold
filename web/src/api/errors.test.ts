import { describe, expect, it } from "vitest";
import { ApiError, apiErrorCode, isApiError, toApiError } from "./errors";

const envelope = {
  error: { code: "project_not_found", message: "no such project", details: {} },
};

const notEnvelopes = [
  undefined,
  null,
  "project_not_found",
  42,
  {},
  { error: null },
  { error: "project_not_found" },
  { error: { code: 404 } },
  { code: "project_not_found" },
  new Error("project_not_found"),
];

describe("apiErrorCode", () => {
  it("reads_the_code_from_the_envelope", () => {
    expect(apiErrorCode(envelope)).toBe("project_not_found");
  });

  it("reads_the_code_of_a_thrown_api_error", () => {
    expect(apiErrorCode(new ApiError("project_unavailable", "m"))).toBe(
      "project_unavailable",
    );
  });

  it("returns_null_for_other_bodies", () => {
    for (const body of notEnvelopes) {
      expect(apiErrorCode(body)).toBeNull();
    }
  });
});

describe("isApiError", () => {
  it("matches_a_specific_code", () => {
    expect(isApiError(envelope, "project_not_found")).toBe(true);
    expect(isApiError(envelope, "project_unavailable")).toBe(false);
    expect(isApiError(new ApiError("x", "m"), "x")).toBe(true);
    expect(isApiError(new Error("x"), "x")).toBe(false);
    expect(
      isApiError({ message: "project_not_found" }, "project_not_found"),
    ).toBe(false);
  });
});

describe("toApiError", () => {
  it("keeps the code and the message of the envelope", () => {
    const error = toApiError(envelope);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      code: "project_not_found",
      message: "no such project",
    });
  });

  it("gives an empty message when the envelope has none", () => {
    expect(toApiError({ error: { code: "auth_required" } })).toMatchObject({
      code: "auth_required",
      message: "",
    });
  });

  it("returns null for other bodies", () => {
    for (const body of notEnvelopes) {
      expect(toApiError(body)).toBeNull();
    }
  });
});
