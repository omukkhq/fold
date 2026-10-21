import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { NotifyTarget, ProjectNotify } from "@/api/notify";
import {
  EMPTY_DRAFT,
  checkDraft,
  checkWebUrl,
  notifyFailure,
  projectStatus,
  secretFrom,
  targetView,
  type TargetDraft,
} from "./notifyModel";

const SECRET = "0123456789abcdef";

function webhook(patch: Partial<TargetDraft> = {}): TargetDraft {
  return {
    ...EMPTY_DRAFT,
    url: " https://hooks.example.com/fold ",
    secret: SECRET,
    ...patch,
  };
}

function email(patch: Partial<TargetDraft> = {}): TargetDraft {
  return {
    ...EMPTY_DRAFT,
    kind: "email",
    to: "team@example.com",
    from: "fold@example.com",
    smtpHost: "smtp.example.com",
    ...patch,
  };
}

function entry(patch: Partial<NotifyTarget>): NotifyTarget {
  return {
    id: "nt-1",
    kind: "webhook",
    interval_minutes: 10,
    created_at: 0,
    created_by: "ana",
    url: "https://hooks.example.com/…",
    to: null,
    from: null,
    smtp_host: null,
    smtp_port: null,
    security: null,
    username: null,
    secret_set: true,
    ...patch,
  };
}

describe("checkDraft", () => {
  it("builds a webhook body with the default interval left to the daemon", () => {
    expect(checkDraft(webhook())).toEqual({
      body: {
        kind: "webhook",
        interval_minutes: null,
        url: "https://hooks.example.com/fold",
        secret: SECRET,
      },
    });
    expect(checkDraft(webhook({ interval: "5" })).body?.interval_minutes).toBe(
      5,
    );
  });

  it("names each webhook field it refuses", () => {
    expect(checkDraft(webhook({ url: "hooks.example.com" })).errors).toEqual({
      url: "Give the full address, starting with https or http.",
    });
    expect(
      checkDraft(webhook({ url: "https://a b" })).errors?.url,
    ).toBeDefined();
    expect(checkDraft(webhook({ secret: "short" })).errors?.secret).toMatch(
      /16 to 512/,
    );
    expect(
      checkDraft(webhook({ secret: "0123456789 abcdef" })).errors?.secret,
    ).toBe("The secret cannot hold a space.");
    for (const interval of ["0", "1441", "ten", "1.5"]) {
      expect(checkDraft(webhook({ interval })).errors?.interval).toMatch(
        /1 to 1440 minutes/,
      );
    }
  });

  it("builds an email body with empty optional fields as null", () => {
    expect(checkDraft(email())).toEqual({
      body: {
        kind: "email",
        interval_minutes: null,
        to: "team@example.com",
        from: "fold@example.com",
        smtp_host: "smtp.example.com",
        smtp_port: null,
        security: "starttls",
        username: null,
        password: null,
      },
    });
    const full = checkDraft(
      email({
        smtpPort: "2525",
        username: " fold ",
        password: "pw",
        security: "tls",
      }),
    );
    expect(full.body).toMatchObject({
      smtp_port: 2525,
      username: "fold",
      password: "pw",
      security: "tls",
    });
  });

  it("names each email field it refuses", () => {
    const errors = checkDraft(
      email({
        to: "a@b, c@d",
        from: "nobody",
        smtpHost: "smtp://host",
        smtpPort: "70000",
        password: "pw",
        security: "none",
      }),
    ).errors;
    expect(Object.keys(errors ?? {}).sort()).toEqual([
      "from",
      "password",
      "smtpHost",
      "smtpPort",
      "to",
    ]);
    expect(checkDraft(email({ smtpPort: "x" })).errors?.smtpPort).toBeDefined();
  });
});

describe("targetView", () => {
  it("shows a webhook by its shown address", () => {
    expect(targetView(entry({}))).toEqual({
      kind: "webhook",
      title: "https://hooks.example.com/…",
      detail: "Webhook · signed with its secret",
      interval: "At most one send every 10 minutes",
    });
    expect(targetView(entry({ url: null, interval_minutes: 1 }))).toMatchObject(
      {
        title: "Webhook",
        interval: "At most one send a minute",
      },
    );
  });

  it("shows an email target by its address, server and sign-in", () => {
    const mail = entry({
      kind: "email",
      url: null,
      to: "team@example.com",
      smtp_host: "smtp.example.com",
      smtp_port: 587,
      security: "starttls",
      username: "fold",
    });
    expect(targetView(mail)).toMatchObject({
      title: "Email to team@example.com",
      detail: "smtp.example.com:587 · STARTTLS · signs in as fold",
    });
    expect(
      targetView({
        ...mail,
        security: null,
        username: null,
        to: null,
        smtp_host: null,
        smtp_port: null,
      }).detail,
    ).toBe(":");
    expect(
      targetView({ ...mail, security: "none", username: null }).detail,
    ).toBe("smtp.example.com:587 · no TLS");
  });
});

describe("projectStatus", () => {
  const settings = (own: number, host: number): ProjectNotify => ({
    targets: Array.from({ length: own }, () => entry({})),
    host_targets: Array.from({ length: host }, () => entry({})),
    uses_host: own === 0,
    link_base: "x",
  });

  it("says whose targets get the notices", () => {
    expect(projectStatus(settings(1, 1))).toMatch(/its own targets/);
    expect(projectStatus(settings(0, 1))).toMatch(/host targets get/);
    expect(projectStatus(settings(0, 0))).toMatch(/No target/);
  });
});

describe("notifyFailure", () => {
  it("words each code and adds the daemon's reason to a failed send", () => {
    expect(
      notifyFailure(
        new ApiError(
          "notify_send_failed",
          "the webhook receiver answered HTTP 500",
        ),
      ),
    ).toEqual({
      code: "notify_send_failed",
      text: "The test notice was not sent: the webhook receiver answered HTTP 500.",
    });
    expect(notifyFailure(new ApiError("notify_send_failed", "")).text).toBe(
      "The test notice was not sent.",
    );
    expect(
      notifyFailure(
        new ApiError("invalid_notify_target", "the URL has no host"),
      ).text,
    ).toBe("The daemon refused the target: the URL has no host.");
    expect(notifyFailure(new ApiError("invalid_notify_target", "")).text).toBe(
      "The daemon refused a field of the target.",
    );
    expect(
      notifyFailure(new ApiError("notify_target_not_found", "")).text,
    ).toMatch(/gone/);
    expect(notifyFailure(new ApiError("notify_settings_io", "")).text).toMatch(
      /could not read or write/,
    );
    expect(notifyFailure(new ApiError("human_name_required", "")).code).toBe(
      "human_name_required",
    );
    expect(notifyFailure(new Error("offline"))).toEqual({
      code: null,
      text: "The request did not reach the daemon. Try again.",
    });
  });
});

describe("secretFrom and checkWebUrl", () => {
  it("writes bytes as two hex digits each", () => {
    expect(secretFrom(new Uint8Array([0, 15, 255]))).toBe("000fff");
  });

  it("takes a full address without its trailing slash, and empty as a clear", () => {
    expect(checkWebUrl(" https://fold.example.com/ ")).toEqual({
      webUrl: "https://fold.example.com",
    });
    expect(checkWebUrl("")).toEqual({ webUrl: null });
    expect(checkWebUrl("fold.example.com").error).toMatch(/full address/);
  });
});
