import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { ProposalDetail } from "@/api/proposals";
import { detail } from "../../../fixtures/proposals";
import { actionErrorText, canComment, visibleActions } from "./actions";

function proposal(
  id: string,
  fields: Partial<ProposalDetail> = {},
): ProposalDetail {
  const found = detail("demo", id);
  if (!found) throw new Error(`no fixture ${id}`);
  return { ...structuredClone(found), ...fields };
}

const ALL = {
  approve: true,
  request_changes: true,
  integrate: true,
  comment: true,
};
const NONE = {
  approve: false,
  request_changes: false,
  integrate: false,
  comment: false,
};

describe("visibleActions", () => {
  it("shows Approve, Request changes and Integrate to a named human when the payload allows them", () => {
    expect(visibleActions(proposal("P-4"), "tanim")).toEqual([
      "approve",
      "request_changes",
      "integrate",
    ]);
  });

  it("shows Integrate after a request for changes on the current version when the payload allows it", () => {
    const p = proposal("P-3");
    expect(p.latest_verdict).toBe("request_changes");
    expect(visibleActions(p, "tanim")).toContain("integrate");
  });

  it("hides Integrate when the payload does not allow it", () => {
    expect(visibleActions(proposal("P-6"), "tanim")).toEqual([
      "approve",
      "request_changes",
    ]);
    expect(
      visibleActions(
        proposal("P-4", { human_actions: { ...ALL, integrate: false } }),
        "tanim",
      ),
    ).not.toContain("integrate");
  });

  it("shows nothing to a human with no name, and nothing on a proposal that is not open", () => {
    expect(visibleActions(proposal("P-4"), null)).toEqual([]);
    expect(
      visibleActions(proposal("P-4", { state: "integrated" }), "tanim"),
    ).toEqual([]);
    expect(
      visibleActions(proposal("P-4", { human_actions: NONE }), "tanim"),
    ).toEqual([]);
  });

  it("never offers a reject", () => {
    expect(visibleActions(proposal("P-4"), "tanim")).not.toContain("reject");
  });
});

describe("canComment", () => {
  it("opens the composer only on the current version for a named human the payload lets comment", () => {
    const p = proposal("P-4");
    expect(canComment(p, "tanim", 2)).toBe(true);
    expect(canComment(p, "tanim", 1)).toBe(false);
    expect(canComment(p, "tanim", null)).toBe(false);
    expect(canComment(p, null, 2)).toBe(false);
    expect(
      canComment(proposal("P-4", { human_actions: NONE }), "tanim", 2),
    ).toBe(false);
    expect(canComment(proposal("P-5"), "tanim", 1)).toBe(false);
  });
});

describe("actionErrorText", () => {
  it("maps the codes the screen knows", () => {
    expect(actionErrorText(new ApiError("version_superseded", "x"))).toMatch(
      /newer version/,
    );
    expect(actionErrorText({ error: { code: "gate_unmet" } })).toMatch(
      /did not integrate/,
    );
  });

  it("leaves an unknown code and an error without a code to the screen", () => {
    expect(actionErrorText(new ApiError("disk_full", "no space"))).toBeNull();
    expect(actionErrorText(new Error("offline"))).toBeNull();
  });
});
