import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { keys } from "@/api/keys";
import { notifySend, notifyTarget } from "@/test/notify";
import {
  COLUMN_OPTIONS,
  EVENT_OPTIONS,
  checkDraft,
  checkWebUrl,
  choosePreset,
  choosesMoves,
  draftOf,
  enabledHint,
  failureField,
  hostTargetsHint,
  isLoopbackHost,
  newDraft,
  notifyFailure,
  presetEvents,
  presetOf,
  projectStatus,
  refreshedByNotifyWrite,
  selectionText,
  sendView,
  targetView,
  toggleColumn,
  toggleEvent,
  urlHost,
  webUrlHint,
} from "./notifyModel";

const SECURE = ["https:", "", "hooks.example.org", "fold"].join("/");
const LOCAL = ["http:", "", "127.0.0.1:9000", "hook"].join("/");
const PLAIN = ["http:", "", "hooks.example.org", "fold"].join("/");
const NOW = 1_760_000_600_000;

function refused(code: string, message = ""): ApiError {
  return new ApiError(code, message);
}

describe("presets", () => {
  it("hold the daemon's events and a selection names its preset", () => {
    expect(presetEvents("recommended")).toHaveLength(9);
    expect(presetEvents("needs_you_only")).toEqual(["needs_you.opened"]);
    expect(presetEvents("all")).toHaveLength(EVENT_OPTIONS.length);
    expect(presetEvents("custom")).toEqual([]);
    for (const preset of ["recommended", "all", "needs_you_only"] as const) {
      expect(presetOf(presetEvents(preset))).toBe(preset);
    }
    expect(presetOf(["task.moved"])).toBe("custom");
    expect(presetOf([...presetEvents("recommended"), "task.created"])).toBe(
      "custom",
    );
  });

  it("follow the boxes, and a preset button sets them", () => {
    let draft = newDraft();
    expect(draft.preset).toBe("recommended");
    draft = toggleEvent(draft, "task.created", true);
    expect(draft.preset).toBe("custom");
    draft = toggleEvent(draft, "task.created", false);
    expect(draft.preset).toBe("recommended");
    draft = choosePreset(draft, "needs_you_only");
    expect(draft.events).toEqual(["needs_you.opened"]);
    expect(choosesMoves(draft)).toBe(false);
    draft = choosePreset(draft, "custom");
    expect(draft.events).toEqual(["needs_you.opened"]);
    expect(choosePreset(draft, "all").events).toHaveLength(18);
    draft = toggleColumn(newDraft(), "in_progress", true);
    expect(draft.movesTo).toEqual(["review", "done", "in_progress"]);
    expect(toggleColumn(draft, "review", false).movesTo).toEqual([
      "done",
      "in_progress",
    ]);
  });
});

describe("checkDraft", () => {
  it("builds an add with a preset or the chosen events", () => {
    const draft = {
      ...newDraft("webhook"),
      name: " Release bot ",
      url: SECURE,
    };
    expect(checkDraft(draft, false)).toEqual({
      add: {
        kind: "webhook",
        name: "Release bot",
        url: SECURE,
        preset: "recommended",
        moves_to: ["review", "done"],
        window_seconds: 15,
      },
    });
    const custom = toggleEvent({ ...draft, window: "0" }, "task.created", true);
    const checked = checkDraft(custom, false);
    expect("add" in checked && checked.add.events).toHaveLength(10);
    expect("add" in checked && checked.add.window_seconds).toBe(0);
    expect("add" in checked && checked.add.preset).toBeUndefined();
  });

  it("keeps the URL of an edit that leaves it empty", () => {
    const draft = draftOf(
      notifyTarget({
        events: ["task.moved", "webhook.test"],
        preset: "custom",
      }),
    );
    expect(draft.events).toEqual(["task.moved"]);
    expect(checkDraft(draft, true)).toEqual({
      edit: {
        name: "Team Slack",
        events: ["task.moved"],
        moves_to: ["review", "done"],
        window_seconds: 15,
      },
    });
    const moved = checkDraft({ ...draft, url: LOCAL }, true);
    expect("edit" in moved && moved.edit.url).toBe(LOCAL);
  });

  it("refuses what the daemon would refuse", () => {
    const checked = checkDraft(
      { ...newDraft(), name: "", url: PLAIN, events: [], window: "4000" },
      false,
    );
    expect(checked.errors).toEqual({
      name: "Give a name of 1 to 64 characters.",
      url: "An http URL sends the notices unencrypted: give an https URL, or http only to localhost.",
      events: "Choose at least one event.",
      window: "Give 0 to 3600 seconds; 0 sends each notice alone.",
    });
    expect(
      checkDraft({ ...newDraft(), name: "x", url: "hooks.example.org" }, false)
        .errors?.url,
    ).toBe("Give the full URL, starting with https or http.");
    expect(
      checkDraft(
        { ...newDraft(), name: "x", url: SECURE, window: "1.5" },
        false,
      ).errors?.window,
    ).toBeDefined();
  });

  it("knows the loopback hosts", () => {
    expect(isLoopbackHost("localhost")).toBe(true);
    expect(isLoopbackHost("[::1]")).toBe(true);
    expect(isLoopbackHost("127.8.9.1")).toBe(true);
    expect(isLoopbackHost("10.0.0.1")).toBe(false);
    expect(urlHost(LOCAL)).toBe("127.0.0.1");
    expect(urlHost(["http:", "", "[::1]:9", "x"].join("/"))).toBe("::1");
  });
});

describe("targetView", () => {
  it("shows a target that works with its last send", () => {
    const view = targetView(
      notifyTarget({ last_ok_at: NOW - 120_000, host: true }),
      NOW,
    );
    expect(view).toEqual({
      title: "Team Slack",
      detail: `Slack · ${["https:", "", "hooks.slack.com", "…"].join("/")} · Recommended (9 events) · card moves to Review, Done`,
      scope: "Host target",
      unused: false,
      state: { label: "On", tone: "ok" },
      health: "Sent 2 min ago",
    });
  });

  it("shows failed sends, the next try and why a target is off", () => {
    const failing = targetView(
      notifyTarget({
        failures: 3,
        next_try_at: NOW + 4 * 60_000,
        state: "failing",
      }),
      NOW,
    );
    expect(failing.health).toBe("3 sends failed · next try in 4 min");
    expect(failing.state).toEqual({
      label: "Off: its sends failed",
      tone: "bad",
    });
    expect(
      targetView(notifyTarget({ failures: 1, next_try_at: NOW }), NOW).health,
    ).toBe("1 send failed · next try in 1 min");
    expect(targetView(notifyTarget({ failures: 1 }), NOW).health).toBe(
      "1 send failed",
    );
    expect(targetView(notifyTarget({ state: "gone" }), NOW).state.label).toBe(
      "Off: the receiver is gone",
    );
    expect(targetView(notifyTarget({ state: "off" }), NOW).state.tone).toBe(
      "muted",
    );
    expect(targetView(notifyTarget(), NOW).health).toBeNull();
    expect(targetView(notifyTarget({ kind: "webhook" }), NOW).detail).toMatch(
      /^Generic JSON/,
    );
  });

  it("greys a host target the project does not use, and only a host target", () => {
    const host = targetView(notifyTarget({ host: true }), NOW, false);
    expect(host.unused).toBe(true);
    expect(host.scope).toBe(
      "Host target · this project does not use the host targets",
    );
    const own = targetView(notifyTarget({ host: false }), NOW, false);
    expect(own.unused).toBe(false);
    expect(own.scope).toBe("This project");
  });

  it("says what the on and off box does", () => {
    expect(enabledHint(notifyTarget(), true)).toBe("Saving…");
    expect(
      enabledHint(notifyTarget({ enabled: false, state: "off" }), false),
    ).toMatch(/^Off/);
    expect(enabledHint(notifyTarget({ state: "failing" }), false)).toMatch(
      /test that gets through/,
    );
    expect(enabledHint(notifyTarget({ state: "gone" }), false)).toMatch(
      /test that gets through/,
    );
    expect(enabledHint(notifyTarget(), false)).toMatch(/events it selected/);
  });

  it("names the board's columns as the board does", () => {
    expect(COLUMN_OPTIONS.map((option) => option.label)).toEqual([
      "Backlog",
      "To do",
      "In progress",
      "Review",
      "Done",
    ]);
  });

  it("words each selection", () => {
    expect(
      selectionText(
        notifyTarget({ preset: "all", events: ["task.moved"], moves_to: [] }),
      ),
    ).toBe("All events · card moves to no column");
    expect(
      selectionText(
        notifyTarget({
          preset: "needs_you_only",
          events: ["needs_you.opened"],
        }),
      ),
    ).toBe("Needs you only");
    expect(
      selectionText(
        notifyTarget({
          preset: "custom",
          events: ["task.moved"],
          moves_to: ["review"],
        }),
      ),
    ).toBe("task.moved · card moves to Review");
    expect(
      selectionText(
        notifyTarget({
          preset: "custom",
          events: ["checks.failed", "git.failed"],
        }),
      ),
    ).toBe("2 events");
  });
});

describe("sendView", () => {
  it("shows a delivered send that can go again", () => {
    expect(sendView(notifySend(), NOW)).toMatchObject({
      summary: "task.moved T-18",
      answer: "200",
      tries: "try 1",
      tone: "ok",
      canSendAgain: true,
    });
  });

  it("shows a failed try, a waiting send and a long send", () => {
    expect(
      sendView(notifySend({ state: "pending", status: 503, tries: 3 }), NOW),
    ).toMatchObject({ answer: "503", tone: "bad", canSendAgain: false });
    expect(
      sendView(
        notifySend({ state: "given_up", status: null, error: "timeout" }),
        NOW,
      ),
    ).toMatchObject({ answer: "timeout", tone: "bad", canSendAgain: true });
    expect(
      sendView(
        notifySend({
          state: "pending",
          status: null,
          last_try_at: null,
          tries: 0,
        }),
        NOW,
      ),
    ).toMatchObject({ answer: "waiting", tone: "muted" });
    expect(
      sendView(notifySend({ state: "sending", status: null }), NOW).answer,
    ).toBe("sending");
    expect(
      sendView(notifySend({ notices: ["a", "b", "c", "d"], count: 120 }), NOW)
        .summary,
    ).toBe("a, b and 118 more");
  });
});

describe("the settings texts", () => {
  const settings = {
    targets: [notifyTarget()],
    host_targets: [notifyTarget({ id: "nt-2", host: true })],
    use_host_targets: true,
    web_url: null,
  };

  it("count the targets that get the notices", () => {
    expect(projectStatus(settings)).toBe(
      "2 targets get this project's notices.",
    );
    expect(projectStatus({ ...settings, use_host_targets: false })).toBe(
      "1 target gets this project's notices.",
    );
    expect(projectStatus({ ...settings, targets: [], host_targets: [] })).toBe(
      "No target gets this project's notices yet.",
    );
    expect(hostTargetsHint(settings)).toMatch(/too/);
    expect(hostTargetsHint({ ...settings, use_host_targets: false })).toMatch(
      /Only/,
    );
    expect(webUrlHint(null)).toMatch(/every project: notices carry no links/);
    expect(webUrlHint("x")).toBe(
      "The address is the host's, for every project: links in notices start with x.",
    );
  });

  it("word the refusals", () => {
    expect(
      notifyFailure(
        refused("notify_send_failed", "the receiver answered HTTP 503"),
      ).text,
    ).toBe("The receiver answered HTTP 503.");
    expect(notifyFailure(refused("notify_send_failed")).text).toBe(
      "The test send failed.",
    );
    expect(notifyFailure(refused("notify_target_inactive")).text).toMatch(
      /^fold sends nothing to this target now/,
    );
    expect(
      notifyFailure(refused("plaintext_notify_target", "plain")).text,
    ).toBe("The daemon refused the target: plain.");
    expect(notifyFailure(refused("invalid_notify_target")).text).toBe(
      "The daemon refused a field of the target.",
    );
    for (const code of [
      "notify_target_not_found",
      "notify_send_not_found",
      "notify_settings_io",
      "human_required",
    ]) {
      expect(notifyFailure(refused(code)).code).toBe(code);
    }
    expect(notifyFailure(new Error("x")).code).toBeNull();
  });

  it("puts a plain-text refusal on the URL field and nothing else on a field", () => {
    expect(failureField(refused("plaintext_notify_target"))).toBe("url");
    expect(failureField(refused("invalid_notify_target"))).toBeNull();
    expect(failureField(new Error("x"))).toBeNull();
  });

  it("checks the web address", () => {
    expect(checkWebUrl("  ")).toEqual({ webUrl: null });
    expect(checkWebUrl(`${SECURE}/`)).toEqual({ webUrl: SECURE });
    expect(checkWebUrl("fold.example.org").error).toBeDefined();
  });
});

describe("refreshedByNotifyWrite", () => {
  it("refreshes the notice settings, the send logs and Needs you of every project, and nothing else", () => {
    expect(refreshedByNotifyWrite(keys.notify("demo"))).toBe(true);
    expect(refreshedByNotifyWrite(keys.notify("other"))).toBe(true);
    expect(refreshedByNotifyWrite(keys.notifySends("demo", "nt-1"))).toBe(true);
    expect(refreshedByNotifyWrite(keys.needsYou("demo"))).toBe(true);
    expect(refreshedByNotifyWrite(keys.board("demo"))).toBe(false);
  });
});
