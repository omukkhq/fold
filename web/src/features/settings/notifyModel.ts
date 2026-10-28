import { ApiError, apiErrorCode } from "@/api/errors";
import type {
  BoardColumn,
  NewNotifyTarget,
  NotifyEventName,
  NotifyPreset,
  NotifySend,
  NotifyTarget,
  NotifyTargetFields,
  NotifyTargetKind,
  ProjectNotify,
} from "@/api/notify";
import { failureText } from "@/format/failure";
import { formatAsOf, formatSince } from "@/format/time";
import type { Failure } from "../workers/startLead";

/** The events a target selects, in the order the form lists them, with the words of each box. `webhook.test` is not among them: every test send carries it. */
export const EVENT_OPTIONS: readonly {
  name: NotifyEventName;
  label: string;
}[] = [
  { name: "needs_you.opened", label: "Needs you: opened" },
  { name: "needs_you.closed", label: "Needs you: closed" },
  { name: "task.moved", label: "Card moved" },
  { name: "task.blocked", label: "Blocker set or cleared" },
  { name: "review.changes_requested", label: "Changes requested" },
  { name: "proposal.integrated", label: "Integrated into main" },
  { name: "integration.failed", label: "Integration failed" },
  { name: "checks.failed", label: "Check failed" },
  { name: "worker.failed", label: "Worker failed" },
  { name: "git.failed", label: "Git failed" },
  { name: "task.created", label: "Card created" },
  { name: "task.sent_back", label: "Task sent back" },
  { name: "proposal.opened", label: "Proposal opened" },
  { name: "review.approved", label: "Approved" },
  { name: "proposal.closed", label: "Proposal closed" },
  { name: "message.to_human", label: "Message to a human" },
  { name: "request.approved", label: "Request approved" },
  { name: "project.paused", label: "Agents paused or resumed" },
];

export const PRESET_OPTIONS: readonly {
  preset: NotifyPreset;
  label: string;
}[] = [
  { preset: "needs_you_only", label: "Needs you only" },
  { preset: "recommended", label: "Recommended" },
  { preset: "all", label: "All" },
  { preset: "custom", label: "Custom" },
];

export const KIND_OPTIONS: readonly {
  kind: NotifyTargetKind;
  label: string;
}[] = [
  { kind: "webhook", label: "Generic JSON" },
  { kind: "slack", label: "Slack" },
  { kind: "discord", label: "Discord" },
];

export const COLUMN_OPTIONS: readonly { column: BoardColumn; label: string }[] =
  [
    { column: "backlog", label: "Backlog" },
    { column: "todo", label: "To do" },
    { column: "in_progress", label: "In progress" },
    { column: "review", label: "Review" },
    { column: "done", label: "Done" },
  ];

export const DEFAULT_WINDOW = 15;
export const MAX_WINDOW = 3600;
export const MAX_NAME = 64;

const RECOMMENDED: readonly NotifyEventName[] = [
  "needs_you.opened",
  "task.moved",
  "task.blocked",
  "review.changes_requested",
  "proposal.integrated",
  "integration.failed",
  "checks.failed",
  "worker.failed",
  "git.failed",
];

/** The events of a preset, as the daemon defines them; `custom` has none of its own. */
export function presetEvents(preset: NotifyPreset): NotifyEventName[] {
  switch (preset) {
    case "needs_you_only":
      return ["needs_you.opened"];
    case "recommended":
      return [...RECOMMENDED];
    case "all":
      return EVENT_OPTIONS.map((option) => option.name);
    case "custom":
      return [];
  }
}

function sameSet<T>(left: readonly T[], right: readonly T[]): boolean {
  const set = new Set(left);
  return set.size === new Set(right).size && right.every((x) => set.has(x));
}

/** The preset whose events are exactly `events`, else `custom`. */
export function presetOf(events: readonly NotifyEventName[]): NotifyPreset {
  const named: NotifyPreset[] = ["needs_you_only", "recommended", "all"];
  return (
    named.find((preset) => sameSet(presetEvents(preset), events)) ?? "custom"
  );
}

/** The add or edit form as a person fills it in. On an edit an empty `url` keeps the URL fold has. */
export interface TargetDraft {
  kind: NotifyTargetKind;
  name: string;
  url: string;
  /** The preset button a person chose; `custom` shows the events as they are. */
  preset: NotifyPreset;
  events: NotifyEventName[];
  movesTo: BoardColumn[];
  window: string;
}

export function newDraft(kind: NotifyTargetKind = "slack"): TargetDraft {
  return {
    kind,
    name: "",
    url: "",
    preset: "recommended",
    events: presetEvents("recommended"),
    movesTo: ["review", "done"],
    window: String(DEFAULT_WINDOW),
  };
}

/** The draft of an edit of `target`. */
export function draftOf(target: NotifyTarget): TargetDraft {
  return {
    kind: target.kind,
    name: target.name,
    url: "",
    preset: target.preset,
    events: target.events.filter((name) => name !== "webhook.test"),
    movesTo: [...target.moves_to],
    window: String(target.window_seconds),
  };
}

/** A preset button sets its events; `custom` keeps the boxes as they are. */
export function choosePreset(
  draft: TargetDraft,
  preset: NotifyPreset,
): TargetDraft {
  return preset === "custom"
    ? { ...draft, preset }
    : { ...draft, preset, events: presetEvents(preset) };
}

function toggled<T>(list: readonly T[], value: T, on: boolean): T[] {
  const rest = list.filter((x) => x !== value);
  return on ? [...rest, value] : rest;
}

/** A box changes the selection, and the preset buttons follow it. */
export function toggleEvent(
  draft: TargetDraft,
  name: NotifyEventName,
  on: boolean,
): TargetDraft {
  const events = toggled(draft.events, name, on);
  return { ...draft, events, preset: presetOf(events) };
}

export function toggleColumn(
  draft: TargetDraft,
  column: BoardColumn,
  on: boolean,
): TargetDraft {
  return { ...draft, movesTo: toggled(draft.movesTo, column, on) };
}

/** True when the card-move columns matter: the selection holds `task.moved`. */
export function choosesMoves(draft: TargetDraft): boolean {
  return draft.events.includes("task.moved");
}

export type DraftField = "name" | "url" | "events" | "window";
export type DraftErrors = Partial<Record<DraftField, string>>;

/** An http or https address with a host and no space or user part. */
const WEB_ADDRESS = /^https?:\/\/[^\s/?#@]+(?:[/?#]\S*)?$/i;
const IPV4_LOOPBACK = /^127(?:\.\d{1,3}){3}$/;

/** True for `localhost` and a loopback address, the only hosts a notice goes to without TLS; the daemon checks the same again. */
export function isLoopbackHost(host: string): boolean {
  const bare = host.replace(/^\[(.*)\]$/, "$1").toLowerCase();
  return bare === "localhost" || bare === "::1" || IPV4_LOOPBACK.test(bare);
}

/** The host of an http or https address, without its port or the brackets of an IPv6 address. */
export function urlHost(url: string): string {
  const authority = url.replace(/^https?:\/\//i, "").split(/[/?#]/)[0] ?? "";
  const bracketed = /^\[([^\]]*)\]/.exec(authority);
  return bracketed ? (bracketed[1] ?? "") : (authority.split(":")[0] ?? "");
}

function urlError(url: string): string | undefined {
  if (!WEB_ADDRESS.test(url)) {
    return "Give the full URL, starting with https or http.";
  }
  if (/^http:/i.test(url) && !isLoopbackHost(urlHost(url))) {
    return "An http URL sends the notices unencrypted: give an https URL, or http only to localhost.";
  }
  return undefined;
}

export type CheckedDraft =
  | { errors: DraftErrors }
  | { errors?: undefined; add: NewNotifyTarget }
  | { errors?: undefined; edit: NotifyTargetFields };

/** The body of an add, or of an edit with `editing`, once the fields pass the checks the daemon makes again. */
export function checkDraft(draft: TargetDraft, editing: boolean): CheckedDraft {
  const errors: DraftErrors = {};
  const name = draft.name.trim();
  if (name === "" || Array.from(name).length > MAX_NAME) {
    errors.name = `Give a name of 1 to ${String(MAX_NAME)} characters.`;
  }
  const url = draft.url.trim();
  if (!(editing && url === "")) {
    const error = urlError(url);
    if (error !== undefined) errors.url = error;
  }
  if (draft.events.length === 0) {
    errors.events = "Choose at least one event.";
  }
  const window = draft.window.trim();
  const seconds = /^\d+$/.test(window) ? Number(window) : Number.NaN;
  if (!(seconds >= 0 && seconds <= MAX_WINDOW)) {
    errors.window = `Give 0 to ${String(MAX_WINDOW)} seconds; 0 sends each notice alone.`;
  }
  if (Object.keys(errors).length > 0) {
    return { errors };
  }
  const preset = presetOf(draft.events);
  const selection = preset === "custom" ? { events: draft.events } : { preset };
  const common = {
    ...selection,
    moves_to: draft.movesTo,
    window_seconds: seconds,
  };
  if (editing) {
    return {
      edit: { ...common, name, ...(url === "" ? {} : { url }) },
    };
  }
  return { add: { ...common, kind: draft.kind, name, url } };
}

export type Tone = "ok" | "bad" | "muted";

export interface TargetView {
  title: string;
  /** The kind, the URL without its path and the selection, such as `Slack · <host>/… · Recommended (9 events)`. */
  detail: string;
  scope: string;
  state: { label: string; tone: Tone };
  /** How the sends go now, such as `Sent 2 min ago` or `3 sends failed · next try in 4 min`; null before the first send. */
  health: string | null;
}

function kindLabel(kind: NotifyTargetKind): string {
  return KIND_OPTIONS.find((option) => option.kind === kind)?.label ?? kind;
}

function columnLabel(column: BoardColumn): string {
  return (
    COLUMN_OPTIONS.find((option) => option.column === column)?.label ?? column
  );
}

/** The words of a target's selection. */
export function selectionText(target: NotifyTarget): string {
  const events = target.events.filter((name) => name !== "webhook.test");
  const moves = events.includes("task.moved")
    ? ` · card moves to ${target.moves_to.map(columnLabel).join(", ") || "no column"}`
    : "";
  switch (target.preset) {
    case "recommended":
      return `Recommended (${String(events.length)} events)${moves}`;
    case "all":
      return `All events${moves}`;
    case "needs_you_only":
      return "Needs you only";
    case "custom":
      return events.length === 1
        ? `${events[0] ?? ""}${moves}`
        : `${String(events.length)} events${moves}`;
  }
}

function count(value: number, one: string, many: string): string {
  return `${String(value)} ${value === 1 ? one : many}`;
}

function waitText(now: number, at: number): string {
  const minutes = Math.max(0, Math.ceil((at - now) / 60_000));
  return minutes <= 1
    ? "next try in 1 min"
    : `next try in ${String(minutes)} min`;
}

/** One row of the target list. */
export function targetView(target: NotifyTarget, now: number): TargetView {
  const state: TargetView["state"] =
    target.state === "on"
      ? { label: "On", tone: "ok" }
      : target.state === "off"
        ? { label: "Off", tone: "muted" }
        : target.state === "gone"
          ? { label: "Off: the receiver is gone", tone: "bad" }
          : { label: "Off: its sends failed", tone: "bad" };
  const parts: string[] = [];
  if (target.failures > 0) {
    parts.push(count(target.failures, "send failed", "sends failed"));
    if (target.next_try_at !== null) {
      parts.push(waitText(now, target.next_try_at));
    }
  } else if (target.last_ok_at !== null) {
    parts.push(`Sent ${formatSince(now, target.last_ok_at)}`);
  }
  return {
    title: target.name,
    detail: `${kindLabel(target.kind)} · ${target.url} · ${selectionText(target)}`,
    scope: target.host ? "Host target" : "This project",
    state,
    health: parts.length === 0 ? null : parts.join(" · "),
  };
}

export interface SendView {
  time: string;
  summary: string;
  /** The status, or the error word of a try without one. */
  answer: string;
  tries: string;
  tone: Tone;
  /** Send again works on a finished send only. */
  canSendAgain: boolean;
}

/** One row of the send log. */
export function sendView(send: NotifySend, now: number): SendView {
  const answer =
    send.status !== null
      ? String(send.status)
      : (send.error ?? (send.state === "sending" ? "sending" : "waiting"));
  const tone: Tone =
    send.state === "delivered"
      ? "ok"
      : send.status === null && send.error === null
        ? "muted"
        : "bad";
  const more = send.count - send.notices.length;
  const summary =
    send.notices.slice(0, 2).join(", ") +
    (send.notices.length > 2
      ? ` and ${String(send.notices.length - 2 + Math.max(0, more))} more`
      : "");
  return {
    time: formatAsOf(now, send.last_try_at ?? send.created_at),
    summary,
    answer,
    tries: `try ${String(send.tries)}`,
    tone,
    canSendAgain: send.state === "delivered" || send.state === "given_up",
  };
}

/** The line above the targets: which ones get this project's notices. */
export function projectStatus(settings: ProjectNotify): string {
  const own = settings.targets.length;
  const host = settings.use_host_targets ? settings.host_targets.length : 0;
  if (own + host === 0) {
    return "No target gets this project's notices yet.";
  }
  return `${count(own + host, "target gets", "targets get")} this project's notices.`;
}

export function hostTargetsHint(settings: ProjectNotify): string {
  return settings.use_host_targets
    ? "The host targets get this project's notices too."
    : "Only this project's own targets get its notices.";
}

/** The hint of the settings: without a web address, notices carry no links. */
export function webUrlHint(webUrl: string | null): string {
  return webUrl === null
    ? "Notices carry no links until you set the address people open fold at."
    : `Links in notices start with ${webUrl}.`;
}

function daemonMessage(error: unknown): string {
  return error instanceof ApiError ? error.message : "";
}

/** A refused write or a failed test, worded for its code. A failed test also says how the receiver answered, in the daemon's words, which never hold a URL or a secret. */
export function notifyFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  const reason = daemonMessage(error);
  switch (code) {
    case "notify_send_failed":
      return {
        code,
        text:
          reason === ""
            ? "The test was not sent."
            : `The test was not sent: ${reason}.`,
      };
    case "invalid_notify_target":
    case "plaintext_notify_target":
      return {
        code,
        text:
          reason === ""
            ? "The daemon refused a field of the target."
            : `The daemon refused the target: ${reason}.`,
      };
    case "notify_target_not_found":
      return {
        code,
        text: "This target is gone. The list shows the targets as they are now.",
      };
    case "notify_send_not_found":
      return { code, text: "This send is no longer in the log." };
    case "notify_settings_io":
      return {
        code,
        text: "The daemon could not read or write its notice settings.",
      };
    default:
      return { code, text: failureText(error) };
  }
}

/** The body of the web address change: null clears it; an address without its http or https scheme is refused. */
export function checkWebUrl(
  text: string,
): { webUrl: string | null; error?: undefined } | { error: string } {
  const trimmed = text.trim();
  if (trimmed === "") return { webUrl: null };
  if (!WEB_ADDRESS.test(trimmed)) {
    return {
      error:
        "Give the full address people open fold at, starting with https or http.",
    };
  }
  return { webUrl: trimmed.replace(/\/+$/, "") };
}
