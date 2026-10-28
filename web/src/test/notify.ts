import type { NotifySend, NotifyTarget } from "@/api/notify";

export function notifyTarget(fields: Partial<NotifyTarget> = {}): NotifyTarget {
  return {
    id: "nt-1",
    name: "Team Slack",
    kind: "slack",
    url: ["https:", "", "hooks.slack.com", "…"].join("/"),
    events: [
      "needs_you.opened",
      "task.moved",
      "task.blocked",
      "review.changes_requested",
      "proposal.integrated",
      "integration.failed",
      "checks.failed",
      "worker.failed",
      "git.failed",
    ],
    preset: "recommended",
    moves_to: ["review", "done"],
    window_seconds: 15,
    enabled: true,
    state: "on",
    failures: 0,
    last_ok_at: null,
    last_failed_at: null,
    next_try_at: null,
    host: false,
    created_at: 1_760_000_000_000,
    created_by: "ana",
    ...fields,
  };
}

export function notifySend(fields: Partial<NotifySend> = {}): NotifySend {
  return {
    id: "ns-1",
    target: "nt-1",
    webhook_id: "demo-1a2b3c4d-1",
    state: "delivered",
    tries: 1,
    next_try_at: null,
    notices: ["task.moved T-18"],
    count: 1,
    test: false,
    created_at: 1_760_000_000_000,
    last_try_at: 1_760_000_000_000,
    status: 200,
    error: null,
    duration_ms: 12,
    ...fields,
  };
}
