import { ApiError, apiErrorCode } from "@/api/errors";
import type {
  NewNotifyTarget,
  NotifySecurity,
  NotifyTarget,
  ProjectNotify,
} from "@/api/notify";
import { failureText } from "@/format/failure";
import type { Failure } from "../workers/startLead";

export type TargetKind = "webhook" | "email";

/** The add form as a person types it: every field is text until the checks pass. */
export interface TargetDraft {
  kind: TargetKind;
  interval: string;
  url: string;
  secret: string;
  to: string;
  from: string;
  smtpHost: string;
  smtpPort: string;
  security: NotifySecurity;
  username: string;
  password: string;
}

export type DraftField = Exclude<keyof TargetDraft, "kind" | "security">;
export type DraftErrors = Partial<Record<DraftField, string>>;

export const EMPTY_DRAFT: TargetDraft = {
  kind: "webhook",
  interval: "",
  url: "",
  secret: "",
  to: "",
  from: "",
  smtpHost: "",
  smtpPort: "",
  security: "starttls",
  username: "",
  password: "",
};

export const DEFAULT_INTERVAL = 10;
export const MAX_INTERVAL = 1440;
export const MIN_SECRET = 16;
export const MAX_SECRET = 512;

export const SECURITY_OPTIONS: readonly {
  value: NotifySecurity;
  label: string;
}[] = [
  { value: "starttls", label: "STARTTLS (port 587)" },
  { value: "tls", label: "TLS (port 465)" },
  { value: "none", label: "None, for a trusted relay (port 25)" },
];

const SECURITY_NAMES: Record<NotifySecurity, string> = {
  starttls: "STARTTLS",
  tls: "TLS",
  none: "no TLS",
};

/** An http or https address with a host and no space. */
const WEB_ADDRESS = /^https?:\/\/[^\s/?#@]+(?:[/?#]\S*)?$/i;
const EMAIL = /^[^\s@,;<>"]+@[^\s@,;<>"]+$/;
const HOST = /^[a-z0-9.\-_:[\]]+$/i;
const SPACE = /\s/;

function wholeNumber(text: string): number | null {
  return /^\d+$/.test(text) ? Number(text) : null;
}

function intervalError(text: string): string | undefined {
  if (text === "") return undefined;
  const minutes = wholeNumber(text);
  return minutes === null || minutes < 1 || minutes > MAX_INTERVAL
    ? `Give 1 to ${String(MAX_INTERVAL)} minutes, or leave it empty for ${String(DEFAULT_INTERVAL)}.`
    : undefined;
}

function webhookErrors(draft: TargetDraft): DraftErrors {
  const errors: DraftErrors = {};
  if (!WEB_ADDRESS.test(draft.url.trim())) {
    errors.url = "Give the full address, starting with https or http.";
  }
  const secret = draft.secret;
  if (secret.length < MIN_SECRET || secret.length > MAX_SECRET) {
    errors.secret = `Give ${String(MIN_SECRET)} to ${String(MAX_SECRET)} characters, or use Generate.`;
  } else if (SPACE.test(secret)) {
    errors.secret = "The secret cannot hold a space.";
  }
  return errors;
}

function emailErrors(draft: TargetDraft): DraftErrors {
  const errors: DraftErrors = {};
  const address = "Give one address, such as team@example.com.";
  if (!EMAIL.test(draft.to.trim())) errors.to = address;
  if (!EMAIL.test(draft.from.trim())) errors.from = address;
  if (!HOST.test(draft.smtpHost.trim())) {
    errors.smtpHost =
      "Give the server's host name alone, such as smtp.example.com.";
  }
  if (draft.smtpPort !== "") {
    const port = wholeNumber(draft.smtpPort);
    if (port === null || port < 1 || port > 65535) {
      errors.smtpPort = "Give a port from 1 to 65535, or leave it empty.";
    }
  }
  if (draft.password !== "" && draft.security === "none") {
    errors.password =
      "A password needs TLS or STARTTLS, so it never travels in the clear.";
  }
  return errors;
}

function optional(text: string): string | null {
  const trimmed = text.trim();
  return trimmed === "" ? null : trimmed;
}

/** The body of the add, or the error of each field the checks refuse. The daemon checks the same again. */
export function checkDraft(
  draft: TargetDraft,
):
  | { body: NewNotifyTarget; errors?: undefined }
  | { body?: undefined; errors: DraftErrors } {
  const errors: DraftErrors = {
    ...(draft.kind === "webhook" ? webhookErrors(draft) : emailErrors(draft)),
  };
  const interval = intervalError(draft.interval);
  if (interval !== undefined) errors.interval = interval;
  if (Object.keys(errors).length > 0) {
    return { errors };
  }
  const interval_minutes =
    draft.interval === "" ? null : Number(draft.interval);
  if (draft.kind === "webhook") {
    return {
      body: {
        kind: "webhook",
        interval_minutes,
        url: draft.url.trim(),
        secret: draft.secret,
      },
    };
  }
  return {
    body: {
      kind: "email",
      interval_minutes,
      to: draft.to.trim(),
      from: draft.from.trim(),
      smtp_host: draft.smtpHost.trim(),
      smtp_port: draft.smtpPort === "" ? null : Number(draft.smtpPort),
      security: draft.security,
      username: optional(draft.username),
      password: draft.password === "" ? null : draft.password,
    },
  };
}

/** A signing secret of hex digits from `bytes`, which come from the browser's random source. */
export function secretFrom(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  );
}

export interface TargetView {
  kind: TargetKind;
  title: string;
  detail: string;
  interval: string;
}

/** One line of a target: where it sends, how, and how often at most. */
export function targetView(target: NotifyTarget): TargetView {
  const interval =
    target.interval_minutes === 1
      ? "At most one send a minute"
      : `At most one send every ${String(target.interval_minutes)} minutes`;
  if (target.kind === "webhook") {
    return {
      kind: "webhook",
      title: target.url ?? "Webhook",
      detail: "Webhook · signed with its secret",
      interval,
    };
  }
  const security =
    target.security === null ? "" : ` · ${SECURITY_NAMES[target.security]}`;
  const signIn =
    target.username === null ? "" : ` · signs in as ${target.username}`;
  return {
    kind: "email",
    title: `Email to ${target.to ?? ""}`,
    detail: `${target.smtp_host ?? ""}:${String(target.smtp_port ?? "")}${security}${signIn}`,
    interval,
  };
}

/** Which targets get the project's notices. */
export function projectStatus(settings: ProjectNotify): string {
  if (settings.targets.length > 0) {
    return "This project sends its notices to its own targets. The host targets do not get them.";
  }
  if (settings.host_targets.length > 0) {
    return "This project has no targets of its own, so the host targets get its notices.";
  }
  return "No target gets this project's notices yet. Add one here, or add a host target for every project.";
}

function daemonMessage(error: unknown): string {
  return error instanceof ApiError ? error.message : "";
}

/** A refused write or a failed test, worded for its code. A failed test also says what failed, in the daemon's words, which never hold a URL or a secret. */
export function notifyFailure(error: unknown): Failure {
  const code = apiErrorCode(error);
  switch (code) {
    case "notify_send_failed": {
      const reason = daemonMessage(error);
      return {
        code,
        text:
          reason === ""
            ? "The test notice was not sent."
            : `The test notice was not sent: ${reason}.`,
      };
    }
    case "invalid_notify_target": {
      const reason = daemonMessage(error);
      return {
        code,
        text:
          reason === ""
            ? "The daemon refused a field of the target."
            : `The daemon refused the target: ${reason}.`,
      };
    }
    case "notify_target_not_found":
      return {
        code,
        text: "This target is gone. The list shows the targets as they are now.",
      };
    case "notify_settings_io":
      return {
        code,
        text: "The daemon could not read or write its notification settings.",
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
