import { describe, expect, it } from "vitest";
import type { DurableEvent } from "../../api/events";
import schema from "../../api/schema.d.ts?raw";
import { Check, MessageSquare, X } from "lucide-react";
import { EVENT_TYPES, eventLine } from "./eventLine";

function event(
  type: string,
  over: Partial<Omit<DurableEvent, "type">> = {},
): Omit<DurableEvent, "type"> & { type: string } {
  return {
    seq: 1,
    type,
    actor: "frontend/ui",
    task: "T-12",
    proposal: "P-4",
    workspace: "ws-9",
    revision: "r17",
    data: {},
    created_at: 0,
    ...over,
  };
}

/** The `DurableEventType` union as the generated schema declares it. */
function declaredTypes(): string[] {
  const union = /DurableEventType: ([^;]+);/.exec(schema)?.[1] ?? "";
  return [...union.matchAll(/"([^"]+)"/g)].map((m) => m[1] ?? "");
}

describe("eventLine", () => {
  it("has a line for exactly the stored event types of the generated schema", () => {
    const declared = declaredTypes();
    expect(declared.length).toBeGreaterThan(40);
    expect([...EVENT_TYPES].sort()).toEqual([...declared].sort());
  });

  it.each(EVENT_TYPES)("writes a line for %s with every id present", (type) => {
    const line = eventLine(event(type));
    expect(line.text.length).toBeGreaterThan(0);
    expect(line.icon).toBeTruthy();
  });

  it.each(EVENT_TYPES)(
    "writes a line for %s with no ids and no data",
    (type) => {
      const line = eventLine(
        event(type, {
          task: null,
          proposal: null,
          workspace: null,
          revision: null,
        }),
      );
      expect(line.text).not.toMatch(/null|undefined/);
    },
  );

  it("names the unknown type in a generic line", () => {
    const line = eventLine(event("billing.invoiced"));
    expect(line.text).toBe("event billing.invoiced");
    expect(line.emphasis).toBe(false);
  });

  it("does not take an inherited property for a type", () => {
    expect(eventLine(event("constructor")).text).toBe("event constructor");
  });

  it("reads review verdicts with the version they reviewed", () => {
    const line = (verdict: unknown) =>
      eventLine(event("review.submitted", { data: { verdict, version: 2 } }));
    expect(line("approve")).toMatchObject({
      text: "approved P-4 v2",
      icon: Check,
      emphasis: true,
    });
    expect(line("request_changes")).toMatchObject({
      text: "requested changes on P-4 v2",
      icon: X,
      emphasis: false,
    });
    expect(line("reject")).toMatchObject({
      text: "rejected P-4 v2",
      icon: X,
      emphasis: false,
    });
    expect(line("comment")).toMatchObject({
      text: "commented on P-4 v2",
      icon: MessageSquare,
      emphasis: false,
    });
    expect(
      eventLine(event("review.submitted", { data: { verdict: 7 } })).text,
    ).toBe("commented on P-4");
  });

  it("reads the data a kind carries and falls back without it", () => {
    const line = (type: string, data: DurableEvent["data"] = {}) =>
      eventLine(event(type, { data })).text;
    expect(
      line("task.status_changed", { from: "todo", to: "in_progress" }),
    ).toBe("T-12 moved to In Progress");
    expect(
      line("task.status_changed", { from: "review", to: "in_progress" }),
    ).toBe("T-12 back to In Progress");
    expect(line("task.status_changed", { to: "done" })).toBe(
      "T-12 moved to Done",
    );
    expect(line("task.status_changed", { to: "shelved" })).toBe(
      "T-12 changed status",
    );
    expect(line("task.status_changed")).toBe("T-12 changed status");
    expect(line("task.blocked_changed", { blocked: true })).toBe(
      "T-12 is blocked",
    );
    expect(line("task.blocked_changed", { blocked: false })).toBe(
      "T-12 is unblocked",
    );
    expect(line("dependency.added", { needs: "T-3" })).toBe("T-12 needs T-3");
    expect(line("dependency.added")).toBe("added a dependency on T-12");
    expect(line("dependency.removed", { needs: "T-3" })).toBe(
      "T-12 no longer needs T-3",
    );
    expect(line("dependency.removed")).toBe("removed a dependency on T-12");
    expect(line("claim.acquired", { kind: "review" })).toBe(
      "claimed the review of P-4",
    );
    expect(line("claim.acquired", { kind: "implementation" })).toBe(
      "claimed T-12",
    );
    expect(line("claim.released", { kind: "review" })).toBe(
      "released the review of P-4",
    );
    expect(line("claim.released")).toBe("released T-12");
    expect(line("claim.taken_over", { kind: "review" })).toBe(
      "took over the review of P-4",
    );
    expect(line("claim.taken_over", { kind: "implementation" })).toBe(
      "took over T-12",
    );
    expect(line("workspace.created", { kind: "review", version: 2 })).toBe(
      "checked out P-4 v2 in ws-9",
    );
    expect(line("workspace.created", { kind: "task" })).toBe(
      "created ws-9 on T-12",
    );
    expect(
      line("checkpoint.recorded", { checkpoint: "ck-31", kind: "explicit" }),
    ).toBe("recorded ck-31 on T-12");
    expect(
      line("checkpoint.recorded", { checkpoint: "ck-31", kind: "auto" }),
    ).toBe("auto checkpoint ck-31 on T-12");
    expect(line("checkpoint.recorded")).toBe("recorded a checkpoint on T-12");
    expect(line("checkpoint.restored", { checkpoint: "ck-2" })).toBe(
      "restored ck-2 in ws-9",
    );
    expect(line("checkpoint.pruned", { checkpoint: "ck-3" })).toBe(
      "pruned ck-3 in ws-9",
    );
    expect(
      eventLine(
        event("checkpoint.pruned", {
          workspace: null,
          data: { checkpoint: "ck-3" },
        }),
      ).text,
    ).toBe("pruned ck-3");
    expect(line("checkpoint.pruned")).toBe("pruned a checkpoint in ws-9");
    expect(line("changeset.recorded", { changeset: "C-9" })).toBe(
      "recorded C-9 on T-12",
    );
    expect(line("proposal.version_added", { version: 2 })).toBe(
      "added v2 to P-4",
    );
    expect(line("proposal.version_added")).toBe("added a version to P-4");
    expect(line("message.sent", { message: "m-31" })).toBe("sent m-31 on T-12");
    expect(line("message.acknowledged", { ids: ["m-31"] })).toBe(
      "acknowledged m-31",
    );
    expect(line("message.acknowledged", { ids: ["m-30", "m-31", 4] })).toBe(
      "acknowledged 2 messages",
    );
    expect(line("message.acknowledged")).toBe("acknowledged a message");
    expect(line("proposal.opened", { version: 1 })).toBe(
      "opened P-4 v1 for T-12",
    );
  });

  it("falls back to a plain noun when an id is missing", () => {
    const bare = {
      task: null,
      proposal: null,
      workspace: null,
      revision: null,
    };
    expect(eventLine(event("proposal.opened", bare)).text).toBe(
      "opened a proposal",
    );
    expect(eventLine(event("proposal.opened")).text).toBe(
      "opened P-4 for T-12",
    );
    expect(eventLine(event("proposal.integrated", bare)).text).toBe(
      "integrated a proposal into main",
    );
    expect(eventLine(event("proposal.integrated")).text).toBe(
      "integrated P-4 into main as r17",
    );
    expect(eventLine(event("integration.succeeded", bare)).text).toBe(
      "integration succeeded",
    );
    expect(eventLine(event("integration.succeeded")).text).toBe(
      "integration succeeded as r17",
    );
    expect(eventLine(event("main.advanced", bare)).text).toBe("main advanced");
    expect(eventLine(event("main.advanced")).text).toBe("main advanced to r17");
  });

  it("marks integrations and main moves as emphasised, approvals by their verdict", () => {
    const emphasised = EVENT_TYPES.filter(
      (type) => eventLine(event(type)).emphasis,
    );
    expect(emphasised).toEqual([
      "proposal.integrated",
      "integration.succeeded",
      "main.advanced",
    ]);
  });
});
