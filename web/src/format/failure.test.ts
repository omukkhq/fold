import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { failureText } from "./failure";

describe("failureText", () => {
  it("words a known code", () => {
    expect(failureText(new ApiError("not_found", "x"))).toBe(
      "The daemon does not serve this request yet.",
    );
    expect(failureText(new ApiError("agent_not_found", "x"))).toBe(
      "fold has no agent with this name.",
    );
    expect(
      failureText({ error: { code: "task_not_found", message: "x" } }),
    ).toBe("No task has this id.");
  });

  it("words an unknown code without showing it", () => {
    const text = failureText(new ApiError("brand_new_code", "x"));
    expect(text).toBe("The daemon refused the request.");
    expect(text).not.toContain("brand_new_code");
  });

  it("says the request did not arrive when there is no code", () => {
    expect(failureText(new Error("boom"))).toBe(
      "The request did not reach the daemon. Try again.",
    );
    expect(failureText(null)).toBe(
      "The request did not reach the daemon. Try again.",
    );
  });
});
