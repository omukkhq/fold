import type { MessageView, RecipientView, ThreadView } from "@/api/messages";

export function recipient(
  name: string,
  fields: Partial<RecipientView> = {},
): RecipientView {
  return {
    name,
    addressed_as: "agent",
    delivered_at: null,
    acknowledged_at: null,
    ...fields,
  };
}

export function message(
  id: string,
  fields: Partial<MessageView> = {},
): MessageView {
  return {
    id,
    thread: id,
    sender: "frontend/ui",
    recipients: [],
    body: "",
    sent_at: 0,
    reply_to: null,
    task: null,
    proposal: null,
    request: null,
    ...fields,
  };
}

export function thread(
  id: string,
  fields: Partial<ThreadView> = {},
): ThreadView {
  return {
    id,
    task: null,
    proposal: null,
    request: null,
    participants: ["human:tanim", "frontend"],
    awaiting_human: false,
    last_at: 0,
    messages: [],
    ...fields,
  };
}
