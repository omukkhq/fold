import { Bot, CircleHelp } from "lucide-react";
import type { MessageView, ThreadView } from "@/api/messages";
import { chipClasses } from "@/components/badge/chips";
import { Id } from "@/components/id/Id";
import { formatClock } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { idHref } from "@/routes/paths";
import { ActorText } from "@/people/ActorText";
import { bareName } from "@/people/names";
import type { MessageContext } from "./chats";
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
  /** The request, proposal or task the message is about, for its chip. */
  context?: MessageContext | null;
  /** The step of the question dock that shows this question, such as `Question 2 of 3`; the dock holds its choices. */
  docked?: string | null;
  /** Shows this question in the dock. */
  onDock?: () => void;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onWriteOwn: (question: MessageView) => void;
}

function ContextChip({
  slug,
  context,
}: {
  slug: string;
  context: MessageContext;
}) {
  return (
    <Id
      href={idHref(slug, context.kind, context.id)}
      className="text-13 text-text-2"
    >
      {context.id}
    </Id>
  );
}

function Avatar({ sender }: { sender: string }) {
  const agent = messageSide(sender, null) === "agent";
  return (
    <span
      aria-hidden="true"
      className={cn(
        "grid size-32 shrink-0 place-items-center bg-accent-soft font-title text-14 font-bold text-accent",
        agent ? "rounded-m" : "rounded-full",
      )}
    >
      {agent ? <Bot size={16} /> : initial(sender)}
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
  context = null,
  docked = null,
  onDock,
  onReply,
  onWriteOwn,
}: MessageItemProps) {
  const side = messageSide(message.sender, you);
  const chip =
    context === null ? null : <ContextChip slug={slug} context={context} />;
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
        onSurface={false}
      />
    );

  if (side === "own") {
    return (
      <li data-message={message.id} className="flex justify-end pl-32 lg:pl-44">
        <article className="flex min-w-0 max-w-measure flex-col items-end gap-6">
          <p className="flex items-baseline gap-8">
            {chip}
            {time}
            <span className="text-15 font-semibold text-text">You</span>
          </p>
          {lines}
          <div className="max-w-full rounded-l rounded-br-s bg-accent-soft px-14 py-10">
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
    <li data-message={message.id} className="flex gap-12">
      <span className="hidden sm:flex">
        <Avatar sender={message.sender} />
      </span>
      <article className="flex min-w-0 flex-1 flex-col gap-6">
        <p className="flex flex-wrap items-baseline gap-x-8 gap-y-2">
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
          {docked === null && freeAskWaitsForYou(thread, message, you) ? (
            <span className="inline-flex items-center gap-5 rounded-pill bg-warn-soft px-8 py-2 text-13 font-semibold text-warn">
              <CircleHelp aria-hidden="true" size={14} />
              Waits for your answer
            </span>
          ) : null}
          <span className="ml-auto flex items-baseline gap-8">
            {chip}
            {time}
          </span>
        </p>
        {lines}
        <div className="max-w-measure">
          {chosen ? (
            <ChosenAnswer body={message.body} own={false} />
          ) : (
            <MessageBody body={message.body} slug={slug} />
          )}
        </div>
        {docked !== null ? (
          <button
            type="button"
            onClick={onDock}
            className={cn(chipClasses("warn"), "self-start", focusRing)}
          >
            <CircleHelp aria-hidden="true" size={14} />
            {docked} waits for your answer below
          </button>
        ) : isQuestion(message) ? (
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
