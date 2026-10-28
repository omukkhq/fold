import { Bot, CircleHelp } from "lucide-react";
import type { MessageView, ThreadView } from "@/api/messages";
import { formatClock } from "@/format/time";
import { cn } from "@/lib/cn";
import { ActorText } from "@/people/ActorText";
import { bareName } from "@/people/names";
import { MessageBody } from "./MessageBody";
import { initial, messageSide, recipientsPhrase } from "./messageLayout";
import { isQuestion } from "./question";
import { QuestionChoices } from "./QuestionChoices";
import { QuotedLines } from "./QuotedLines";
import { Receipts } from "./Receipts";
import { freeAskWaitsForYou } from "./replyTarget";

interface MessageItemProps {
  slug: string;
  thread: ThreadView;
  message: MessageView;
  now: number;
  you: string | null;
  /** The message answers a question by naming one of its options. */
  chosen: boolean;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onWriteOwn: (question: MessageView) => void;
}

function Avatar({ sender, small }: { sender: string; small: boolean }) {
  const agent = messageSide(sender, null) === "agent";
  return (
    <span
      aria-hidden="true"
      className={cn(
        "grid shrink-0 place-items-center font-bold",
        small ? "size-24 text-13" : "size-28 text-13",
        agent
          ? "rounded-m bg-warn-soft text-warn"
          : "rounded-full bg-accent-soft text-accent",
      )}
    >
      {agent ? <Bot size={small ? 13 : 16} /> : initial(sender)}
    </span>
  );
}

function ChosenAnswer({ body, own }: { body: string; own: boolean }) {
  return (
    <div className="flex flex-col gap-2">
      <p className="text-15 font-semibold text-text">{body.trim()}</p>
      <p className="text-13 text-text-2">
        {own
          ? "Your answer to the question above"
          : "Chosen option of the question above"}
      </p>
    </div>
  );
}

/** One message of a thread: the signed-in human's own as a bubble on the right, any other sender's on the left under its name. */
export function MessageItem({
  slug,
  thread,
  message,
  now,
  you,
  chosen,
  onReply,
  onWriteOwn,
}: MessageItemProps) {
  const side = messageSide(message.sender, you);
  const time = (
    <time
      dateTime={new Date(message.sent_at).toISOString()}
      className="text-13 text-text-2"
    >
      {formatClock(message.sent_at)}
    </time>
  );
  const lines =
    message.lines === null ? null : (
      <QuotedLines
        slug={slug}
        proposal={message.proposal}
        lines={message.lines}
      />
    );

  if (side === "own") {
    return (
      <li className="flex justify-end pl-40">
        <article className="flex min-w-0 max-w-measure flex-col items-end gap-6">
          <p className="flex items-baseline gap-8">
            {time}
            <span className="text-15 font-semibold text-accent">You</span>
          </p>
          {lines}
          <div className="max-w-full rounded-pill rounded-br-s bg-accent-soft px-14 py-10 inset-ring inset-ring-accent">
            {chosen ? (
              <ChosenAnswer body={message.body} own />
            ) : (
              <MessageBody body={message.body} slug={slug} />
            )}
          </div>
          <Receipts message={message} now={now} you={you} align="end" />
        </article>
      </li>
    );
  }

  return (
    <li className="flex gap-12">
      <span className="hidden sm:flex">
        <Avatar sender={message.sender} small={false} />
      </span>
      <article className="flex min-w-0 flex-1 flex-col gap-6">
        <p className="flex flex-wrap items-center gap-x-8 gap-y-2">
          <span className="sm:hidden">
            <Avatar sender={message.sender} small />
          </span>
          <span
            className={cn(
              "text-15 font-semibold",
              side === "agent" ? "text-text" : "text-accent",
            )}
          >
            {bareName(message.sender)}
          </span>
          {message.recipients.length > 0 ? (
            <span className="hidden text-13 text-text-2 sm:inline">
              <ActorText segments={recipientsPhrase(message, you)} you={you} />
            </span>
          ) : null}
          {freeAskWaitsForYou(thread, message, you) ? (
            <span className="inline-flex items-center gap-5 rounded-pill bg-warn-soft px-8 py-2 text-13 font-semibold text-warn">
              <CircleHelp aria-hidden="true" size={12} strokeWidth={2.2} />
              Waits for your answer
            </span>
          ) : null}
          <span className="ml-auto">{time}</span>
        </p>
        {lines}
        <div className="max-w-measure">
          {chosen ? (
            <ChosenAnswer body={message.body} own={false} />
          ) : (
            <MessageBody body={message.body} slug={slug} />
          )}
        </div>
        {isQuestion(message) ? (
          <QuestionChoices
            thread={thread}
            question={message}
            you={you}
            onReply={onReply}
            onWriteOwn={() => {
              onWriteOwn(message);
            }}
          />
        ) : null}
        <Receipts message={message} now={now} you={you} />
      </article>
    </li>
  );
}
