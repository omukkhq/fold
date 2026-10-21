import type {
  DraftCard,
  RequestComment,
  RequestDetail,
  RequestDraft,
  RequestMessage,
  RequestSummary,
} from "@/api/requests";

export const T0 = 1_760_000_000_000;

/** A request row as the list route sends it: open, unclaimed and unsorted, created by tanim. */
export function requestSummary(
  id: string,
  fields: Partial<RequestSummary> = {},
): RequestSummary {
  return {
    id,
    title: null,
    excerpt: `Message of ${id}`,
    kind: null,
    state: "open",
    requester: "human:tanim",
    drafter: null,
    latest_version: null,
    created_at: T0,
    updated_at: T0,
    ...fields,
  };
}

export function card(key: string, title = `Card ${key}`): DraftCard {
  return { key, title, description: `**Scope** the work of ${key}` };
}

export function requestDraft(
  version: number,
  fields: Partial<RequestDraft> = {},
): RequestDraft {
  return {
    version,
    author: "backend",
    cards: [card("c1"), card("c2")],
    edges: [{ dependent: "c2", prerequisite: "c1" }],
    note: null,
    created_at: T0 + version * 60_000,
    ...fields,
  };
}

export function requestComment(
  version: number,
  body: string,
  fields: Partial<RequestComment> = {},
): RequestComment {
  return {
    version,
    body,
    author: "human:tanim",
    created_at: T0 + version * 90_000,
    ...fields,
  };
}

export function requestDetail(
  request: RequestSummary,
  fields: Partial<Omit<RequestDetail, "request">> = {},
): RequestDetail {
  return {
    request,
    body: request.excerpt,
    attachments: [],
    close_reason: null,
    comments: [],
    drafts: [],
    tasks: [],
    thread: null,
    ...fields,
  };
}

export function requestMessage(
  n: number,
  sender: string,
  to: string,
  body: string,
): RequestMessage {
  return {
    id: `m-${String(n)}`,
    thread: `m-${String(1)}`,
    reply_to: null,
    sender,
    task: null,
    proposal: null,
    request: "RQ-4",
    choices: [],
    header: null,
    asks_human: false,
    body,
    sent_at: T0 + n * 1000,
    recipients: [
      {
        name: to,
        addressed_as: "agent",
        delivered_at: null,
        acknowledged_at: null,
        reachable: null,
      },
    ],
  };
}
