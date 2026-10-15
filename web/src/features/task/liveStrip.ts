import type {
  CheckpointSummary,
  LiveWorkspace,
  TaskDetail,
  TaskWorkspace,
} from "@/api/tasks";
import { formatQuiet, formatSince } from "@/format/time";

type ChangeKind = LiveWorkspace["files"][number]["change"];

/**
 * The task payload carries the live state at read time and the event stream carries every later frame of the same
 * workspace; the later observation wins.
 */
export function currentLive(
  workspace: TaskWorkspace | null,
  streamed: LiveWorkspace | undefined,
): LiveWorkspace | null {
  if (workspace === null) {
    return null;
  }
  const fromTask = workspace.live;
  if (streamed?.workspace !== workspace.id) {
    return fromTask;
  }
  if (fromTask === null) {
    return streamed;
  }
  return streamed.last_activity_at > fromTask.last_activity_at
    ? streamed
    : fromTask;
}

export interface LiveStrip {
  worker: string;
  /** `is editing`, or `no edits for 7 min` while the daemon reports the worker stalled. */
  verb: string;
  /** The file being edited; null while stalled or when the daemon cannot name one file. */
  file: string | null;
  /** `· 4 files · ck-31 3 min ago · base r7 (main r8)`. */
  rest: string;
  stalled: boolean;
}

function files(count: number): string {
  return `${String(count)} ${count === 1 ? "file" : "files"}`;
}

/** The newest checkpoint of the task and the live frame together. */
export function latestCheckpoint(
  task: readonly CheckpointSummary[],
  live: LiveWorkspace | null,
): CheckpointSummary | null {
  const all = [...task, ...(live?.checkpoints ?? [])];
  return all.reduce<CheckpointSummary | null>(
    (newest, ck) =>
      newest === null || ck.created_at > newest.created_at ? ck : newest,
    null,
  );
}

/** `base r7 (main r8)`, or `base r8 (main)` when the workspace is on the main head. */
export function baseText(
  workspace: Pick<TaskWorkspace, "base" | "main">,
): string {
  return workspace.base === workspace.main
    ? `base ${workspace.base} (main)`
    : `base ${workspace.base} (main ${workspace.main})`;
}

/** Who is editing what, the files changed, the latest checkpoint and the base against main; null without a workspace. */
export function liveStrip(
  task: Pick<
    TaskDetail,
    "activity" | "attention" | "claims" | "checkpoints" | "workspace"
  >,
  live: LiveWorkspace | null,
  now: number,
): LiveStrip | null {
  const { workspace } = task;
  if (workspace === null) {
    return null;
  }
  const holder = task.claims.find((c) => c.kind === "implementation");
  const worker = live?.worker ?? holder?.holder ?? null;
  if (worker === null) {
    return null;
  }
  const stalled = task.attention.some((a) => a.kind === "stalled");
  const lastEdit = Math.max(
    task.activity?.at ?? Number.NEGATIVE_INFINITY,
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
  parts.push(baseText(workspace));
  const quiet = Number.isFinite(lastEdit);
  return {
    worker,
    verb: stalled && quiet ? formatQuiet(now, lastEdit) : "is editing",
    file: stalled ? null : (task.activity?.path ?? null),
    rest: `· ${parts.join(" · ")}`,
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
  workspace: Pick<TaskWorkspace, "base">,
): string {
  return `${files(live.files.length)} against base ${workspace.base} · not in main`;
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

/** Added files take the primary soft fill and deleted files the danger soft fill, as W03 draws them. */
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
