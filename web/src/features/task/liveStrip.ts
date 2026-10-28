import type { CheckpointSummary, LiveWorkspace, TaskDetail } from "@/api/tasks";
import {
  sentText,
  sentVersion,
  showsStall,
  waitText,
} from "@/features/board/sentVersion";
import { formatQuiet, formatSince } from "@/format/time";

type ChangeKind = LiveWorkspace["files"][number]["change"];

export interface LiveStrip {
  worker: string;
  /** `is editing`; `sent P-1 v1` while the workspace files are the open proposal's newest version; or `no edits for 7 min` while the daemon reports the worker stalled and no sent version waits on others. */
  verb: string;
  /** The file being edited; null after a sent version, while stalled or unless exactly one file changed. */
  file: string | null;
  /** What a sent version waits for, `waits for review`; null without one. */
  wait: string | null;
  /** The strip's details behind its info button: `4 files · ck-31 3 min ago · base r7`; null when the stream and the task read name none. */
  details: string | null;
  stalled: boolean;
}

function files(count: number): string {
  return `${String(count)} ${count === 1 ? "file" : "files"}`;
}

/** Every recorded checkpoint, from the task read and the live frame, newest first and each once. */
export function recordedCheckpoints(
  task: readonly CheckpointSummary[],
  live: LiveWorkspace | null,
): CheckpointSummary[] {
  const byId = new Map<string, CheckpointSummary>();
  for (const ck of [...task, ...(live?.checkpoints ?? [])]) {
    byId.set(ck.id, ck);
  }
  return [...byId.values()].sort((a, b) => b.created_at - a.created_at);
}

/** The newest checkpoint of the task and the live frame together. */
export function latestCheckpoint(
  task: readonly CheckpointSummary[],
  live: LiveWorkspace | null,
): CheckpointSummary | null {
  return recordedCheckpoints(task, live)[0] ?? null;
}

/** `base r7 (main r8)`, `base r8 (main)` on the main head, or `base r7` while the main head is not known. */
export function baseText(workspace: { base: string; main?: string }): string {
  if (workspace.main === undefined) {
    return `base ${workspace.base}`;
  }
  return workspace.base === workspace.main
    ? `base ${workspace.base} (main)`
    : `base ${workspace.base} (main ${workspace.main})`;
}

/**
 * One fact, who is editing what or which version the holder sent and what it waits for, and its details: the files changed, the latest checkpoint and the base.
 * The task payload names the workspace, and its details, activity, checkpoints and proposal; the event stream's frame fills in the rest.
 * The stall wording shows while no sent version waits on others. Null without a workspace.
 */
export function liveStrip(
  task: Pick<
    TaskDetail,
    | "activity"
    | "attention"
    | "checkpoints"
    | "claims"
    | "edits_after_version"
    | "proposal"
    | "workspace"
    | "workspace_info"
  >,
  live: LiveWorkspace | null,
  now: number,
): LiveStrip | null {
  if (task.workspace === null) {
    return null;
  }
  const holder = task.claims[0];
  const worker = live?.worker ?? holder?.holder ?? null;
  if (worker === null) {
    return null;
  }
  const sent = sentVersion(
    task.proposal,
    task.edits_after_version,
    live,
    task.attention,
  );
  const stalled = showsStall(sent, task.attention);
  const activity = task.activity ?? null;
  const lastEdit = Math.max(
    activity?.at ?? Number.NEGATIVE_INFINITY,
    live?.last_activity_at ?? Number.NEGATIVE_INFINITY,
  );
  const parts: string[] = [];
  if (live) {
    parts.push(files(live.files.length));
  }
  const checkpoint = latestCheckpoint(task.checkpoints, live);
  if (checkpoint) {
    parts.push(`${checkpoint.id} ${formatSince(now, checkpoint.created_at)}`);
  }
  const base = task.workspace_info ?? live;
  if (base) {
    parts.push(baseText(base));
  }
  const [only, ...others] = live?.files ?? [];
  const liveFile = only !== undefined && others.length === 0 ? only.path : null;
  let verb = "is editing";
  if (stalled && Number.isFinite(lastEdit)) {
    verb = formatQuiet(now, lastEdit);
  } else if (sent !== null) {
    verb = sentText(sent);
  }
  return {
    worker,
    verb,
    file: sent !== null || stalled ? null : (activity?.path ?? liveFile),
    wait: sent === null ? null : waitText(sent.waits),
    details: parts.length > 0 ? parts.join(" · ") : null,
    stalled,
  };
}

/** The chip beside "Working changes", `Live · 12 s ago`. */
export function liveChipText(live: LiveWorkspace, now: number): string {
  return `Live · ${formatSince(now, live.last_activity_at)}`;
}

/** `4 files against base r7 · not in main`: working changes are never history. */
export function changesNote(
  live: LiveWorkspace,
  workspace: { base: string } | null = null,
): string {
  return `${files(live.files.length)} against base ${(workspace ?? live).base} · not in main`;
}

const LETTERS: Record<ChangeKind, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
};

export function changeLetter(change: ChangeKind): string {
  return LETTERS[change];
}

export type ChangeTone = "added" | "deleted" | "other";

/** Added files take the primary soft fill and deleted files the danger soft fill. */
export function changeTone(change: ChangeKind): ChangeTone {
  if (change === "added") {
    return "added";
  }
  return change === "deleted" ? "deleted" : "other";
}

/** `+42` and `−8` with a true minus sign; null for a zero count, which the row leaves out. */
export function lineCounts(file: {
  added_lines: number;
  removed_lines: number;
  binary: boolean;
}): { added: string | null; removed: string | null } {
  if (file.binary) {
    return { added: null, removed: null };
  }
  return {
    added: file.added_lines > 0 ? `+${String(file.added_lines)}` : null,
    removed: file.removed_lines > 0 ? `−${String(file.removed_lines)}` : null,
  };
}
