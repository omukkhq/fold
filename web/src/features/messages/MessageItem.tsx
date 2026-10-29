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
import { QuotedLines } from "./QuotedLines";
import { Receipts } from "./Receipts";
import { answeredPairs, listedQuestions, type AnsweredPair } from "./sets";

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
  /** The open set this message asks: `shown` on the card below, `queued` behind it; null for any other message. */
  docked?: "shown" | "queued" | null;
  /** Moves the focus to the card that shows this set. */
  onDock?: () => void;
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

/** Each question of an answered set with the answer the reply gave it. */
function AnsweredPairs({ pairs }: { pairs: AnsweredPair[] }) {
  return (
    <dl className="flex flex-col gap-8">
      {pairs.map((pair, i) => (
        <div key={`${String(i)} ${pair.question}`} className="flex flex-col">
          <dt className="text-14 text-text-2">{pair.question}</dt>
          <dd className="text-15 font-semibold whitespace-pre-wrap text-text">
            {pair.answer}
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** The questions of a closed set under its body, so the timeline keeps what was asked; an open set shows its questions one at a time on the card only. */
function AskedQuestions({ message }: { message: MessageView }) {
  const listed = listedQuestions(message);
  if (listed.length > 0) {
    return (
      <ol className="flex list-decimal flex-col gap-2 pl-20 text-14 text-text-2">
        {listed.map((q, i) => (
          <li key={`${String(i)} ${q.text}`}>{q.text}</li>
        ))}
      </ol>
    );
  }
  if (!isQuestion(message)) return null;
  return (
    <p className="text-14 text-text-2">
      {message.choices.map((c) => c.label).join(" · ")}
    </p>
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
}: MessageItemProps) {
  const pairs = answeredPairs(message, thread);
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
            {pairs !== null ? (
              <AnsweredPairs pairs={pairs} />
            ) : chosen ? (
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
          {docked === "queued" ? (
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
          {docked === "queued" ? (
            <p className="text-14 text-text-2">
              Its questions show once you answer the set below.
            </p>
          ) : pairs !== null ? (
            <AnsweredPairs pairs={pairs} />
          ) : chosen ? (
            <ChosenAnswer body={message.body} own={false} />
          ) : (
            <MessageBody body={message.body} slug={slug} />
          )}
        </div>
        {docked === null ? <AskedQuestions message={message} /> : null}
        {docked === "shown" ? (
          <button
            type="button"
            onClick={onDock}
            className={cn(chipClasses("warn"), "self-start", focusRing)}
          >
            <CircleHelp aria-hidden="true" size={14} />
            Waits for your answer below
          </button>
        ) : null}
        <Receipts message={message} now={now} you={you} />
      </article>
    </li>
  );
}
