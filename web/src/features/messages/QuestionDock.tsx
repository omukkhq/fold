import { ChevronLeft, ChevronRight, CircleHelp } from "lucide-react";
import { IconButton } from "@/components/button/Button";
import { Id } from "@/components/id/Id";
import { Tooltip } from "@/components/overlay/Tooltip";
import type { MessageView } from "@/api/messages";
import { formatClock } from "@/format/time";
import { PersonName } from "@/people/PersonName";
import { idHref } from "@/routes/paths";
import { messageContext } from "./chats";
import { MessageBody } from "./MessageBody";
import { isQuestion } from "./question";
import { QuestionChoices } from "./QuestionChoices";
import type { OpenQuestion } from "./questionQueue";
import { QuotedLines } from "./QuotedLines";

interface QuestionDockProps {
  slug: string;
  queue: OpenQuestion[];
  index: number;
  step: string;
  you: string | null;
  onStep: (index: number) => void;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onWriteOwn: (question: MessageView) => void;
}

/**
 * The asks that wait for the signed-in human, one at a time above the reply box with the step they are at. An answer goes to the question's own thread; once the daemon has stored it, the question leaves the queue and the next one shows.
 */
export function QuestionDock({
  slug,
  queue,
  index,
  step,
  you,
  onStep,
  onReply,
  onWriteOwn,
}: QuestionDockProps) {
  const current = queue[index];
  if (current === undefined) return null;
  const { message, thread } = current;
  const context = messageContext(message, thread);
  const many = queue.length > 1;
  return (
    <section
      aria-label="Open questions"
      className="flex max-h-1/2 shrink-0 flex-col overflow-y-auto px-12 pt-6 lg:px-28 lg:pt-8"
    >
      <div className="flex w-full max-w-thread-column flex-col gap-8 rounded-l bg-surface px-14 py-10 sm:px-16 sm:py-12">
        <div className="flex flex-wrap items-center gap-x-8 gap-y-2">
          <CircleHelp
            aria-hidden="true"
            size={16}
            className="shrink-0 text-warn"
          />
          <h3 className="font-title text-15 leading-tight font-semibold text-warn">
            {step}
          </h3>
          <span className="text-13 text-text-2">
            from <PersonName name={message.sender} />
          </span>
          {context === null ? null : (
            <Id
              href={idHref(slug, context.kind, context.id)}
              className="text-13 text-text-2"
            >
              {context.id}
            </Id>
          )}
          <time
            dateTime={new Date(message.sent_at).toISOString()}
            className="text-13 text-text-2"
          >
            {formatClock(message.sent_at)}
          </time>
          {many ? (
            <span className="ml-auto flex items-center gap-4">
              <Tooltip content="Previous question">
                <IconButton
                  aria-label="Previous question"
                  variant="ghost"
                  size="sm"
                  disabled={index === 0}
                  onClick={() => {
                    onStep(index - 1);
                  }}
                >
                  <ChevronLeft aria-hidden="true" size={16} />
                </IconButton>
              </Tooltip>
              <Tooltip content="Next question">
                <IconButton
                  aria-label="Next question"
                  variant="ghost"
                  size="sm"
                  disabled={index === queue.length - 1}
                  onClick={() => {
                    onStep(index + 1);
                  }}
                >
                  <ChevronRight aria-hidden="true" size={16} />
                </IconButton>
              </Tooltip>
            </span>
          ) : null}
        </div>
        {message.lines === null ? null : (
          <QuotedLines
            slug={slug}
            proposal={message.proposal}
            lines={message.lines}
          />
        )}
        <div className="max-w-measure">
          <MessageBody body={message.body} slug={slug} />
        </div>
        {isQuestion(message) ? (
          <QuestionChoices
            key={message.id}
            thread={thread}
            question={message}
            you={you}
            onReply={onReply}
            onWriteOwn={() => {
              onWriteOwn(message);
            }}
          />
        ) : (
          <p className="text-13 text-text-2">
            Write your answer in the reply box below.
          </p>
        )}
      </div>
    </section>
  );
}
