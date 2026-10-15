import type {
  ConflictReport,
  Gate,
  ProposalDetail,
  ProposalSummary,
  ProposalVersion,
  Review,
  ReviewComment,
} from "@/api/proposals";
import type { Revision } from "@/api/repository";

const T0 = 1_760_000_000_000;

/** A proposal row as the list route sends it: open, with the reviewer, nothing else set; a human review mark starts unmet. */
export function proposalSummary(
  id: string,
  fields: Partial<ProposalSummary> = {},
): ProposalSummary {
  return {
    id,
    task: "T-3",
    title: `Proposal ${id}`,
    author: "backend/db",
    state: "open",
    conflicted: false,
    current_version: 1,
    turn: "with_reviewer",
    latest_verdict: null,
    human_approved: !fields.human_review,
    review_claim: null,
    human_review: null,
    close_reason: null,
    created_at: T0,
    updated_at: T0,
    closed_at: null,
    ...fields,
  };
}

export function proposalVersion(
  version: number,
  fields: Partial<ProposalVersion> = {},
): ProposalVersion {
  return {
    version,
    base: "r7",
    changeset: `C-${String(10 + version)}`,
    tree: String(version).repeat(64),
    files_added: 1,
    files_modified: 1,
    files_deleted: 0,
    evidence: null,
    created_by: "backend/db",
    created_at: T0 + version * 60_000,
    ...fields,
  };
}

export function review(
  version: number,
  reviewer: string,
  verdict: Review["verdict"],
  fields: Partial<Review> = {},
): Review {
  return {
    version,
    reviewer,
    verdict,
    body: "",
    comments: 0,
    created_at: T0 + version * 60_000 + 30_000,
    ...fields,
  };
}

/** The gates the daemon reports to a human on an open proposal with nothing in the way. */
export function humanGates(
  overrides: Partial<Record<Gate["gate"], Partial<Gate>>> = {},
): Gate[] {
  const names: Gate["gate"][] = [
    "open",
    "version",
    "caller",
    "approval",
    "human_review",
    "prerequisites",
  ];
  return [
    ...names.map((gate) => ({
      gate,
      status: "met" as const,
      code: null,
      ...overrides[gate],
    })),
    {
      gate: "merge",
      status: "checked_at_integration",
      code: null,
      ...overrides.merge,
    },
  ];
}

/** A proposal as the detail route answers it; `versions` defaults to v1 up to the summary's current version. */
export function proposalDetail(
  summary: ProposalSummary,
  fields: Partial<ProposalDetail> = {},
): ProposalDetail {
  const versions =
    fields.versions ??
    Array.from({ length: summary.current_version }, (_, i) =>
      proposalVersion(i + 1),
    );
  const selected =
    versions.find((v) => v.version === summary.current_version) ??
    proposalVersion(summary.current_version);
  return {
    summary,
    task_status: summary.state === "integrated" ? "done" : "review",
    task: {
      task: summary.task,
      title: summary.title,
      description: "",
      status: "review",
    },
    prerequisites: [],
    versions,
    selected,
    review_claim: null,
    reviews: [],
    comments: { total: 0, on_version: 0, outdated: 0 },
    gates: humanGates(),
    conflict: null,
    files: [],
    totals: { files: 2, added_lines: 10, removed_lines: 2 },
    ...fields,
  };
}

/** A line comment as the comments route sends it, written by a review worker. */
export function reviewComment(
  id: number,
  version: number,
  fields: Partial<ReviewComment> = {},
): ReviewComment {
  return {
    id: `rc-${String(id)}`,
    version,
    verdict: "comment",
    author: "review/quality",
    path: "a.sql",
    side: "new",
    start_line: 2,
    end_line: 4,
    reply_to: null,
    body: `comment ${String(id)}`,
    created_at: T0 + id * 1_000,
    outdated: false,
    batch_comment: null,
    ...fields,
  };
}

/** A conflict report with one file of every kind. */
export function conflictReport(
  proposal: string,
  version: number,
): ConflictReport {
  const entry = (kind: "file" | "symlink", seed: string) => ({
    kind,
    hash: seed.repeat(64).slice(0, 64),
  });
  return {
    proposal,
    version,
    base_revision: "r7",
    main_revision: "r8",
    trees: {
      base: "a".repeat(64),
      main: "b".repeat(64),
      version: "c".repeat(64),
    },
    files: [
      {
        path: "src/chart.ts",
        kind: "text",
        base: entry("file", "1"),
        main: entry("file", "2"),
        version: entry("file", "3"),
        hunks: [
          {
            base: { start: 4, count: 1, lines: ["const max = 1;\n"] },
            main: {
              start: 4,
              count: 1,
              lines: ["const max = Math.max(...totals, 1);\n"],
            },
            version: { start: 4, count: 0, lines: [] },
          },
        ],
      },
      {
        path: "assets/logo.png",
        kind: "both_modified_binary",
        base: entry("file", "4"),
        main: entry("file", "5"),
        version: entry("file", "6"),
        hunks: [],
      },
      {
        path: "src/old.ts",
        kind: "modify_delete",
        base: entry("file", "7"),
        main: null,
        version: entry("file", "8"),
        hunks: [],
      },
      {
        path: "src/new.ts",
        kind: "add_add",
        base: null,
        main: entry("file", "9"),
        version: entry("file", "a"),
        hunks: [],
      },
      {
        path: "link",
        kind: "type_change",
        base: entry("file", "b"),
        main: entry("symlink", "c"),
        version: entry("file", "d"),
        hunks: [],
      },
    ],
  };
}

/** A main revision; `proposal` names the proposal it integrated. */
export function revision(n: number, proposal: string | null = null): Revision {
  return {
    id: `r${String(n)}`,
    parent: n > 1 ? `r${String(n - 1)}` : null,
    origin: proposal === null ? "initial" : "proposal",
    proposal,
    task: proposal === null ? null : "T-3",
    title: proposal === null ? null : `Proposal ${proposal}`,
    author: proposal === null ? null : "backend",
    pull: null,
    commit: null,
    tree: String(n).repeat(64).slice(0, 64),
    created_at: T0 + n * 3_600_000,
  };
}
