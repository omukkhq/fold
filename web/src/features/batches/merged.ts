import type { ReviewBatchDiff } from "@/api/reviews";
import { projectPath, withQuery } from "@/routes/paths";

/** The two tabs of the batch screen; Members is the default. */
export type BatchTab = "members" | "changes";

/** The batch screen's address: the tab and the file of the merged change on screen. */
export interface BatchSearch {
  /** Absent, the tab is Merged changes for an address that names a file, and Members otherwise. */
  tab?: BatchTab;
  path?: string;
}

export function isBatchTab(value: unknown): value is BatchTab {
  return value === "members" || value === "changes";
}

export function parseBatchSearch(search: Record<string, unknown>): BatchSearch {
  const path = typeof search.path === "string" ? search.path.trim() : "";
  return {
    ...(isBatchTab(search.tab) ? { tab: search.tab } : {}),
    ...(path === "" ? {} : { path }),
  };
}

export function batchTab(search: BatchSearch): BatchTab {
  return search.tab ?? (search.path === undefined ? "members" : "changes");
}

/** The address of a tab: the file stays only on the Merged changes tab. */
export function batchTabSearch(
  search: BatchSearch,
  tab: BatchTab,
): BatchSearch {
  if (tab === "members") return {};
  return search.path === undefined ? { tab } : { tab, path: search.path };
}

export function batchHref(
  slug: string,
  id: string,
  search: BatchSearch,
): string {
  const params = new URLSearchParams();
  if (search.tab !== undefined) params.set("tab", search.tab);
  if (search.path !== undefined) params.set("path", search.path);
  return withQuery(projectPath(slug, "batches", id), params);
}

function memberList(diff: ReviewBatchDiff): string {
  return diff.members
    .map((m) => `${m.proposal} v${String(m.version)}`)
    .join(" + ");
}

/** What the two sides of the merged change are, such as `main r17 + P-3 v2 + P-4 v1, merged in order`; null when the change compares nothing. */
export function mergedLine(diff: ReviewBatchDiff): string | null {
  const base = diff.base ?? "";
  if (diff.basis === "merged") {
    return diff.members.length === 0
      ? `main ${base}, before any member`
      : `main ${base} + ${memberList(diff)}, merged in order`;
  }
  if (diff.basis === "integrated") {
    return `main ${base} → ${diff.head ?? ""}: ${memberList(diff)}, integrated together`;
  }
  return null;
}

/** The empty state of a merged change that has no file to show. */
export interface MergedEmpty {
  title: string;
  text: string;
}

export function mergedEmpty(diff: ReviewBatchDiff): MergedEmpty | null {
  switch (diff.basis) {
    case "no_members":
      return {
        title: "No members to merge",
        text: "Every member left the batch, or its proposal is no longer open.",
      };
    case "closed":
      return {
        title: "No merged change",
        text: "Every member left the batch without a batch review.",
      };
    case "not_together":
      return {
        title: "Not integrated together",
        text: "The batch's integration published no members together. Each member's page shows its own change.",
      };
    case "merged":
    case "integrated":
      if (diff.files.length > 0) return null;
      if (diff.conflict !== null && diff.members.length === 0) {
        return {
          title: "No member merges",
          text: `${diff.conflict.proposal} is the first member, so no change merges before it.`,
        };
      }
      return {
        title: "No changes",
        text: "Together, the members change no file on main.",
      };
  }
}

/** The banner of the first member that does not merge. */
export interface ConflictNote {
  title: string;
  text: string;
}

export function conflictNote(diff: ReviewBatchDiff): ConflictNote | null {
  const conflict = diff.conflict;
  if (conflict === null) return null;
  const other = conflict.with ?? "main";
  const files = conflict.paths.join(", ");
  const before =
    diff.members.length === 0
      ? "No member merges before it."
      : `The change shows only ${memberList(diff)}, the members before it.`;
  return {
    title: `${conflict.proposal} v${String(conflict.version)} conflicts with ${other}`,
    text: `${before} Conflicted ${conflict.paths.length === 1 ? "file" : "files"}: ${files}.`,
  };
}

/** The viewed marks belong to one browser and one merged change: a new main head or a new member version starts them again. */
export function mergedViewedKey(
  slug: string,
  id: string,
  diff: ReviewBatchDiff,
): string {
  const sides =
    diff.basis === "integrated"
      ? `${diff.base ?? ""}..${diff.head ?? ""}`
      : `${diff.base ?? ""}+${diff.members.map((m) => `${m.proposal}v${String(m.version)}`).join("+")}`;
  return `fold.viewed.${slug}.${id}.${sides}`;
}
