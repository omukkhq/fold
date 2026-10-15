import { ApiError } from "./errors";
import { fromFixture } from "./fixture";

/** The OpenAPI schema has no `GET /messages` route, so these types are the payload the screen reads; each is declared here once and gives way to the generated type when the route is in the schema. Names starting `human:` are humans. */
export interface RecipientView {
  name: string;
  /** UTC milliseconds; null while undelivered. */
  delivered_at: number | null;
  /** UTC milliseconds; null while unacknowledged. */
  acknowledged_at: number | null;
}

export interface MessageView {
  /** @example m-31 */
  id: string;
  /** The id of the first message of the thread. */
  thread: string;
  sender: string;
  recipients: RecipientView[];
  /** Plain text. */
  body: string;
  /** UTC milliseconds. */
  sent_at: number;
  reply_to: string | null;
}

export interface ThreadView {
  /** The id of the first message. */
  id: string;
  subject: string;
  task: string | null;
  proposal: string | null;
  participants: string[];
  /** True while a question to a human waits for an answer; the daemon clears it. */
  unread: boolean;
  /** UTC milliseconds. */
  last_at: number;
  messages: MessageView[];
}

export type NewMessage = { body: string } & (
  | { reply_to: string; task?: undefined }
  | { task: string; reply_to?: undefined }
);

export interface SentMessage {
  message: MessageView;
  /** Ids of the questions the daemon marked answered by this message. */
  answered: string[];
}

const fixture = import.meta.env.DEV
  ? () => import("../../fixtures/messages")
  : undefined;

export async function listThreads(): Promise<ThreadView[]> {
  return fromFixture(fixture, (m) => m.threads, []);
}

/**
 * A human reply (`reply_to`) or feedback linked to a task. The daemon picks the recipients; the message changes no task state.
 * Rejects with the daemon's codes, such as `message_not_found`; without a fixture it rejects with `not_found`, the daemon's answer to a route it does not serve.
 */
export async function sendMessage(input: NewMessage): Promise<SentMessage> {
  const sent = await fromFixture(fixture, (m) => m.send(input), null);
  if (!sent) {
    throw new ApiError("not_found", "the daemon serves no message route");
  }
  return sent;
}
