import { api } from "./client";
import { expectOk, unwrap } from "./result";
import type { components } from "./schema";

export type NotifyTarget = components["schemas"]["NotifyTargetEntry"];
export type NewNotifyTarget = components["schemas"]["NewNotifyTargetBody"];
export type NotifySecurity = components["schemas"]["NotifySecurity"];
export type ProjectNotify = components["schemas"]["ProjectNotify"];
export type HostNotify = components["schemas"]["HostNotify"];

/** Whose targets: the host's, which every project without targets of its own uses, or the project's. */
export type NotifyScope = { kind: "host" } | { kind: "project"; slug: string };

/** The project's targets, the host's and which of them get its notices. Rejects with `project_not_found` or `project_unavailable`. */
export async function getProjectNotify(slug: string): Promise<ProjectNotify> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/notify", { params: { path: { slug } } }),
  );
}

export async function getHostNotify(): Promise<HostNotify> {
  return unwrap(api.GET("/api/v1/notify"));
}

/** Sets the web address that links in notices start with; null lets them follow the daemon's listen address. */
export async function setWebUrl(webUrl: string | null): Promise<HostNotify> {
  return unwrap(api.PUT("/api/v1/notify", { body: { web_url: webUrl } }));
}

/** Rejects with `invalid_notify_target` for a field the daemon refuses. */
export async function addNotifyTarget(
  scope: NotifyScope,
  body: NewNotifyTarget,
): Promise<NotifyTarget> {
  return scope.kind === "host"
    ? unwrap(api.POST("/api/v1/notify/targets", { body }))
    : unwrap(
        api.POST("/api/v1/projects/{slug}/notify/targets", {
          params: { path: { slug: scope.slug } },
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

/** Resolves once the receiver or SMTP server took the test notice; rejects with `notify_send_failed`, whose message says what failed. */
export async function testNotifyTarget(
  scope: NotifyScope,
  id: string,
): Promise<void> {
  return scope.kind === "host"
    ? expectOk(
        api.POST("/api/v1/notify/targets/{id}/test", {
          params: { path: { id } },
        }),
      )
    : expectOk(
        api.POST("/api/v1/projects/{slug}/notify/targets/{id}/test", {
          params: { path: { slug: scope.slug, id } },
        }),
      );
}
