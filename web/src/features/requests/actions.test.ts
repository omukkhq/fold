import { describe, expect, it } from "vitest";
import type { RequestState } from "@/api/requests";
import { requestSummary } from "@/test/requests";
import {
  decisionText,
  isFinal,
  isRequester,
  panelKind,
  pendingText,
  threadClosedText,
  threadTarget,
  versionsNote as versionsNoteSegments,
  viewerText as viewerTextSegments,
  visibleActions,
} from "./actions";
import { segmentsText } from "@/people/segments";

const versionsNote = (...args: Parameters<typeof versionsNoteSegments>) =>
  segmentsText(versionsNoteSegments(...args));

const viewerText = (...args: Parameters<typeof viewerTextSegments>) =>
  segmentsText(viewerTextSegments(...args));

describe("visibleActions", () => {
  it.each<[RequestState, string[]]>([
    ["open", ["close"]],
    ["drafting", ["close"]],
    ["draft_ready", ["approve", "request_changes", "edit", "close"]],
    ["changes_requested", ["request_changes", "edit", "close"]],
    ["approved", []],
    ["closed", []],
  ])("gives the requester of a %s request %j", (state, actions) => {
    expect(visibleActions(requestSummary("RQ-1", { state }), "tanim")).toEqual(
      actions,
    );
  });

  it("gives anyone else no action", () => {
    const r = requestSummary("RQ-1", { state: "draft_ready" });
    expect(visibleActions(r, "ana")).toEqual([]);
    expect(visibleActions(r, null)).toEqual([]);
    expect(
      isRequester(requestSummary("RQ-2", { requester: "backend" }), "backend"),
    ).toBe(false);
  });

  it("knows the final states", () => {
    expect(isFinal("approved")).toBe(true);
    expect(isFinal("closed")).toBe(true);
    expect(isFinal("draft_ready")).toBe(false);
  });
});

describe("texts", () => {
  it("lists what the requester can do", () => {
    const ready = requestSummary("RQ-4", { state: "draft_ready" });
    expect(decisionText(ready, visibleActions(ready, "tanim"))).toBe(
      "You asked for RQ-4, so you can approve it, ask for changes, edit it or close it. Anyone else sees no actions.",
    );
    const open = requestSummary("RQ-7");
    expect(decisionText(open, ["close"])).toBe(
      "You asked for RQ-7, so you can close it. The cards come once an orchestrator drafts them. Anyone else sees no actions.",
    );
    expect(viewerText(requestSummary("RQ-4"))).toBe(
      "Only @tanim can approve, ask for changes, edit or close.",
    );
  });

  it("says fold is saving the decision while an action is pending", () => {
    const ready = requestSummary("RQ-4", { state: "draft_ready" });
    expect(decisionText(ready, visibleActions(ready, "tanim"), true)).toBe(
      "You asked for RQ-4, so you can approve it, ask for changes, edit it or close it. fold is saving your decision.",
    );
  });

  it("explains approval of versions to the requester and to everyone else", () => {
    const r = requestSummary("RQ-4", { state: "draft_ready" });
    expect(versionsNote(r, "tanim")).toBe(
      "Approval applies to the version you see. If a new version arrives first, fold asks you to look again.",
    );
    expect(versionsNote(r, "omar")).toBe(
      "@tanim approves one version at a time. Each new version starts unapproved.",
    );
    expect(versionsNote(r, null)).toBe(
      "@tanim approves one version at a time. Each new version starts unapproved.",
    );
  });

  it("says what fold is doing while an action is pending", () => {
    const r = requestSummary("RQ-4", {
      state: "draft_ready",
      latest_version: 2,
    });
    expect(pendingText("approve", r, 3)).toMatch(
      /^Waiting for fold to create 3 tasks\./,
    );
    expect(pendingText("approve", r, 1)).toMatch(/create 1 task\./);
    expect(pendingText("request_changes", r, 3)).toMatch(
      /save the comment on v2/,
    );
    expect(pendingText("edit", r, 3)).toMatch(
      /save v3\. The cards and edges stay locked/,
    );
    expect(pendingText("close", r, 3)).toMatch(/close RQ-4/);
    expect(pendingText("edit", requestSummary("RQ-1"), 0)).toMatch(/save v1/);
  });

  it("closes the thread of a final request", () => {
    expect(
      threadClosedText(requestSummary("RQ-1", { state: "approved" })),
    ).toMatch(/approved/);
    expect(
      threadClosedText(requestSummary("RQ-1", { state: "closed" })),
    ).toMatch(/closed/);
    expect(threadClosedText(requestSummary("RQ-1"))).toBeNull();
  });
});

describe("threadTarget", () => {
  it("sends to the drafter, else the requester, else every agent", () => {
    expect(
      threadTarget(requestSummary("RQ-1", { drafter: "backend" }), "tanim"),
    ).toEqual({
      to: undefined,
      label: "@backend",
    });
    expect(threadTarget(requestSummary("RQ-1"), "ana")).toEqual({
      to: ["human:tanim"],
      label: "@tanim",
    });
    expect(threadTarget(requestSummary("RQ-1"), "tanim")).toEqual({
      to: ["all"],
      label: "all agents",
    });
  });
});

describe("panelKind", () => {
  it("picks the side panel", () => {
    const r = (state: RequestState) => requestSummary("RQ-1", { state });
    expect(panelKind(r("approved"), "tanim", false)).toBe("approved");
    expect(panelKind(r("closed"), "ana", false)).toBe("closed");
    expect(panelKind(r("draft_ready"), "ana", false)).toBe("viewer");
    expect(panelKind(r("draft_ready"), "tanim", true)).toBe("editing");
    expect(panelKind(r("draft_ready"), "tanim", false)).toBe("decide");
  });
});
