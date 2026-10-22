import { Check, CircleCheck, CircleHelp, Loader2, Pencil } from "lucide-react";
import { useState } from "react";
import type { MessageView, ThreadView } from "@/api/messages";
import { apiErrorCode } from "@/api/errors";
import { failureText } from "@/format/failure";
import { formatClock } from "@/format/time";
import { cn } from "@/lib/cn";
import { insetFocusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { isYou } from "@/people/names";
import { answerPending, questionState } from "./question";
import { asksYou } from "./replyTarget";

const ROW = "flex w-full items-center gap-12 px-14 py-10 text-left";

/**
 * The options of an agent's question as rows of one box a human picks from. A click on a row sends that option as the answer, which stays pending until the thread shows it; the last row moves the human to the reply box for an answer of their own.
 */
export function QuestionChoices({
  thread,
  question,
  you,
  onReply,
  onWriteOwn,
}: {
  thread: ThreadView;
  question: MessageView;
  you: string | null;
  onReply: (replyTo: string, body: string) => Promise<void>;
  onWriteOwn: () => void;
}) {
  const state = questionState(thread, question);
  const [sending, setSending] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);
  const pending = answerPending(sending !== null, sent, state);
  const answer = state.answer;

  const choose = async (label: string) => {
    setSending(label);
    setFailure(null);
    try {
      await onReply(question.id, label);
      setSent(true);
    } catch (error) {
      setFailure(error);
    } finally {
      setSending(null);
    }
  };

  return (
    <div className="flex max-w-measure flex-col gap-6">
      <div className="overflow-hidden rounded-m bg-surface inset-ring inset-ring-border">
        <p
          className={cn(
            "flex flex-wrap items-center gap-x-8 gap-y-2 border-b border-border-subtle px-14 py-9 text-code",
            answer === null ? "bg-warning-soft" : "bg-success-soft",
          )}
        >
          {answer === null ? (
            <>
              <CircleHelp
                aria-hidden="true"
                size={14}
                className="shrink-0 text-warning"
              />
              <span className="font-semibold text-warning">
                {asksYou(question, you)
                  ? "Waits for your answer"
                  : "Waits for an answer"}
              </span>
            </>
          ) : (
            <>
              <CircleCheck
                aria-hidden="true"
                size={14}
                className="shrink-0 text-success-text"
              />
              <span className="font-semibold text-success-text">Answered</span>
              <span className="text-text-muted">
                by{" "}
                {isYou(answer.sender, you) ? (
                  "you"
                ) : (
                  <PersonName name={answer.sender} />
                )}{" "}
                · {formatClock(answer.sent_at)}
              </span>
            </>
          )}
          {question.header === null ? null : (
            <span className="ml-auto rounded-s bg-sunken px-6 py-2 text-label font-medium text-text-muted">
              {question.header}
            </span>
          )}
        </p>
        <ul aria-label="Choices" className="flex flex-col">
          {question.choices.map(({ label, description }) => {
            const chosen = state.chosen === label;
            return (
              <li
                key={label}
                className="border-t border-border-subtle first:border-t-0"
              >
                <button
                  type="button"
                  disabled={answer !== null || pending}
                  onClick={() => {
                    void choose(label);
                  }}
                  className={cn(
                    ROW,
                    "min-h-44 enabled:hover:bg-sunken disabled:cursor-default",
                    insetFocusRing,
                  )}
                >
                  {answer === null ? (
                    <span
                      aria-hidden="true"
                      className="size-16 shrink-0 rounded-full inset-ring-2 inset-ring-border"
                    />
                  ) : null}
                  <span className="flex min-w-0 flex-1 flex-col items-start gap-2">
                    <span className="flex items-center gap-8 text-body font-semibold text-text">
                      {label}
                      {sending === label ? (
                        <Loader2
                          aria-label="sending"
                          role="img"
                          size={13}
                          className="animate-spin text-text-muted motion-reduce:animate-none"
                        />
                      ) : null}
                    </span>
                    {description === null ? null : (
                      <span className="text-bar text-text-muted">
                        {description}
                      </span>
                    )}
                  </span>
                  {chosen ? (
                    <span className="inline-flex shrink-0 items-center gap-6 text-code font-semibold text-success-text">
                      <Check
                        aria-label="chosen"
                        role="img"
                        size={16}
                        strokeWidth={2.4}
                      />
                      <span aria-hidden="true" className="hidden sm:inline">
                        Chosen
                      </span>
                    </span>
                  ) : null}
                </button>
              </li>
            );
          })}
        </ul>
        {answer === null ? (
          <button
            type="button"
            disabled={pending}
            onClick={onWriteOwn}
            className={cn(
              ROW,
              "min-h-44 border-t border-border-subtle text-bar text-primary-text enabled:hover:bg-sunken disabled:cursor-default",
              insetFocusRing,
            )}
          >
            <Pencil aria-hidden="true" size={15} className="shrink-0" />
            Or write your own answer in the reply box below
          </button>
        ) : null}
      </div>
      {pending && sending === null ? (
        <p role="status" className="text-meta text-text-faint">
          fold sends your answer. The question shows it when the daemon confirms
          it.
        </p>
      ) : null}
      {failure === null ? null : (
        <p role="alert" className="text-meta font-medium text-danger">
          The answer was not sent. {failureText(failure)}
          {apiErrorCode(failure) ? (
            <span className="font-mono text-text-muted">
              {" "}
              code: {apiErrorCode(failure)}
            </span>
          ) : null}
        </p>
      )}
    </div>
  );
}
