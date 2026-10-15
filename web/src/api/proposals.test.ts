import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { comparisons, reset } from "../../fixtures/proposals";
import { isApiError } from "./errors";
import { keys } from "./keys";
import {
  addComment,
  getProposal,
  getVersionComparison,
  getVersionFileDiff,
  integrateProposal,
  listProposals,
  pairKey,
  submitReview,
} from "./proposals";
import {
  proposalQuery,
  proposalsQuery,
  versionComparisonQuery,
  versionFileDiffQuery,
} from "./queries";

beforeEach(() => {
  vi.useFakeTimers();
  reset();
});
afterEach(() => {
  vi.useRealTimers();
});

async function settle<T>(promise: Promise<T>): Promise<T> {
  const settled = promise.then(
    (value) => ({ ok: true as const, value }),
    (error: unknown) => ({ ok: false as const, error }),
  );
  await vi.runAllTimersAsync();
  const result = await settled;
  if (!result.ok) throw result.error;
  return result.value;
}

describe("the proposal fixture", () => {
  it("lists open, integrated and closed proposals for demo and none elsewhere", async () => {
    const list = await settle(listProposals("demo"));
    expect(new Set(list.map((p) => p.state))).toEqual(
      new Set(["open", "integrated", "closed"]),
    );
    expect(await settle(listProposals("empty"))).toEqual([]);
  });

  it("has a comparison for every version", async () => {
    for (const summary of await settle(listProposals("demo"))) {
      const p = await settle(getProposal("demo", summary.id));
      for (const v of p?.versions ?? []) {
        expect(
          comparisons[`${summary.id} v${String(v.version)}`],
          `${summary.id} v${String(v.version)}`,
        ).toBeDefined();
      }
    }
  });

  it("holds a conflicted proposal with every conflict kind", async () => {
    const p = await settle(getProposal("demo", "P-6"));
    expect(p?.conflict?.files.map((f) => f.kind).sort()).toEqual([
      "add_add",
      "both_modified_binary",
      "both_modified_text",
      "modify_delete",
      "type_change",
    ]);
  });

  it("answers null for a proposal the project does not have", async () => {
    expect(await settle(getProposal("demo", "P-99"))).toBeNull();
  });
});

describe("version comparisons", () => {
  it("key a version alone and a pair of versions", () => {
    expect(pairKey(undefined, "P-4 v2")).toBe("P-4 v2");
    expect(pairKey("P-4 v1", "P-4 v2")).toBe("P-4 v1..P-4 v2");
  });

  it("have a diff with the generated shapes for every file, whose line counts match the hunks", () => {
    for (const entry of Object.values(comparisons)) {
      for (const file of entry.comparison.files) {
        const diff = entry.diffs.find((d) => d.path === file.path);
        expect(diff, file.path).toBeDefined();
        const lines = diff?.hunks.flatMap((h) => h.lines) ?? [];
        expect(lines.filter((l) => l.kind === "added")).toHaveLength(
          file.added_lines,
        );
        expect(lines.filter((l) => l.kind === "removed")).toHaveLength(
          file.removed_lines,
        );
        expect(lines.every((l) => l.text.endsWith("\n"))).toBe(true);
        expect(diff?.binary).toBe(file.binary);
      }
    }
  });

  it("answer a version's comparison and one file's hunks", async () => {
    const comparison = await settle(
      getVersionComparison("demo", undefined, "P-4 v2"),
    );
    expect(comparison?.to).toBe("P-4 v2");
    const path = comparison?.files[0]?.path ?? "";
    const diff = await settle(
      getVersionFileDiff("demo", undefined, "P-4 v2", path),
    );
    expect(diff.path).toBe(path);
    expect(
      await settle(getVersionComparison("other", undefined, "P-4 v2")),
    ).toBeNull();
  });

  it("reject a path the comparison does not list", async () => {
    const missing = getVersionFileDiff("demo", undefined, "P-4 v2", "nope");
    const check = expect(missing).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "path_not_in_comparison"),
    );
    await vi.runAllTimersAsync();
    await check;
  });

  it("are keyed apart from the diff route's comparisons", async () => {
    const client = new QueryClient();
    expect(
      versionComparisonQuery("demo", undefined, "P-4 v2").queryKey,
    ).toEqual(keys.versionComparison("demo", undefined, "P-4 v2"));
    expect(
      versionFileDiffQuery("demo", "P-4 v1", "P-4 v2", "a").queryKey,
    ).toEqual(["demo", "version-file-diff", "P-4 v1", "P-4 v2", "a"]);
    const loaded = client.query(
      versionComparisonQuery("demo", "P-4 v1", "P-4 v2"),
    );
    await vi.runAllTimersAsync();
    expect((await loaded)?.files.length).toBeGreaterThan(0);
  });
});

describe("human writes", () => {
  it("records an approval on the current version and meets the human review", async () => {
    const p = await settle(
      submitReview("demo", "P-4", { version: 2, verdict: "approve", body: "" }),
    );
    expect(p.reviews.at(-1)).toMatchObject({
      version: 2,
      reviewer: "human:tanim",
      verdict: "approve",
    });
    expect(p.human_approved).toBe(true);
    expect(p.gates.find((g) => g.kind === "human_review")?.state).toBe("met");
  });

  it("returns the proposal to the author on a request for changes", async () => {
    const p = await settle(
      submitReview("demo", "P-4", {
        version: 2,
        verdict: "request_changes",
        body: "Rename the index",
      }),
    );
    expect(p.with).toBe("author");
    expect(p.waiting_on).toBe("backend/db");
    expect(p.gates.find((g) => g.kind === "approval")?.state).toBe("unmet");
  });

  it("stores a line comment with its range on the version it was written on", async () => {
    const p = await settle(
      addComment("demo", "P-4", {
        version: 2,
        path: "src/db/mod.rs",
        side: "old",
        start_line: 11,
        end_line: 12,
        body: "Why?",
      }),
    );
    expect(p.comments.at(-1)).toMatchObject({
      version: 2,
      path: "src/db/mod.rs",
      side: "old",
      start_line: 11,
      end_line: 12,
      author: "human:tanim",
    });
    expect(p.reviews.at(-1)?.verdict).toBe("comment");
  });

  it("approves and integrates in one action and moves main", async () => {
    const p = await settle(integrateProposal("demo", "P-3", { version: 1 }));
    expect(p.state).toBe("integrated");
    expect(p.integration).toMatchObject({
      revision: "r9",
      version: 1,
      actor: "human:tanim",
    });
    expect(p.reviews.at(-1)?.verdict).toBe("approve");
    expect((await settle(getProposal("demo", "P-4")))?.main).toBe("r9");
  });

  it("refuses a write on an old version, a closed proposal, an unknown one, or an integration the gates refuse", async () => {
    const cases: [() => Promise<unknown>, string][] = [
      [
        () =>
          submitReview("demo", "P-4", {
            version: 1,
            verdict: "approve",
            body: "",
          }),
        "version_superseded",
      ],
      [
        () =>
          submitReview("demo", "P-8", {
            version: 1,
            verdict: "approve",
            body: "",
          }),
        "proposal_not_open",
      ],
      [
        () =>
          submitReview("demo", "P-99", {
            version: 1,
            verdict: "approve",
            body: "",
          }),
        "proposal_not_found",
      ],
      [() => integrateProposal("demo", "P-6", { version: 1 }), "gate_unmet"],
    ];
    for (const [write, code] of cases) {
      await expect(settle(write())).rejects.toSatisfy((e: unknown) =>
        isApiError(e, code),
      );
    }
  });
});

describe("proposal queries", () => {
  it("key the list and the detail under the project", () => {
    const client = new QueryClient();
    expect(proposalsQuery("demo").queryKey).toEqual(keys.proposals("demo"));
    expect(proposalQuery("demo", "P-4").queryKey).toEqual(
      keys.proposal("demo", "P-4"),
    );
    client.clear();
  });

  it("read through the fixture", async () => {
    const client = new QueryClient();
    const list = client.query(proposalsQuery("demo"));
    const one = client.query(proposalQuery("demo", "P-4"));
    await vi.runAllTimersAsync();
    expect((await list).length).toBeGreaterThan(0);
    expect((await one)?.id).toBe("P-4");
  });
});

describe("a production build", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it("lists nothing and refuses writes with the daemon's not_found", async () => {
    vi.stubEnv("DEV", false);
    vi.resetModules();
    const proposals = await import("./proposals");
    const errors = await import("./errors");
    await expect(proposals.listProposals("demo")).resolves.toEqual([]);
    await expect(proposals.getProposal("demo", "P-4")).resolves.toBeNull();
    await expect(
      proposals.getVersionComparison("demo", undefined, "P-4 v2"),
    ).resolves.toBeNull();
    const notFound = (e: unknown) => errors.isApiError(e, "not_found");
    await expect(
      proposals.submitReview("demo", "P-4", {
        version: 2,
        verdict: "approve",
        body: "",
      }),
    ).rejects.toSatisfy(notFound);
    await expect(
      proposals.addComment("demo", "P-4", {
        version: 2,
        path: "a",
        side: "new",
        start_line: 1,
        end_line: 1,
        body: "x",
      }),
    ).rejects.toSatisfy(notFound);
    await expect(
      proposals.integrateProposal("demo", "P-4", { version: 2 }),
    ).rejects.toSatisfy(notFound);
  });
});
