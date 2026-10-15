import type {
  MessageView,
  NewMessage,
  SentMessage,
  ThreadView,
} from "../src/api/messages";
import { ApiError } from "../src/api/errors";

const NOW = Date.now();
const MIN = 60 * 1000;

function message(
  id: number,
  thread: number,
  sender: string,
  to: string[],
  body: string,
  ago: number,
  fields: Partial<MessageView> & { acked?: number } = {},
): MessageView {
  const { acked, ...rest } = fields;
  return {
    id: `m-${String(id)}`,
    thread: `m-${String(thread)}`,
    sender,
    recipients: to.map((name) => ({
      name,
      delivered_at: NOW - ago + 5000,
      acknowledged_at: acked === undefined ? null : NOW - acked,
    })),
    body,
    sent_at: NOW - ago,
    reply_to: null,
    ...rest,
  };
}

export const threads: ThreadView[] = [
  {
    id: "m-31",
    subject: "Error copy on the login form",
    task: "T-12",
    proposal: null,
    participants: ["human:tanim", "frontend/ui"],
    unread: false,
    last_at: NOW - 3 * MIN,
    messages: [
      message(
        31,
        31,
        "human:tanim",
        ["frontend/ui"],
        "Keep the error text short and neutral. Please also read the <b>style note</b> in T-11.",
        20 * MIN,
        { acked: 18 * MIN },
      ),
      message(
        32,
        31,
        "frontend/ui",
        ["human:tanim"],
        'Using the short versions now. "Wrong email or password" and "Too many attempts, try again in a minute". See P-3 and r17.',
        3 * MIN,
        { reply_to: "m-31", acked: 2 * MIN },
      ),
    ],
  },
  {
    id: "m-35",
    subject: "Which chart library for the dashboard?",
    task: "T-21",
    proposal: null,
    participants: ["human:tanim", "frontend"],
    unread: true,
    last_at: NOW - 12 * MIN,
    messages: [
      message(
        35,
        35,
        "frontend",
        ["human:tanim"],
        "Recharts or plain SVG? Bundle size matters.",
        12 * MIN,
      ),
    ],
  },
  {
    id: "m-28",
    subject: "Seed data scope",
    task: "T-16",
    proposal: "P-4",
    participants: ["human:tanim", "backend/db"],
    unread: false,
    last_at: NOW - 27 * MIN,
    messages: [
      message(
        28,
        28,
        "human:tanim",
        ["backend/db"],
        "Only two users and a month of reports.",
        27 * MIN,
        { acked: 26 * MIN },
      ),
    ],
  },
];

const ROUTES: Record<string, string> = {
  "T-12": "frontend/ui",
  "T-14": "backend/api",
  "T-16": "backend/db",
};

let next = 40;

/** Stands in for the daemon: it picks the recipients, stores the message and clears the unread mark of a thread that a human answers. */
export function send(input: NewMessage): SentMessage {
  const id = next;
  next += 1;
  if (input.reply_to !== undefined) {
    const thread = threads.find((t) =>
      t.messages.some((m) => m.id === input.reply_to),
    );
    if (!thread) {
      throw new ApiError(
        "message_not_found",
        `no message is ${input.reply_to}`,
      );
    }
    const to = thread.participants.filter((p) => !p.startsWith("human:"));
    const sent = message(
      id,
      Number(thread.id.slice(2)),
      "human:tanim",
      to,
      input.body,
      0,
      {
        reply_to: input.reply_to,
      },
    );
    thread.messages.push(sent);
    thread.last_at = sent.sent_at;
    const answered = thread.unread ? [input.reply_to] : [];
    thread.unread = false;
    return { message: sent, answered };
  }
  const to = [ROUTES[input.task] ?? "frontend"];
  const sent = message(id, id, "human:tanim", to, input.body, 0);
  threads.push({
    id: sent.id,
    subject: `Feedback on ${input.task}`,
    task: input.task,
    proposal: null,
    participants: ["human:tanim", ...to],
    unread: false,
    last_at: sent.sent_at,
    messages: [sent],
  });
  return { message: sent, answered: [] };
}
