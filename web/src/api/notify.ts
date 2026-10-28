import { api } from "./client";
import { expectOk, unwrap } from "./result";
import type { components } from "./schema";

export type NotifyTarget = components["schemas"]["NotifyTargetEntry"];
export type NotifyTargetAdded = components["schemas"]["NotifyTargetAdded"];
export type NewNotifyTarget = components["schemas"]["NewNotifyTargetBody"];
export type NotifyTargetFields = components["schemas"]["NotifyTargetFields"];
export type NotifyEventName = components["schemas"]["NotifyEventName"];
export type NotifyPreset = components["schemas"]["NotifyPreset"];
export type NotifyTargetKind = components["schemas"]["NotifyTargetKind"];
export type NotifyTargetState = components["schemas"]["NotifyTargetState"];
export type NotifySend = components["schemas"]["NotifySendEntry"];
export type ProjectNotify = components["schemas"]["ProjectNotify"];
export type HostNotify = components["schemas"]["HostNotify"];
export type BoardColumn = components["schemas"]["TaskStatus"];

/** Whose target: the host's, which every project that uses the host targets sends to, or the project's. */
export type NotifyScope = { kind: "host" } | { kind: "project"; slug: string };

/** The project's targets and the host's. Rejects with `project_not_found` or `project_unavailable`. */
export async function getProjectNotify(slug: string): Promise<ProjectNotify> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/notify", { params: { path: { slug } } }),
  );
}

export async function getHostNotify(): Promise<HostNotify> {
  return unwrap(api.GET("/api/v1/notify"));
}

/** Sets the web address that links in notices start with; null sends notices without links. */
export async function setWebUrl(webUrl: string | null): Promise<HostNotify> {
  return unwrap(api.PUT("/api/v1/notify", { body: { web_url: webUrl } }));
}

/** Sets whether the host's targets get the project's notices too. */
export async function setUseHostTargets(
  slug: string,
  useHostTargets: boolean,
): Promise<ProjectNotify> {
  return unwrap(
    api.PUT("/api/v1/projects/{slug}/notify", {
      params: { path: { slug } },
      body: { use_host_targets: useHostTargets },
    }),
  );
}

/** The answer holds the signing secret of a generic target, which the daemon shows only here. Rejects with `invalid_notify_target` or `plaintext_notify_target` for a field it refuses. */
export async function addNotifyTarget(
  scope: NotifyScope,
  body: NewNotifyTarget,
): Promise<NotifyTargetAdded> {
  return scope.kind === "host"
    ? unwrap(api.POST("/api/v1/notify/targets", { body }))
    : unwrap(
        api.POST("/api/v1/projects/{slug}/notify/targets", {
          params: { path: { slug: scope.slug } },
          body,
        }),
      );
}

export async function editNotifyTarget(
  scope: NotifyScope,
  id: string,
  body: NotifyTargetFields,
): Promise<NotifyTarget> {
  return scope.kind === "host"
    ? unwrap(
        api.PUT("/api/v1/notify/targets/{id}", {
          params: { path: { id } },
          body,
        }),
      )
    : unwrap(
        api.PUT("/api/v1/projects/{slug}/notify/targets/{id}", {
          params: { path: { slug: scope.slug, id } },
          body,
        }),
      );
}

export async function removeNotifyTarget(
  scope: NotifyScope,
  id: string,
): Promise<void> {
  return scope.kind === "host"
    ? expectOk(
        api.DELETE("/api/v1/notify/targets/{id}", {
          params: { path: { id } },
        }),
      )
    : expectOk(
        api.DELETE("/api/v1/projects/{slug}/notify/targets/{id}", {
          params: { path: { slug: scope.slug, id } },
        }),
      );
}

/** A new signing secret, shown only in the answer. */
export async function rotateNotifyTarget(
  scope: NotifyScope,
  id: string,
): Promise<NotifyTargetAdded> {
  return scope.kind === "host"
    ? unwrap(
        api.POST("/api/v1/notify/targets/{id}/rotate", {
          params: { path: { id } },
        }),
      )
    : unwrap(
        api.POST("/api/v1/projects/{slug}/notify/targets/{id}/rotate", {
          params: { path: { slug: scope.slug, id } },
        }),
      );
}

/** Resolves with the send once the receiver took the test; rejects with `notify_send_failed`, whose message says how it answered. */
export async function testNotifyTarget(
  slug: string,
  id: string,
): Promise<NotifySend> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/notify/targets/{id}/test", {
      params: { path: { slug, id } },
    }),
  );
}

/** The newest sends of a target, newest first. */
export async function listNotifySends(
  slug: string,
  id: string,
): Promise<NotifySend[]> {
  const list = await unwrap(
    api.GET("/api/v1/projects/{slug}/notify/targets/{id}/sends", {
      params: { path: { slug, id } },
    }),
  );
  return list.sends;
}

/** Sends a finished send again with its webhook id. */
export async function sendNotifyAgain(
  slug: string,
  send: string,
): Promise<NotifySend> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/notify/sends/{id}/again", {
      params: { path: { slug, id: send } },
    }),
  );
}
