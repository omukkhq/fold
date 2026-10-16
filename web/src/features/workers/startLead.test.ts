import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import {
  defaultModelText,
  startFailure,
  startLeadBody,
  startLeadDefaults,
  startLeadSchema,
  stopFailure,
} from "./startLead";

describe("startLeadSchema", () => {
  it("needs an area for an area lead only", () => {
    const area = startLeadSchema.safeParse({
      role: "area_lead",
      area: "",
      model: "",
      instructions: "",
    });
    expect(area.success).toBe(false);
    expect(area.error?.issues[0]?.path).toEqual(["area"]);
    expect(
      startLeadSchema.safeParse({
        role: "review_lead",
        area: "",
        model: "",
        instructions: "",
      }).success,
    ).toBe(true);
  });
});

describe("startLeadDefaults", () => {
  it("starts on the first area, or on the review lead without areas", () => {
    expect(startLeadDefaults(["web", "api"])).toMatchObject({
      role: "area_lead",
      area: "web",
    });
    expect(startLeadDefaults([])).toMatchObject({
      role: "review_lead",
      area: "",
    });
  });
});

describe("startLeadBody", () => {
  it("sends the area of an area lead and leaves out empty fields", () => {
    expect(
      startLeadBody({
        role: "area_lead",
        area: "web",
        model: "opus",
        instructions: "  Split T-18.  ",
      }),
    ).toEqual({
      role: "area_lead",
      area: "web",
      model: "opus",
      instructions: "Split T-18.",
    });
    expect(
      startLeadBody({
        role: "review_lead",
        area: "web",
        model: "",
        instructions: " ",
      }),
    ).toEqual({ role: "review_lead" });
  });
});

describe("failures", () => {
  const values = {
    role: "area_lead" as const,
    area: "backend",
    model: "",
    instructions: "",
  };

  it.each([
    ["area_has_lead", "A lead for the backend area is already running"],
    ["worker_exists", "already running"],
    ["worker_limit_reached", "as many workers"],
    ["area_not_found", "no area named backend"],
    ["invalid_config", "fld doctor"],
    ["worker_start_denied", "only the leads"],
    ["human_name_required", "Tell fold your name"],
  ])("words %s", (code, text) => {
    const failure = startFailure(new ApiError(code, ""), values);
    expect(failure.code).toBe(code);
    expect(failure.text).toContain(text);
  });

  it("has no code for a failure that never reached the daemon", () => {
    expect(
      startFailure(new Error("offline"), { ...values, role: "review_lead" }),
    ).toEqual({
      code: null,
      text: "The request did not reach the daemon. Try again.",
    });
  });

  it("words a refused stop", () => {
    expect(stopFailure(new ApiError("worker_process_not_found", "")).text).toBe(
      "fold runs no process for this worker.",
    );
    expect(stopFailure(new ApiError("human_name_required", "")).code).toBe(
      "human_name_required",
    );
  });
});

describe("defaultModelText", () => {
  it("names the model the config sets for the role", () => {
    const roles = [
      { role: "area_lead" as const, cache_ttl: "1h", model: "opus" },
      { role: "review_lead" as const, cache_ttl: "1h", model: null },
    ];
    expect(defaultModelText(roles, "area_lead")).toBe(
      "The role's model from the project config (opus)",
    );
    expect(defaultModelText(roles, "review_lead")).toBe(
      "The role's model from the project config",
    );
  });
});
