import type {
  CheckRun,
  ProposalDetail,
  ProposalVersion,
  Review,
  Verdict,
} from "@/api/proposals";
import { listedReviews, reviewScopeText, versionVerdict } from "./present";
import type { DetailSearch } from "./view";

/** The lines of a review the history shows before "Read the full review". */
export const EXCERPT_LINES = 3;
export const EXCERPT_CHARS = 280;

export interface Excerpt {
  text: string;
  /** The full text holds more than the excerpt. */
  cut: boolean;
}

/** The first non-empty lines of a text, cut at a word near `chars`. */
export function excerpt(
  body: string,
  lines = EXCERPT_LINES,
  chars = EXCERPT_CHARS,
): Excerpt {
  const all = body
    .trim()
    .split("\n")
    .map((line) => line.trimEnd())
    .filter((line) => line.trim() !== "");
  const kept = all.slice(0, lines).join("\n");
  if (kept.length <= chars) {
    return { text: kept, cut: all.length > lines };
  }
  const space = kept.lastIndexOf(" ", chars);
  return {
    text: `${kept.slice(0, space > chars / 2 ? space : chars).trimEnd()}…`,
    cut: true,
  };
}

const HEADS: Record<Verdict, string> = {
  approve: "Approved",
  request_changes: "Changes requested",
  reject: "Rejected",
  comment: "Comment",
};

export interface HistoryReview {
  review: Review;
  /** `Approved`, or `Approved (kept from v2)` for an approval a human edit kept. */
  head: string;
  excerpt: Excerpt;
  scope: string | null;
}

/** How a version came to be: by the author, as a reviewer's fix, or as a human's edit in a human review. */
export type VersionOrigin = "author" | "review_fix" | "human_edit";

export interface HistoryEntry {
  version: ProposalVersion;
  current: boolean;
  /** The latest verdict on the version, such as `approved`. */
  verdict: string;
  origin: VersionOrigin;
  /** The base a version moved to when the author merged main into it; null for the first version and for a version on the same base as the one before it. */
  mergedMain: string | null;
  /** The change the version's link opens: since the version before it, or the full change of the first. */
  diff: { label: string; search: DetailSearch };
  /** The version's check runs, newest first; null until they arrive. */
  runs: readonly CheckRun[] | null;
  /** Its reviews, newest first. */
  reviews: HistoryReview[];
}

function originOf(v: ProposalVersion): VersionOrigin {
  if (v.reviewer_fix) return "review_fix";
  if (v.human_edit) return "human_edit";
  return "author";
}

function historyReview(review: Review): HistoryReview {
  const head =
    review.kept_from === null
      ? HEADS[review.verdict]
      : `${HEADS[review.verdict]} (kept from v${String(review.kept_from)})`;
  return {
    review,
    head,
    excerpt: excerpt(review.body),
    scope: reviewScopeText(review),
  };
}

/** One card per version, newest first, each with what made it, its check runs and its reviews. */
export function historyEntries(
  p: ProposalDetail,
  runs: ReadonlyMap<number, readonly CheckRun[]>,
): HistoryEntry[] {
  const versions = [...p.versions].sort((a, b) => a.version - b.version);
  const reviews = listedReviews(p.reviews);
  const current = p.summary.current_version;
  return versions
    .map((v, index): HistoryEntry => {
      const before = versions[index - 1];
      const here = v.version === current ? {} : { version: v.version };
      return {
        version: v,
        current: v.version === current,
        verdict: versionVerdict(v.version, p.reviews),
        origin: originOf(v),
        mergedMain: before && before.base !== v.base ? v.base : null,
        diff: before
          ? {
              label: `Changes since v${String(before.version)}`,
              search: { tab: "changes", ...here, since: before.version },
            }
          : { label: "Full change", search: { tab: "changes", ...here } },
        runs: runs.get(v.version) ?? null,
        reviews: reviews
          .filter((r) => r.version === v.version)
          .map(historyReview),
      };
    })
    .reverse();
}

function count(run: CheckRun, outcome: CheckRun["results"][number]["outcome"]) {
  return run.results.filter((r) => r.outcome === outcome).length;
}

const NAMES_SHOWN = 6;

function names(run: CheckRun): string {
  const all = run.results.map((r) => r.name);
  const shown = all.slice(0, NAMES_SHOWN).join(", ");
  const more = all.length - NAMES_SHOWN;
  return more > 0 ? `${shown} and ${String(more)} more` : shown;
}

/** One run in a line: `5 of 5 passed · setup, backend, web`, or `web failed (exit 2) · 2 passed, 2 not run`. */
export function runSummary(run: CheckRun): string {
  const total = run.results.length;
  const passed = count(run, "passed");
  if (run.state === "running") {
    return `running · ${String(passed)} of ${String(total)} passed so far`;
  }
  if (run.state === "passed") {
    const skipped = count(run, "skipped");
    const head = `${String(passed)} of ${String(total)} passed`;
    const tail = skipped > 0 ? ` · ${String(skipped)} skipped` : "";
    return total === 0 ? "passed" : `${head}${tail} · ${names(run)}`;
  }
  const failed = run.results.find(
    (r) => r.outcome === "failed" || r.outcome === "timed_out",
  );
  const rest = [
    passed > 0 ? `${String(passed)} passed` : null,
    count(run, "not_run") > 0
      ? `${String(count(run, "not_run"))} not run`
      : null,
  ].filter((part): part is string => part !== null);
  if (!failed) {
    return [run.state, ...rest].join(" · ");
  }
  const how =
    failed.outcome === "timed_out"
      ? "timed out"
      : failed.exit_code === null
        ? "failed"
        : `failed (exit ${String(failed.exit_code)})`;
  return [`${failed.name} ${how}`, rest.join(", ")]
    .filter((part) => part !== "")
    .join(" · ");
}
