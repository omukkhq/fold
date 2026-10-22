import type { ChangeRange, CommentSide, ProposalDetail } from "@/api/proposals";
import { versionRef, versionVerdict } from "./present";
import { projectPath, withQuery } from "@/routes/paths";

/** The four tabs of the detail screen; Overview is the default. */
export type DetailTab = "overview" | "changes" | "checks" | "conversation";

const TABS: readonly DetailTab[] = [
  "overview",
  "changes",
  "checks",
  "conversation",
];

/** The detail screen's address: the tab, the version on screen, an earlier version whose change since it the screen shows instead of the full change, the file, and the check run and step the Checks tab shows. */
export interface DetailSearch {
  /** Absent, the tab is Changes for an address that names a file or an earlier version, and Overview otherwise. */
  tab?: DetailTab;
  /** The current version when absent. */
  version?: number;
  /** An earlier version; the screen then shows only what changed after it. */
  since?: number;
  path?: string;
  /** A check run, such as `cr-4`. */
  run?: string;
  /** The name of a command of `run`. */
  step?: string;
}

function positive(value: unknown): number | undefined {
  const n = typeof value === "string" ? Number(value) : value;
  return typeof n === "number" && Number.isInteger(n) && n >= 1 ? n : undefined;
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

export function isDetailTab(value: unknown): value is DetailTab {
  return TABS.some((tab) => tab === value);
}

export function parseDetailSearch(
  search: Record<string, unknown>,
): DetailSearch {
  const version = positive(search.version);
  const since = positive(search.since);
  const path = text(search.path);
  const run = text(search.run);
  const step = text(search.step);
  return {
    ...(isDetailTab(search.tab) ? { tab: search.tab } : {}),
    ...(version === undefined ? {} : { version }),
    ...(since === undefined ? {} : { since }),
    ...(path === "" ? {} : { path }),
    ...(/^cr-[1-9][0-9]*$/.test(run) ? { run } : {}),
    ...(step === "" ? {} : { step }),
  };
}

/** The tab the address shows. Older links name only a file or an earlier version, and those open the change. */
export function detailTab(search: DetailSearch): DetailTab {
  if (search.tab !== undefined) return search.tab;
  return search.path !== undefined || search.since !== undefined
    ? "changes"
    : "overview";
}

/** The address of another tab: the version, change and file stay for the Changes tab, and the run and step for the Checks tab. */
export function tabSearch(search: DetailSearch, tab: DetailTab): DetailSearch {
  const kept: DetailSearch = {
    ...(search.version === undefined ? {} : { version: search.version }),
    ...(search.since === undefined ? {} : { since: search.since }),
    ...(search.path === undefined ? {} : { path: search.path }),
    ...(search.run === undefined ? {} : { run: search.run }),
    ...(search.step === undefined ? {} : { step: search.step }),
  };
  return tab === "overview" && detailTab(kept) === "overview"
    ? kept
    : { ...kept, tab };
}

/** The Changes tab at `search`. */
export function changesSearch(search: DetailSearch): DetailSearch {
  return { ...search, tab: "changes" };
}

/**
 * What the diff shows: one version's full change against its base revision, or only what changed after an earlier version of the same proposal.
 * `range` is what the proposal diff route reads; `label` names the shown version as the screen writes it, such as `P-3 v2`.
 */
export interface DetailView {
  kind: "version" | "since";
  version: number;
  current: boolean;
  range: ChangeRange;
  label: string;
}

/** A version the proposal does not have falls back to the current one; a change since another version needs an earlier version than the one shown. */
export function resolveView(
  p: ProposalDetail,
  search: DetailSearch,
): DetailView {
  const current = p.summary.current_version;
  const find = (n: number | undefined) =>
    n === undefined ? undefined : p.versions.find((v) => v.version === n);
  const shown =
    find(search.version) ?? find(current) ?? p.versions.at(-1) ?? p.selected;
  const earlier = find(search.since);
  const since =
    earlier && earlier.version < shown.version ? earlier.version : null;
  return {
    kind: since === null ? "version" : "since",
    version: shown.version,
    current: shown.version === current,
    range: { version: shown.version, since },
    label: versionRef(p.summary.id, shown.version),
  };
}

/** The earlier version a reviewer of `version` reads the change since: the newest one with a verdict by anyone, a comment not counted, as the daemon's `last_reviewed_version` is for the current version. Null when no earlier version has one. */
export function defaultSince(
  p: ProposalDetail,
  version: number,
): number | null {
  const reviewed = p.reviews
    .filter((r) => r.verdict !== "comment" && r.version < version)
    .map((r) => r.version);
  return reviewed.length > 0 ? Math.max(...reviewed) : null;
}

export interface ChangeOption {
  key: "full" | "since";
  label: string;
  search: DetailSearch;
  selected: boolean;
}

/** The two choices of the change switch, or null when the shown version has no earlier version to read the change since. */
export function changeOptions(
  p: ProposalDetail,
  view: DetailView,
): ChangeOption[] | null {
  const since = view.range.since ?? defaultSince(p, view.version);
  if (since === null) return null;
  const version: DetailSearch = view.current
    ? { tab: "changes" }
    : { tab: "changes", version: view.version };
  return [
    {
      key: "full",
      label: "Full change",
      search: version,
      selected: view.kind === "version",
    },
    {
      key: "since",
      label: `Since v${String(since)}`,
      search: { ...version, since },
      selected: view.kind === "since",
    },
  ];
}

export const OLD_SIDE_NOTE =
  "A change since an earlier version takes comments on the new side only: its lines are the version's own.";

/** The empty state of a range that changes no file. */
export function emptyChangeText(view: DetailView): string {
  return view.range.since === null
    ? `${view.label} holds the same files as its base.`
    : `${view.label} holds the same files as v${String(view.range.since)}.`;
}

/** On a change since an earlier version the old side holds that version's lines, which no comment reads, so only the new side takes a comment. */
export function commentsOnSide(view: DetailView, side: CommentSide): boolean {
  return view.kind === "version" || side === "new";
}

export interface VersionTab {
  key: string;
  label: string;
  search: DetailSearch;
  selected: boolean;
}

/** The newest version first, each with its latest verdict, as the version menu of the Changes tab lists them. */
export function versionTabs(p: ProposalDetail, view: DetailView): VersionTab[] {
  const current = p.summary.current_version;
  return [...p.versions]
    .sort((a, b) => b.version - a.version)
    .map((v) => ({
      key: `v${String(v.version)}`,
      label: `v${String(v.version)} · ${versionVerdict(v.version, p.reviews)}`,
      search:
        v.version === current
          ? { tab: "changes" }
          : { tab: "changes", version: v.version },
      selected: view.version === v.version,
    }));
}

export function detailHref(
  slug: string,
  id: string,
  search: DetailSearch,
): string {
  const params = new URLSearchParams();
  if (search.tab !== undefined) params.set("tab", search.tab);
  if (search.version !== undefined)
    params.set("version", String(search.version));
  if (search.since !== undefined) params.set("since", String(search.since));
  if (search.path !== undefined) params.set("path", search.path);
  if (search.run !== undefined) params.set("run", search.run);
  if (search.step !== undefined) params.set("step", search.step);
  return withQuery(projectPath(slug, "proposals", id), params);
}

/** The version the screen's comments and composer belong to. */
export function shownVersion(view: DetailView): number {
  return view.version;
}
