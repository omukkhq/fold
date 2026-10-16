import { zodResolver } from "@hookform/resolvers/zod";
import { Loader2, Send } from "lucide-react";
import { useState } from "react";
import { useForm, useWatch } from "react-hook-form";
import type { MessageView, ThreadView } from "@/api/messages";
import { apiErrorCode } from "@/api/errors";
import { Button } from "@/components/button/Button";
import { controlClasses } from "@/components/form/Field";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { replySchema, type ReplyValues } from "./compose";
import { answerPending, questionState } from "./question";

/** The badge above a question, waiting or answered. */
export function QuestionBadge({
  thread,
  question,
}: {
  thread: ThreadView;
  question: MessageView;
}) {
  const answered = questionState(thread, question).answer !== null;
  return (
    <span
      className={cn(
        "self-start rounded-s px-8 py-2 text-meta leading-normal font-semibold",
        answered
          ? "bg-success-soft text-success"
          : "bg-warning-soft text-warning",
      )}
    >
      {answered ? "Question · answered" : "Question · waiting for an answer"}
    </span>
  );
}

/** The question's short header, as Claude Code's `AskUserQuestion` draws it above the question. */
export function QuestionHeader({ header }: { header: string }) {
  return (
    <span className="self-start rounded-s bg-sunken px-8 py-2 text-label font-medium text-text-muted inset-ring inset-ring-border">
      {header}
    </span>
  );
}

/**
 * The options of an agent's question as cards a human picks, and a free answer. An answer is a reply that names the option; it stays pending until the thread shows it.
 */
export function QuestionChoices({
  thread,
  question,
  onReply,
}: {
  thread: ThreadView;
  question: MessageView;
  onReply: (replyTo: string, body: string) => Promise<void>;
}) {
  const state = questionState(thread, question);
  const [sending, setSending] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);
  const { register, handleSubmit, reset, control } = useForm<ReplyValues>({
    resolver: zodResolver(replySchema),
    defaultValues: { body: "" },
  });
  const text = useWatch({ control, name: "body" });
  const pending = answerPending(sending !== null, sent, state);
  const answered = state.answer !== null;

  const answer = async (body: string) => {
    setSending(body);
    setFailure(null);
    try {
      await onReply(question.id, body);
      setSent(true);
      reset();
    } catch (error) {
      setFailure(error);
    } finally {
      setSending(null);
    }
  };

  return (
    <div className="flex flex-col gap-10">
      <ul aria-label="Choices" className="flex flex-col gap-8">
        {question.choices.map(({ label, description }) => {
          const chosen = state.chosen === label;
          return (
            <li key={label}>
              <button
                type="button"
                disabled={answered || pending}
                aria-pressed={chosen}
                onClick={() => {
                  void answer(label);
                }}
                className={cn(
                  "flex w-full flex-col items-start gap-2 rounded-m bg-surface px-12 py-8 text-left inset-ring inset-ring-border enabled:hover:bg-sunken disabled:cursor-default",
                  chosen && "bg-primary-soft inset-ring-2 inset-ring-ink",
                  answered && !chosen && "opacity-55",
                  focusRing,
                )}
              >
                <span className="flex items-center gap-8 text-body font-semibold text-text">
                  {chosen ? `${label} · chosen` : label}
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
                  <span className="text-code text-text-muted">
                    {description}
                  </span>
                )}
              </button>
            </li>
          );
        })}
      </ul>
      {answered ? null : (
        <form
          className="flex items-center gap-8"
          onSubmit={(event) => {
            void handleSubmit((values) => answer(values.body.trim()))(event);
          }}
        >
          <input
            aria-label="Your own answer"
            placeholder="Or write your own answer…"
            readOnly={pending}
            className={cn(controlClasses, focusRing, "py-7")}
            {...register("body")}
          />
          <Button
            type="submit"
            pending={
              sending !== null &&
              !question.choices.some((choice) => choice.label === sending)
            }
            disabled={pending || text.trim() === ""}
            icon={<Send aria-hidden="true" size={13} />}
          >
            Answer
          </Button>
        </form>
      )}
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
