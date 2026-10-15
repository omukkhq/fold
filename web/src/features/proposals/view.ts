import type { ProposalDetail } from "@/api/proposals";
import { versionRef, versionVerdict } from "./present";

/** The detail screen's address: the version on screen, an earlier version to compare it with, and the file. */
export interface DetailSearch {
  /** The current version when absent. */
  version?: number;
  /** An earlier version; the screen then compares it with `version`. */
  compare?: number;
  path?: string;
}

function positive(value: unknown): number | undefined {
  const n = typeof value === "string" ? Number(value) : value;
  return typeof n === "number" && Number.isInteger(n) && n >= 1 ? n : undefined;
}

export function parseDetailSearch(
  search: Record<string, unknown>,
): DetailSearch {
  const version = positive(search.version);
  const compare = positive(search.compare);
  const path = typeof search.path === "string" ? search.path.trim() : "";
  return {
    ...(version === undefined ? {} : { version }),
    ...(compare === undefined ? {} : { compare }),
    ...(path === "" ? {} : { path }),
  };
}

/** What the diff shows: one version against its base, or two versions. */
export type DetailView =
  | { kind: "version"; version: number; current: boolean; to: string }
  | {
      kind: "compare";
      from: number;
      version: number;
      fromRef: string;
      to: string;
    };

/** A version the proposal does not have falls back to the current one; a comparison needs an earlier version than the one shown. */
export function resolveView(
  p: ProposalDetail,
  search: DetailSearch,
): DetailView {
  const known = (n: number | undefined) =>
    n !== undefined && p.versions.some((v) => v.version === n);
  const version = known(search.version)
    ? (search.version ?? p.current_version)
    : p.current_version;
  const to = versionRef(p.id, version);
  if (
    known(search.compare) &&
    search.compare !== undefined &&
    search.compare < version
  ) {
    return {
      kind: "compare",
      from: search.compare,
      version,
      fromRef: versionRef(p.id, search.compare),
      to,
    };
  }
  return {
    kind: "version",
    version,
    current: version === p.current_version,
    to,
  };
}

export interface VersionTab {
  key: string;
  label: string;
  compare: boolean;
  search: DetailSearch;
  selected: boolean;
}

/** The newest version first, then a comparison of the previous version with the current one when there are two or more. */
export function versionTabs(p: ProposalDetail, view: DetailView): VersionTab[] {
  const versions = [...p.versions].sort((a, b) => b.version - a.version);
  const tabs: VersionTab[] = versions.map((v) => ({
    key: `v${String(v.version)}`,
    label: `v${String(v.version)} · ${versionVerdict(v.version, p.reviews)}`,
    compare: false,
    search: v.version === p.current_version ? {} : { version: v.version },
    selected: view.kind === "version" && view.version === v.version,
  }));
  const previous = p.current_version - 1;
  if (p.versions.some((v) => v.version === previous)) {
    tabs.push({
      key: "compare",
      label: `Compare v${String(previous)} → v${String(p.current_version)}`,
      compare: true,
      search: { compare: previous },
      selected:
        view.kind === "compare" &&
        view.from === previous &&
        view.version === p.current_version,
    });
  }
  return tabs;
}

export function detailHref(
  slug: string,
  id: string,
  search: DetailSearch,
): string {
  const params = new URLSearchParams();
  if (search.version !== undefined)
    params.set("version", String(search.version));
  if (search.compare !== undefined)
    params.set("compare", String(search.compare));
  if (search.path !== undefined) params.set("path", search.path);
  const query = params.toString();
  return `/p/${slug}/proposals/${id}${query ? `?${query}` : ""}`;
}

/** The version the screen's comments and composer belong to; a comparison has none. */
export function shownVersion(view: DetailView): number | null {
  return view.kind === "version" ? view.version : null;
}
