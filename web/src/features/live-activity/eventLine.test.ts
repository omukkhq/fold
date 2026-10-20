import { describe, expect, it } from "vitest";
import type { DurableEvent } from "../../api/events";
import schema from "../../api/schema.d.ts?raw";
import { Check, MessageSquare, X } from "lucide-react";
import { codeParts } from "../../format/codeText";
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

  it("names the agent and the worker of a refused session", () => {
    const line = (data: DurableEvent["data"]) =>
      eventLine(event("session.bind_refused", { data })).text;
    expect(line({ agent: "main", worker: "web-worker", pid: 4242 })).toBe(
      "refused a session as main from a process of web-worker",
    );
    expect(line({ agent: "main", worker: null })).toBe(
      "refused a session as main from a worker's process",
    );
    expect(line({})).toBe("refused a session from a worker's process");
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
    expect(line("proposal.human_edited", { version: 3 })).toBe(
      "edited P-4 as v3",
    );
    expect(line("proposal.human_edited")).toBe("edited P-4");
    expect(line("proposal.reviewer_fixed", { version: 3 })).toBe(
      "fixed P-4 in review as v3",
    );
    expect(line("proposal.reviewer_fixed")).toBe("fixed P-4 in review");
    expect(
      line("review.scope_chosen", { scope: "full", version: 3, reason: "API" }),
    ).toBe("chose a full review of P-4 v3");
    expect(line("review.scope_chosen")).toBe("chose the review scope of P-4");
    expect(line("note.added", { note: "n-4", area: "web" })).toBe(
      "noted n-4 in web",
    );
    expect(line("note.added", { note: "n-5", area: null })).toBe("noted n-5");
    expect(line("note.added")).toBe("noted a decision");
    expect(line("proposal.review_again_requested", { version: 3 })).toBe(
      "sent v3 of P-4 for review again",
    );
    expect(line("proposal.review_again_requested")).toBe(
      "sent P-4 for review again",
    );
    expect(
      line("proposal.author_changed", {
        reason: "taken_over",
        from: "frontend",
        to: "backend",
      }),
    ).toBe("took over P-4 from frontend");
    expect(line("proposal.author_changed")).toBe("took over P-4");
    expect(line("message.sent", { message: "m-31" })).toBe("sent m-31 on T-12");
    expect(line("message.acknowledged", { ids: ["m-31"] })).toBe(
      "acknowledged m-31",
    );
    expect(line("message.acknowledged", { ids: ["m-30", "m-31", 4] })).toBe(
      "acknowledged 2 messages",
    );
    expect(line("message.acknowledged")).toBe("acknowledged a message");
    expect(line("request.created", { request: "RQ-4" })).toBe("created RQ-4");
    expect(line("request.created")).toBe("created a request");
    expect(line("request.claimed", { request: "RQ-4" })).toBe("claimed RQ-4");
    expect(line("request.claimed", { request: "RQ-4", taken_over: true })).toBe(
      "took over RQ-4",
    );
    expect(line("request.released", { request: "RQ-4" })).toBe("released RQ-4");
    expect(
      line("request.draft_submitted", { request: "RQ-4", version: 2 }),
    ).toBe("drafted RQ-4 v2");
    expect(
      line("request.changes_requested", { request: "RQ-4", version: 1 }),
    ).toBe("requested changes on RQ-4 v1");
    expect(
      line("request.approved", { request: "RQ-4", tasks: ["T-12", "T-13", 5] }),
    ).toBe("approved RQ-4 as T-12, T-13");
    expect(line("request.approved")).toBe("approved a request");
    expect(line("request.closed", { request: "RQ-4" })).toBe("closed RQ-4");
    expect(
      line("review.batch_created", {
        batch: "rb-3",
        members: [{ proposal: "P-3" }, { proposal: "P-4" }],
      }),
    ).toBe("batched 2 proposals for review as rb-3");
    expect(line("review.batch_created")).toBe("started a review batch");
    expect(line("review.batch_submitted", { batch: "rb-3" })).toBe(
      "reviewed rb-3",
    );
    expect(line("review.batch_closed", { batch: "rb-3" })).toBe("closed rb-3");
    expect(line("area.defined", { area: "web", label: "ui" })).toBe(
      "defined web over label ui",
    );
    expect(line("area.defined")).toBe("defined an area");
    expect(line("area.updated", { area: "design" })).toBe(
      "changed the profile of design",
    );
    expect(line("area.updated")).toBe("changed the profile of an area");
    expect(line("workers.limits_changed")).toBe("changed the worker limits");
    expect(line("project.paused")).toBe("paused every agent of the project");
    expect(line("project.resumed")).toBe("resumed the agents of the project");
    expect(line("task.reported")).toBe(
      "reported T-12 finished for a lead check",
    );
    expect(line("task.sent_back")).toBe("sent T-12 back to its task worker");
    expect(line("task.hold_set")).toBe("put a blocker on T-12");
    expect(line("task.hold_cleared")).toBe("cleared the blocker of T-12");
    expect(
      line("worker.resumed", {
        agent: "web-worker",
        reason: "sent_back",
        resumed: true,
      }),
    ).toBe("resumed web-worker for the send-back of T-12");
    expect(
      line("worker.started", { agent: "web-worker", role: "task_worker" }),
    ).toBe("started web-worker as task worker");
    // A stored event keeps the role value it was written with.
    expect(
      line("worker.started", { agent: "web-coder", role: "task_coder" }),
    ).toBe("started web-coder as task worker");
    expect(
      line("worker.started", { agent: "web-lead", role: "area_lead" }),
    ).toBe("started web-lead as area lead");
    expect(line("worker.started", { agent: "web-worker", resumed: true })).toBe(
      "resumed web-worker",
    );
    expect(line("worker.started")).toBe("started a worker");
    expect(line("worker.stopped", { agent: "web-worker" })).toBe(
      "stopped web-worker",
    );
    expect(line("worker.failed", { agent: "web-worker" })).toBe(
      "web-worker failed",
    );
    expect(line("worker.refreshed", { agent: "web-lead", handoff: true })).toBe(
      "gave web-lead a fresh context with its hand-off note",
    );
    expect(
      line("worker.refreshed", { agent: "web-lead", handoff: false }),
    ).toBe("gave web-lead a fresh context");
    expect(
      line("worker.resumed", {
        agent: "web-worker",
        reason: "changes",
        resumed: true,
      }),
    ).toBe("resumed web-worker for changes to P-4");
    expect(
      line("worker.resumed", {
        agent: "web-worker",
        reason: "conflict",
        resumed: false,
      }),
    ).toBe("restarted web-worker in a new session for the conflict on P-4");
    expect(
      line("worker.resumed", {
        agent: "web-worker",
        reason: "checks",
        resumed: true,
      }),
    ).toBe("resumed web-worker for the failed integration checks of P-4");
    expect(
      line("worker.resumed", {
        agent: "web-worker",
        reason: "checks",
        stage: "proposal",
        resumed: true,
      }),
    ).toBe("resumed web-worker for the failed checks of T-12");
    expect(
      line("worker.resumed", {
        agent: "web-lead",
        reason: "session_lost",
        resumed: false,
      }),
    ).toBe("restarted web-lead in a new session");
    expect(
      line("worker.permission_asked", { agent: "web-worker", tool: "Bash" }),
    ).toBe("web-worker asked to use Bash");
    expect(line("worker.permission_asked")).toBe(
      "a worker asked to use a tool",
    );
    expect(
      line("worker.permission_answered", {
        agent: "web-worker",
        tool: "Bash",
        behavior: "deny",
      }),
    ).toBe("denied Bash for web-worker");
    expect(line("worker.permission_answered", { behavior: "allow" })).toBe(
      "allowed a tool for a worker",
    );
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
    const direct = { data: { kind: "direct", changeset: "C-40" } };
    expect(eventLine(event("integration.succeeded", direct)).text).toBe(
      "integrated direct change C-40 with the user's OK as r17",
    );
    expect(
      eventLine(
        event("integration.succeeded", {
          ...bare,
          data: { kind: "direct" },
        }),
      ).text,
    ).toBe("integrated a direct change with the user's OK");
    expect(
      eventLine(event("integration.succeeded", { data: { kind: "pull" } }))
        .text,
    ).toBe("integration succeeded as r17");
    expect(eventLine(event("main.advanced", bare)).text).toBe("main advanced");
    expect(eventLine(event("main.advanced")).text).toBe("main advanced to r17");
    expect(eventLine(event("git.commit_failed", bare)).text).toBe(
      "writing the Git commit failed",
    );
    expect(eventLine(event("git.commit_failed")).text).toBe(
      "writing the Git commit of r17 failed",
    );
    expect(eventLine(event("git.committed", bare)).text).toBe(
      "wrote a Git commit",
    );
    expect(eventLine(event("git.committed")).text).toBe(
      "wrote the Git commit of r17",
    );
    expect(eventLine(event("git.outside_commits")).text).toBe(
      "the Git branch has commits fold did not write",
    );
    expect(eventLine(event("git.uncommitted_changes")).text).toBe(
      "the project folder has changes that are not recorded; run `fld record`",
    );
    expect(
      eventLine(event("git.uncommitted_changes", { data: { staged: true } }))
        .text,
    ).toBe(
      "the Git index holds a staged change on a file main changed; run `git restore --staged` on it",
    );
    expect(
      eventLine(
        event("git.uncommitted_changes", {
          data: { staged: false, held: ["README.md", 7] },
        }),
      ).text,
    ).toBe(
      "the project folder waits on your changes to README.md that are not recorded; run `fld record`, or take them back",
    );
    expect(
      eventLine(
        event("git.uncommitted_changes", {
          data: { held: ["a.md", "b.md"], held_count: 52 },
        }),
      ).text,
    ).toBe(
      "the project folder waits on your changes to a.md, b.md and 50 more that are not recorded; run `fld record`, or take them back",
    );
    expect(
      eventLine(event("git.uncommitted_changes", { data: { held: "a.md" } }))
        .text,
    ).toBe(
      "the project folder has changes that are not recorded; run `fld record`",
    );
    const moved = eventLine(
      event("git.head_moved", {
        data: { hint: "run `git switch main` in the project folder" },
      }),
    ).text;
    expect(moved).toBe(
      "the Git branch changed in a way fold did not make; run `git switch main` in the project folder",
    );
    expect(codeParts(moved)).toContainEqual({ code: "git switch main" });
    expect(eventLine(event("git.head_moved", { data: {} })).text).toBe(
      "the Git branch changed in a way fold did not make; the project folder waits",
    );
    expect(
      eventLine(
        event("git.attention_cleared", { data: { kind: "head_moved" } }),
      ).text,
    ).toBe("the Git branch is back where fold left it");
    expect(
      eventLine(
        event("git.attention_cleared", { data: { kind: "outside_commits" } }),
      ).text,
    ).toBe("the Git branch has no commits fold did not write");
    expect(
      eventLine(
        event("git.attention_cleared", {
          data: { kind: "uncommitted_changes" },
        }),
      ).text,
    ).toBe("the project folder follows main again");
  });

  it("writes a line for each step of a human change", () => {
    expect(eventLine(event("human_change.recorded"))).toMatchObject({
      text: "recorded a change of the project folder as r17",
      emphasis: true,
    });
    expect(
      eventLine(event("human_change.recorded", { revision: null })).text,
    ).toBe("recorded a change of the project folder");
    expect(eventLine(event("human_change.conflicted")).text).toBe(
      "recorded a change of the project folder that conflicts with main; main did not move",
    );
    expect(eventLine(event("human_change.resolved")).text).toBe(
      "resolved the change of the project folder in r17",
    );
    expect(
      eventLine(event("human_change.resolved", { revision: null })).text,
    ).toBe("resolved the change of the project folder");
    expect(eventLine(event("human_change.withdrawn")).text).toBe(
      "took back the change of the project folder that waited for the main agent",
    );
  });

  it("writes a line for each step of a stash", () => {
    expect(
      eventLine(event("stash.pushed", { data: { file_count: 2 } })).text,
    ).toBe("stashed 2 files of the project folder");
    expect(eventLine(event("stash.pushed")).text).toBe(
      "stashed changes of the project folder",
    );
    expect(
      eventLine(event("stash.applied", { data: { file_count: 1 } })).text,
    ).toBe("unstashed 1 file into the project folder");
    expect(
      eventLine(
        event("stash.conflicted", {
          data: { conflicted: ["a.txt"], conflicted_count: 3 },
        }),
      ).text,
    ).toBe(
      "unstashed changes that conflict with the project folder in a.txt and 2 more; the entry stays",
    );
    expect(eventLine(event("stash.conflicted")).text).toBe(
      "unstashed changes that conflict with the project folder; the entry stays",
    );
    expect(eventLine(event("stash.dropped")).text).toBe(
      "fixed the conflict of the unstash; the stash entry is dropped",
    );
    expect(
      eventLine(event("stash.dropped", { data: { task: "T-9" } })).text,
    ).toBe("recorded the conflict of the unstash on main; T-9 fixes it");
  });

  it("keeps a backtick in a path from pairing with the marks of a command", () => {
    const line = eventLine(
      event("git.uncommitted_changes", {
        data: { held: ["a`b.md", "c\\d.md"] },
      }),
    );
    expect(codeParts(line.text)).toEqual([
      "the project folder waits on your changes to a`b.md, c\\d.md that are not recorded; run ",
      { code: "fld record" },
      ", or take them back",
    ]);
    const conflicted = eventLine(
      event("stash.conflicted", { data: { conflicted: ["x`y", "z`w"] } }),
    );
    expect(codeParts(conflicted.text)).toEqual([
      "unstashed changes that conflict with the project folder in x`y, z`w; the entry stays",
    ]);
  });
  it("names the task of a check run and how it ended", () => {
    expect(eventLine(event("checks.started")).text).toBe(
      "started the checks of T-12",
    );
    expect(
      eventLine(event("checks.finished", { data: { state: "failed" } })).text,
    ).toBe("ended the checks of T-12: failed");
    expect(eventLine(event("checks.finished", { task: null })).text).toBe(
      "ended the checks of a task",
    );
  });

  it("names the proposal of a queued integration and of its end", () => {
    expect(eventLine(event("integration.queued")).text).toBe(
      "queued the integration of P-4",
    );
    expect(eventLine(event("integration.unqueued")).text).toBe(
      "took the integration of P-4 out of the queue",
    );
    expect(eventLine(event("integration.held", { revision: "r7" })).text).toBe(
      "held the integrations: fold-checks.toml on r7 is not valid",
    );
    expect(eventLine(event("integration.held", { revision: null })).text).toBe(
      "held the integrations: fold-checks.toml on main is not valid",
    );
  });

  it("marks integrations, main moves, request approvals and recorded human changes as emphasised, review approvals by their verdict", () => {
    const emphasised = EVENT_TYPES.filter(
      (type) => eventLine(event(type)).emphasis,
    );
    expect(emphasised).toEqual([
      "proposal.integrated",
      "integration.succeeded",
      "main.advanced",
      "request.approved",
      "human_change.recorded",
    ]);
  });
});
