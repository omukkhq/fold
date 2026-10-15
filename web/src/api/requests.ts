import { API_BASE, api } from "./client";
import { ApiError, isApiError } from "./errors";
import { unwrap } from "./result";
import type { components } from "./schema";

type Schemas = components["schemas"];

export type RequestKind = Schemas["RequestKind"];
export type RequestState = Schemas["RequestState"];
export type RequestSummary = Schemas["RequestSummary"];
export type RequestDetail = Schemas["RequestDetailView"];
export type RequestDraft = Schemas["RequestDraftView"];
export type DraftCard = Schemas["DraftCardView"];
export type DraftEdge = Schemas["DraftEdgeView"];
export type RequestComment = Schemas["RequestCommentView"];
export type RequestAttachment = Schemas["RequestAttachmentView"];
export type RequestThread = Schemas["ThreadView"];
export type RequestMessage = Schemas["MessageView"];
export type NewDraft = Schemas["EditDraftBody"];

/**
 * A body over the daemon's size limit is refused before any handler runs, as plain text with no error envelope. The web gives that answer its own code so the dialog can branch on it.
 */
export const REQUEST_TOO_LARGE = "request_too_large";

function base(slug: string, id: string) {
  return { path: { slug, id } };
}

/** Every request of the project, newest first; the screen filters them, so the list follows the event stream as one cache entry. */
export async function listRequests(slug: string): Promise<RequestSummary[]> {
  const { items } = await unwrap(
    api.GET("/api/v1/projects/{slug}/requests", {
      params: { path: { slug } },
    }),
  );
  return items;
}

/** The request with its drafts, comments and thread summary; null for a request the project does not have. */
export async function getRequest(
  slug: string,
  id: string,
): Promise<RequestDetail | null> {
  try {
    return await unwrap(
      api.GET("/api/v1/projects/{slug}/requests/{id}", {
        params: base(slug, id),
      }),
    );
  } catch (error) {
    if (isApiError(error, "request_not_found")) return null;
    throw error;
  }
}

/**
 * Creates a request from the message and the dropped files as one multipart form: a `body` part and one `files` part per file, each with its file name.
 * The generated body type describes the parts, not the form, so the form goes through the serializer.
 */
export async function createRequest(
  slug: string,
  body: string,
  files: readonly File[],
): Promise<RequestDetail> {
  const form = new FormData();
  form.append("body", body);
  for (const file of files) form.append("files", file, file.name);
  const answer = await api.POST("/api/v1/projects/{slug}/requests", {
    params: { path: { slug } },
    body: { body, files: files.map((file) => file.name) },
    bodySerializer: () => form,
  });
  if (answer.response.status === 413) {
    throw new ApiError(
      REQUEST_TOO_LARGE,
      "the daemon refused a body larger than its limit",
    );
  }
  return unwrap(Promise.resolve(answer));
}

/** Sends the latest version back to the drafter; `version` is the one the requester read, and an older one is `stale_version`. */
export async function requestDraftChanges(
  slug: string,
  id: string,
  version: number,
  comment: string,
): Promise<RequestDetail> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/requests/{id}/changes", {
      params: base(slug, id),
      body: { version, comment },
    }),
  );
}

/** The requester's own next version of the draft. */
export async function editRequestDraft(
  slug: string,
  id: string,
  draft: NewDraft,
): Promise<RequestDetail> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/requests/{id}/drafts", {
      params: base(slug, id),
      body: draft,
    }),
  );
}

/** Approves the version on screen: the daemon creates one task per card and refuses with `stale_version` when a newer version arrived. */
export async function approveRequestDraft(
  slug: string,
  id: string,
  expectedVersion: number,
): Promise<RequestDetail> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/requests/{id}/approve", {
      params: base(slug, id),
      body: { expected_version: expectedVersion },
    }),
  );
}

export async function closeRequest(
  slug: string,
  id: string,
  reason: string,
): Promise<RequestDetail> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/requests/{id}/close", {
      params: base(slug, id),
      body: { reason },
    }),
  );
}

/** The clarification thread, read by the id of any of its messages. */
export async function getThread(
  slug: string,
  thread: string,
): Promise<RequestThread> {
  return unwrap(
    api.GET("/api/v1/projects/{slug}/threads/{id}", {
      params: { path: { slug, id: thread } },
    }),
  );
}

/** A message in the request's clarification thread. Left without `to`, the daemon sends it to the drafter. It never changes the request. */
export async function sendRequestMessage(
  slug: string,
  request: string,
  body: string,
  to?: readonly string[],
): Promise<RequestMessage> {
  return unwrap(
    api.POST("/api/v1/projects/{slug}/feedback", {
      params: { path: { slug } },
      body: { body, request, ...(to ? { to: [...to] } : {}) },
    }),
  );
}

/** The download address of an attachment. The browser fetches it as a download, so the bytes never pass through the page. */
export function attachmentHref(slug: string, id: string, position: number) {
  const parts = [slug, "requests", id, "attachments", String(position)];
  return `${API_BASE}/projects/${parts.map(encodeURIComponent).join("/")}`;
}
