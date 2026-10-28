import { Check, CircleCheck, CircleHelp, Loader2, Pencil } from "lucide-react";
import { useState } from "react";
import type { MessageView, ThreadView } from "@/api/messages";
import { apiErrorCode } from "@/api/errors";
import { failureText } from "@/format/failure";
import { formatClock } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { isYou } from "@/people/names";
import { answerPending, questionState } from "./question";
import { asksYou } from "./replyTarget";

const ROW = "flex w-full items-center gap-12 rounded-s px-10 py-10 text-left";

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
      <div className="overflow-hidden rounded-l bg-surface">
        <p
          className={cn(
            "flex flex-wrap items-center gap-x-8 gap-y-2 px-14 py-9 text-14",
            answer === null ? "bg-warn-soft" : "bg-ok-soft",
          )}
        >
          {answer === null ? (
            <>
              <CircleHelp
                aria-hidden="true"
                size={14}
                className="shrink-0 text-warn"
              />
              <span className="font-semibold text-warn">
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
                className="shrink-0 text-ok"
              />
              <span className="font-semibold text-ok">Answered</span>
              <span className="text-text-2">
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
            <span className="ml-auto rounded-s bg-surface px-9 py-2 text-13 font-medium text-text">
              {question.header}
            </span>
          )}
        </p>
        <ul aria-label="Choices" className="flex flex-col gap-2 p-6">
          {question.choices.map(({ label, description }) => {
            const chosen = state.chosen === label;
            return (
              <li key={label}>
                <button
                  type="button"
                  disabled={answer !== null || pending}
                  onClick={() => {
                    void choose(label);
                  }}
                  className={cn(
                    ROW,
                    "min-h-44 enabled:hover:bg-sunken disabled:cursor-default",
                    focusRing,
                  )}
                >
                  {answer === null ? (
                    <span
                      aria-hidden="true"
                      className="size-18 shrink-0 rounded-full inset-ring-2 inset-ring-text-2"
                    />
                  ) : null}
                  <span className="flex min-w-0 flex-1 flex-col items-start gap-2">
                    <span className="flex items-center gap-8 text-15 font-semibold text-text">
                      {label}
                      {sending === label ? (
                        <Loader2
                          aria-label="sending"
                          role="img"
                          size={14}
                          className="animate-spin text-text-2 motion-reduce:animate-none"
                        />
                      ) : null}
                    </span>
                    {description === null ? null : (
                      <span className="text-14 text-text-2">{description}</span>
                    )}
                  </span>
                  {chosen ? (
                    <span className="inline-flex shrink-0 items-center gap-6 text-13 font-semibold text-ok">
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
              "min-h-44 rounded-none px-16 pt-8 pb-12 text-14 font-medium text-accent enabled:hover:underline disabled:cursor-default",
              insetFocusRing,
            )}
          >
            <Pencil aria-hidden="true" size={14} className="shrink-0" />
            Or write your own answer in the reply box below
          </button>
        ) : null}
      </div>
      {pending && sending === null ? (
        <p role="status" className="text-13 text-text-2">
          fold sends your answer. The question shows it when the daemon confirms
          it.
        </p>
      ) : null}
      {failure === null ? null : (
        <p role="alert" className="text-13 font-medium text-bad">
          The answer was not sent. {failureText(failure)}
          {apiErrorCode(failure) ? (
            <span className="font-mono text-text-2">
              {" "}
              code: {apiErrorCode(failure)}
            </span>
          ) : null}
        </p>
      )}
    </div>
  );
}
