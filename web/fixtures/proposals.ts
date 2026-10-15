import type {
  ComparedFile,
  ComparedState,
  ComparisonFixture,
  FileDiff,
  Hunk,
  HunkLine,
} from "../src/api/diff";
import { ApiError } from "../src/api/errors";
import type {
  ConflictFile,
  Gate,
  IntegrateRequest,
  NewComment,
  NewReview,
  ProposalDetail,
  ProposalSummary,
  ProposalVersion,
  Review,
  ReviewComment,
} from "../src/api/proposals";

const NOW = Date.now();
const MIN = 60 * 1000;
const HUMAN = "tanim";

function hash(seed: string): string {
  return seed.repeat(64).slice(0, 64);
}

const ctx = (text: string): HunkLine => ({ kind: "context", text });
const add = (text: string): HunkLine => ({ kind: "add", text });
const del = (text: string): HunkLine => ({ kind: "remove", text });

function hunk(
  oldStart: number,
  newStart: number,
  section: string,
  lines: HunkLine[],
): Hunk {
  return {
    old_start: oldStart,
    old_lines: lines.filter((l) => l.kind !== "add").length,
    new_start: newStart,
    new_lines: lines.filter((l) => l.kind !== "remove").length,
    section,
    lines,
  };
}

interface Entry {
  file: ComparedFile;
  diff: FileDiff;
}

function text(
  path: string,
  change: ComparedFile["change"],
  hunks: Hunk[],
  seed: string,
): Entry {
  const lines = hunks.flatMap((h) => h.lines);
  return {
    file: {
      path,
      previous_path: null,
      change,
      added_lines: lines.filter((l) => l.kind === "add").length,
      removed_lines: lines.filter((l) => l.kind === "remove").length,
      binary: false,
      old_hash: change === "added" ? null : hash(`${seed}0`),
      new_hash: change === "deleted" ? null : hash(`${seed}1`),
    },
    diff: { path, binary: false, hunks },
  };
}

function binary(path: string, seed: string): Entry {
  return {
    file: {
      path,
      previous_path: null,
      change: "modified",
      added_lines: 0,
      removed_lines: 0,
      binary: true,
      old_hash: hash(`${seed}0`),
      new_hash: hash(`${seed}1`),
    },
    diff: { path, binary: true, hunks: [] },
  };
}

function revision(id: string): ComparedState {
  return { kind: "revision", id, base: null, task: null, worker: null };
}

function versionState(id: string, task: string): ComparedState {
  return { kind: "proposal_version", id, base: null, task, worker: null };
}

function comparison(
  from: ComparedState,
  to: ComparedState,
  entries: Entry[],
): ComparisonFixture {
  return {
    comparison: {
      from,
      to,
      read_at: null,
      files: entries.map((e) => e.file),
    },
    diffs: entries.map((e) => e.diff),
  };
}

const TABLE = [
  "CREATE TABLE reports (",
  "    id           INTEGER PRIMARY KEY,",
  "    owner_id     INTEGER NOT NULL REFERENCES users(id),",
  "    report_date  TEXT    NOT NULL,",
  "    kind         TEXT    NOT NULL CHECK (kind IN ('daily','monthly')),",
  "    total_cents  INTEGER NOT NULL,",
  "    created_at   INTEGER NOT NULL",
  ");",
  "",
];

const migrationV1 = text(
  "migrations/0004_reports.sql",
  "added",
  [
    hunk(0, 1, "", [
      ...TABLE.map(add),
      add("CREATE INDEX reports_kind_idx ON reports (kind);"),
    ]),
  ],
  "a",
);

const migrationV2 = text(
  "migrations/0004_reports.sql",
  "added",
  [
    hunk(0, 1, "", [
      ...TABLE.map(add),
      add("CREATE INDEX reports_owner_date_idx"),
      add("    ON reports (owner_id, report_date);"),
    ]),
  ],
  "b",
);

const reportsRs = text(
  "src/db/reports.rs",
  "added",
  [
    hunk(0, 1, "", [
      add("use sqlx::SqlitePool;"),
      add(""),
      add("pub struct Report {"),
      add("    pub id: i64,"),
      add("    pub owner_id: i64,"),
      add("    pub report_date: String,"),
      add("    pub kind: String,"),
      add("    pub total_cents: i64,"),
      add("}"),
      add(""),
      add(
        "pub async fn for_owner(pool: &SqlitePool, owner: i64) -> sqlx::Result<Vec<Report>> {",
      ),
      add(
        '    sqlx::query_as!(Report, "SELECT id, owner_id, report_date, kind, total_cents FROM reports WHERE owner_id = ? ORDER BY report_date", owner)',
      ),
      add("        .fetch_all(pool)"),
      add("        .await"),
      add("}"),
    ]),
  ],
  "c",
);

const dbMod = text(
  "src/db/mod.rs",
  "modified",
  [
    hunk(10, 10, "pub mod schema {", [
      ctx("pub mod schema {"),
      del("    pub mod users;"),
      del("    pub mod sessions;"),
      add("    pub mod reports;"),
      add("    pub mod sessions;"),
      add("    pub mod users;"),
      ctx("}"),
    ]),
  ],
  "d",
);

const migrationV1ToV2 = text(
  "migrations/0004_reports.sql",
  "modified",
  [
    hunk(8, 8, "", [
      ctx(");"),
      ctx(""),
      del("CREATE INDEX reports_kind_idx ON reports (kind);"),
      add("CREATE INDEX reports_owner_date_idx"),
      add("    ON reports (owner_id, report_date);"),
    ]),
  ],
  "e",
);

const tokensCss = text(
  "src/styles/tokens.css",
  "modified",
  [
    hunk(1, 1, ":root {", [
      ctx(":root {"),
      add("  --focus-ring: #4a7a73;"),
      add("  --scrim: #0a0a0b8c;"),
      ctx("  --bg: #fafaf8;"),
      ctx("  --surface: #ffffff;"),
      del("  --text: #111111;"),
      add("  --text: #1a1a1f;"),
    ]),
  ],
  "f",
);

const themeToggle = text(
  "src/app/ThemeToggle.tsx",
  "added",
  [
    hunk(0, 1, "", [
      add("export function ThemeToggle() {"),
      add("  const { theme, setTheme } = useTheme()"),
      add(
        "  return <button onClick={() => setTheme(next(theme))}>Theme</button>",
      ),
      add("}"),
    ]),
  ],
  "g",
);

const weekly = text(
  "src/charts/weekly.tsx",
  "modified",
  [
    hunk(18, 18, "export function WeeklyChart({ rows }: Props) {", [
      ctx("export function WeeklyChart({ rows }: Props) {"),
      del("  const totals = rows.map((r) => r.total)"),
      add("  const totals = weeklyTotals(rows)"),
      add("  const max = Math.max(...totals, 1)"),
      ctx("  return ("),
    ]),
  ],
  "h",
);

const legacy = text(
  "src/charts/legacy.ts",
  "modified",
  [
    hunk(3, 3, "", [
      ctx("export function legacyScale(value: number) {"),
      del("  return value / 100"),
      add("  return value / 1000"),
      ctx("}"),
    ]),
  ],
  "i",
);

const colors = text(
  "src/charts/colors.ts",
  "added",
  [
    hunk(0, 1, "", [
      add("export const SERIES = ['var(--label-sage)', 'var(--label-gold)']"),
    ]),
  ],
  "j",
);

const themeLink = text(
  "src/charts/theme.ts",
  "modified",
  [
    hunk(1, 1, "", [
      del("export * from '../styles/chart-theme'"),
      add("../styles/chart-theme.ts"),
    ]),
  ],
  "k",
);

const sprite = binary("public/chart-sprite.png", "l");

function single(path: string, line: string, seed: string): Entry {
  return text(path, "added", [hunk(0, 1, "", [add(line)])], seed);
}

/** Keyed by `P-n vN` for a version against its base, and by `from..to` for two versions. */
export const comparisons: Record<string, ComparisonFixture> = {
  "P-1 v1": comparison(revision("r4"), versionState("P-1 v1", "T-16"), [
    single("web/package.json", '{ "name": "web" }', "m"),
  ]),
  "P-2 v1": comparison(revision("r5"), versionState("P-2 v1", "T-15"), [
    single("api/Cargo.toml", '[package] name = "api"', "n"),
  ]),
  "P-3 v1": comparison(revision("r6"), versionState("P-3 v1", "T-11"), [
    tokensCss,
    themeToggle,
  ]),
  "P-4 v1": comparison(revision("r7"), versionState("P-4 v1", "T-3"), [
    migrationV1,
    reportsRs,
    dbMod,
  ]),
  "P-4 v1..P-4 v2": comparison(
    versionState("P-4 v1", "T-3"),
    versionState("P-4 v2", "T-3"),
    [migrationV1ToV2],
  ),
  "P-4 v2": comparison(revision("r7"), versionState("P-4 v2", "T-3"), [
    migrationV2,
    reportsRs,
    dbMod,
  ]),
  "P-5 v1": comparison(revision("r7"), versionState("P-5 v1", "T-13"), [
    single(
      "src/components/TextField.tsx",
      "export function TextField() {}",
      "o",
    ),
  ]),
  "P-6 v1": comparison(revision("r7"), versionState("P-6 v1", "T-12"), [
    weekly,
    sprite,
    legacy,
    colors,
    themeLink,
  ]),
  "P-7 v1": comparison(revision("r6"), versionState("P-7 v1", "T-14"), [
    single("api/src/error.rs", "pub struct ErrorBody;", "p"),
  ]),
  "P-8 v1": comparison(revision("r7"), versionState("P-8 v1", "T-9"), [
    single("api/src/limit.rs", "pub struct Limiter;", "q"),
  ]),
};

function stats(key: string) {
  const files = comparisons[key]?.comparison.files ?? [];
  return {
    files: files.length,
    added_lines: files.reduce((n, f) => n + f.added_lines, 0),
    removed_lines: files.reduce((n, f) => n + f.removed_lines, 0),
  };
}

function version(
  id: string,
  n: number,
  base: string,
  changeset: number,
  ago: number,
  evidence: string | null,
): ProposalVersion {
  return {
    version: n,
    base,
    tree: hash(`${id.slice(2)}${String(n)}`),
    changeset: `C-${String(changeset)}`,
    evidence,
    ...stats(`${id} v${String(n)}`),
    created_at: NOW - ago * MIN,
  };
}

function pastReview(
  n: number,
  reviewer: string,
  verdict: Review["verdict"],
  body: string,
  ago: number,
): Review {
  return { version: n, reviewer, verdict, body, created_at: NOW - ago * MIN };
}

function pastComment(
  id: number,
  n: number,
  author: string,
  body: string,
  ago: number,
  at: Partial<ReviewComment> = {},
): ReviewComment {
  return {
    id: `rc-${String(id)}`,
    version: n,
    author,
    path: null,
    side: null,
    start_line: null,
    end_line: null,
    reply_to: null,
    body,
    created_at: NOW - ago * MIN,
    ...at,
  };
}

function line(
  path: string,
  start: number,
  end: number,
  side: "old" | "new" = "new",
): Partial<ReviewComment> {
  return { path, side, start_line: start, end_line: end };
}

const NONE = {
  approve: false,
  request_changes: false,
  integrate: false,
  comment: false,
};

const ALL = {
  approve: true,
  request_changes: true,
  integrate: true,
  comment: true,
};

function integrator(): Gate {
  return { kind: "integrator", state: "met", agents: ["review"] };
}

function prerequisites(): Gate {
  return { kind: "prerequisites", state: "met", unfinished: [], total: 0 };
}

function cleanMerge(main: string, ago: number): Gate {
  return {
    kind: "merge",
    state: "met",
    main,
    conflicting_files: 0,
    checked_at: NOW - ago * MIN,
  };
}

function base(
  fields: Pick<ProposalDetail, "id" | "title" | "task" | "author"> &
    Partial<ProposalDetail>,
): ProposalDetail {
  return {
    state: "open",
    current_version: 1,
    with: null,
    waiting_on: null,
    latest_verdict: null,
    human_review: null,
    human_approved: false,
    review_claim: null,
    conflicted: false,
    integrated_revision: null,
    updated_at: NOW,
    main: "r8",
    close_reason: null,
    versions: [],
    reviews: [],
    comments: [],
    gates: [],
    conflict: null,
    integration: null,
    human_actions: NONE,
    ...fields,
  };
}

const conflictFiles: ConflictFile[] = [
  {
    path: "src/charts/weekly.tsx",
    kind: "both_modified_text",
    base: { kind: "file", hash: hash("h0") },
    main: { kind: "file", hash: hash("h2") },
    version: { kind: "file", hash: hash("h1") },
    hunks: [
      {
        base: ["  const totals = rows.map((r) => r.total)"],
        main: [
          "  const totals = rows.map((r) => r.total_cents / 100)",
          "  const limit = rateLimitFor(rows)",
        ],
        version: [
          "  const totals = weeklyTotals(rows)",
          "  const max = Math.max(...totals, 1)",
        ],
      },
    ],
  },
  {
    path: "public/chart-sprite.png",
    kind: "both_modified_binary",
    base: { kind: "file", hash: hash("l0") },
    main: { kind: "file", hash: hash("l2") },
    version: { kind: "file", hash: hash("l1") },
    hunks: [],
  },
  {
    path: "src/charts/legacy.ts",
    kind: "modify_delete",
    base: { kind: "file", hash: hash("i0") },
    main: null,
    version: { kind: "file", hash: hash("i1") },
    hunks: [],
  },
  {
    path: "src/charts/colors.ts",
    kind: "add_add",
    base: null,
    main: { kind: "file", hash: hash("j2") },
    version: { kind: "file", hash: hash("j1") },
    hunks: [],
  },
  {
    path: "src/charts/theme.ts",
    kind: "type_change",
    base: { kind: "file", hash: hash("k0") },
    main: { kind: "file", hash: hash("k2") },
    version: { kind: "symlink", hash: hash("k1") },
    hunks: [],
  },
];

function integrated(
  id: string,
  title: string,
  task: string,
  author: string,
  revisionId: string,
  parent: string,
  ago: number,
): ProposalDetail {
  return base({
    id,
    title,
    task,
    author,
    state: "integrated",
    latest_verdict: "approve",
    integrated_revision: revisionId,
    updated_at: NOW - ago * MIN,
    main: "r8",
    versions: [version(id, 1, parent, 10 + ago, ago + 20, null)],
    reviews: [pastReview(1, "review/quality", "approve", "", ago + 2)],
    gates: [
      {
        kind: "approval",
        state: "met",
        version: 1,
        approvers: ["review/quality"],
        latest_verdict: "approve",
      },
      integrator(),
      prerequisites(),
      cleanMerge(parent, ago),
    ],
    integration: {
      revision: revisionId,
      version: 1,
      actor: "review",
      at: NOW - ago * MIN,
    },
  });
}

function seed(): ProposalDetail[] {
  return [
    base({
      id: "P-4",
      title: "Database schema for reports",
      task: "T-3",
      author: "backend/db",
      current_version: 2,
      with: "reviewer",
      waiting_on: `human:${HUMAN}`,
      latest_verdict: "approve",
      human_review: {
        reviewer: HUMAN,
        marked_by: "backend",
        reason: "schema work for a junior task",
      },
      review_claim: "review/quality",
      updated_at: NOW - 2 * MIN,
      versions: [
        version(
          "P-4",
          1,
          "r7",
          17,
          30,
          "$ cargo nextest run -p db\n    46 passed, 0 failed",
        ),
        version(
          "P-4",
          2,
          "r7",
          18,
          12,
          "$ cargo nextest run -p db\n    48 passed, 0 failed\n$ fold migrate --check\n    0004_reports applied on empty db",
        ),
      ],
      reviews: [
        pastReview(1, "review/quality", "comment", "Naming matches T-14", 26),
        pastReview(
          1,
          "review/security",
          "request_changes",
          "Missing index",
          25,
        ),
        pastReview(2, "review/quality", "approve", "", 3),
      ],
      comments: [
        pastComment(
          1,
          1,
          "review/security",
          "Reports are always read by owner and date. Add an index on (owner_id, report_date).",
          25,
          line("migrations/0004_reports.sql", 10, 10),
        ),
        pastComment(
          2,
          2,
          "backend/db",
          "Added reports_owner_date_idx in v2.",
          12,
          {
            ...line("migrations/0004_reports.sql", 10, 10),
            reply_to: "rc-1",
          },
        ),
        pastComment(
          3,
          2,
          "review/quality",
          "for_owner reads every report of an owner; page it once T-4 lands.",
          4,
          line("src/db/reports.rs", 11, 14),
        ),
      ],
      gates: [
        {
          kind: "approval",
          state: "met",
          version: 2,
          approvers: ["review/quality"],
          latest_verdict: "approve",
        },
        {
          kind: "human_review",
          state: "waiting_human",
          reviewer: HUMAN,
          marked_by: "backend",
          reason: "schema work for a junior task",
        },
        integrator(),
        prerequisites(),
        cleanMerge("r8", 2),
      ],
      human_actions: ALL,
    }),
    base({
      id: "P-3",
      title: "Design tokens and the theme switch",
      task: "T-11",
      author: "frontend/styles",
      with: "author",
      waiting_on: "frontend/styles",
      latest_verdict: "request_changes",
      updated_at: NOW - 9 * MIN,
      versions: [
        version("P-3", 1, "r6", 15, 40, "$ bun run test\n    212 passed"),
      ],
      reviews: [
        pastReview(
          1,
          "review/quality",
          "request_changes",
          "The dark theme has no focus ring token",
          9,
        ),
      ],
      comments: [
        pastComment(
          4,
          1,
          "review/quality",
          "Add --focus-ring to the dark block too.",
          9,
          line("src/styles/tokens.css", 2, 3),
        ),
      ],
      gates: [
        {
          kind: "approval",
          state: "unmet",
          version: 1,
          approvers: [],
          latest_verdict: "request_changes",
        },
        integrator(),
        prerequisites(),
        cleanMerge("r8", 9),
      ],
      human_actions: ALL,
    }),
    base({
      id: "P-6",
      title: "Chart the weekly totals",
      task: "T-12",
      author: "frontend/charts",
      with: "author",
      waiting_on: "frontend/charts",
      latest_verdict: "approve",
      review_claim: "review/quality",
      conflicted: true,
      updated_at: NOW - MIN,
      versions: [
        version("P-6", 1, "r7", 19, 20, "$ bun run test charts\n    18 passed"),
      ],
      reviews: [pastReview(1, "review/quality", "approve", "", 5)],
      gates: [
        {
          kind: "approval",
          state: "met",
          version: 1,
          approvers: ["review/quality"],
          latest_verdict: "approve",
        },
        integrator(),
        prerequisites(),
        {
          kind: "merge",
          state: "unmet",
          main: "r8",
          conflicting_files: conflictFiles.length,
          checked_at: NOW - MIN,
        },
      ],
      conflict: {
        version: 1,
        base: "r7",
        main: "r8",
        trees: { base: hash("70"), main: hash("80"), version: hash("61") },
        files: conflictFiles,
      },
      human_actions: { ...ALL, integrate: false },
    }),
    base({
      id: "P-8",
      title: "Rate-limit the login endpoint",
      task: "T-9",
      author: "backend/api",
      state: "closed",
      close_reason: "Split into a smaller change after T-1",
      updated_at: NOW - 70 * MIN,
      versions: [version("P-8", 1, "r7", 16, 90, null)],
      gates: [
        {
          kind: "approval",
          state: "unmet",
          version: 1,
          approvers: [],
          latest_verdict: null,
        },
        integrator(),
        prerequisites(),
        cleanMerge("r7", 80),
      ],
    }),
    integrated(
      "P-5",
      "Form field components",
      "T-13",
      "frontend/ui",
      "r8",
      "r7",
      35,
    ),
    integrated(
      "P-7",
      "Error envelope and health route",
      "T-14",
      "backend/api",
      "r7",
      "r6",
      110,
    ),
    integrated(
      "P-2",
      "Scaffold the API service",
      "T-15",
      "backend",
      "r6",
      "r5",
      300,
    ),
    integrated(
      "P-1",
      "Scaffold the web app",
      "T-16",
      "frontend",
      "r5",
      "r4",
      420,
    ),
  ];
}

let store = seed();
let nextComment = 10;

/** The `demo` project has proposals; every other project has none. */
function project(slug: string): ProposalDetail[] {
  return slug === "demo" ? store : [];
}

function summary(p: ProposalDetail): ProposalSummary {
  return {
    id: p.id,
    title: p.title,
    task: p.task,
    author: p.author,
    state: p.state,
    current_version: p.current_version,
    with: p.with,
    waiting_on: p.waiting_on,
    latest_verdict: p.latest_verdict,
    human_review: p.human_review,
    human_approved: p.human_approved,
    review_claim: p.review_claim,
    conflicted: p.conflicted,
    integrated_revision: p.integrated_revision,
    updated_at: p.updated_at,
  };
}

export function list(slug: string): ProposalSummary[] {
  return project(slug).map(summary);
}

export function detail(slug: string, id: string): ProposalDetail | null {
  return project(slug).find((p) => p.id === id) ?? null;
}

/** Puts the seed data back, so tests that write start from the same proposals. */
export function reset(): void {
  store = seed();
  nextComment = 10;
}

function writable(slug: string, id: string, version: number): ProposalDetail {
  const found = detail(slug, id);
  if (!found) {
    throw new ApiError("proposal_not_found", `no proposal is ${id}`);
  }
  if (found.state !== "open") {
    throw new ApiError("proposal_not_open", `${id} is ${found.state}`);
  }
  if (version !== found.current_version) {
    throw new ApiError(
      "version_superseded",
      `${id} v${String(version)} is not the current version`,
    );
  }
  return found;
}

/** Marks the approval and human-review gates as the daemon would after a human approval of the current version. */
function humanApproved(p: ProposalDetail): void {
  p.latest_verdict = "approve";
  p.human_approved = true;
  p.with = "reviewer";
  p.waiting_on = p.review_claim;
  p.gates = p.gates.map((gate) => {
    if (gate.kind === "approval") {
      return {
        ...gate,
        state: "met",
        latest_verdict: "approve",
        approvers: [...gate.approvers, `human:${HUMAN}`],
      };
    }
    return gate.kind === "human_review" ? { ...gate, state: "met" } : gate;
  });
}

/** Stands in for the daemon: a human verdict on the current version. */
export function review(
  slug: string,
  id: string,
  input: NewReview,
): ProposalDetail {
  const p = writable(slug, id, input.version);
  p.reviews.push({
    version: input.version,
    reviewer: `human:${HUMAN}`,
    verdict: input.verdict,
    body: input.body,
    created_at: Date.now(),
  });
  p.updated_at = Date.now();
  if (input.verdict === "approve") {
    humanApproved(p);
    return p;
  }
  p.latest_verdict = "request_changes";
  p.with = "author";
  p.waiting_on = p.author;
  p.gates = p.gates.map((gate) =>
    gate.kind === "approval"
      ? { ...gate, state: "unmet", latest_verdict: "request_changes" }
      : gate,
  );
  return p;
}

export function comment(
  slug: string,
  id: string,
  input: NewComment,
): ProposalDetail {
  const p = writable(slug, id, input.version);
  const now = Date.now();
  p.reviews.push({
    version: input.version,
    reviewer: `human:${HUMAN}`,
    verdict: "comment",
    body: "",
    created_at: now,
  });
  p.comments.push({
    id: `rc-${String(nextComment)}`,
    version: input.version,
    author: `human:${HUMAN}`,
    path: input.path,
    side: input.side,
    start_line: input.start_line,
    end_line: input.end_line,
    reply_to: null,
    body: input.body,
    created_at: now,
  });
  nextComment += 1;
  p.updated_at = now;
  return p;
}

/** The human approves and integrates in one action; main moves to the next revision. */
export function integrate(
  slug: string,
  id: string,
  input: IntegrateRequest,
): ProposalDetail {
  const p = writable(slug, id, input.version);
  if (!p.human_actions.integrate) {
    throw new ApiError("gate_unmet", `${id} cannot be integrated now`);
  }
  const now = Date.now();
  p.reviews.push({
    version: input.version,
    reviewer: `human:${HUMAN}`,
    verdict: "approve",
    body: "",
    created_at: now,
  });
  humanApproved(p);
  const next = `r${String(Number(p.main.slice(1)) + 1)}`;
  for (const other of store) other.main = next;
  p.state = "integrated";
  p.with = null;
  p.waiting_on = null;
  p.integrated_revision = next;
  p.integration = {
    revision: next,
    version: input.version,
    actor: `human:${HUMAN}`,
    at: now,
  };
  p.human_actions = NONE;
  p.updated_at = now;
  return p;
}
