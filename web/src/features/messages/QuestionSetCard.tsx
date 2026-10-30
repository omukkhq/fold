import { ArrowLeft, Loader2, Pencil } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { apiErrorCode } from "@/api/errors";
import { DISABLED_CLASSES } from "@/components/button/buttonStyles";
import { Id } from "@/components/id/Id";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { idHref } from "@/routes/paths";
import { messageContext } from "./chats";
import { MessageBody } from "./MessageBody";
import {
  alreadyAnswered,
  answerStep,
  canStepBack,
  draftAnswers,
  draftComplete,
  headingQuestion,
  keyChoice,
  moreSetsText,
  ownStart,
  setQuestions,
  shownStep,
  startDraft,
  stepBack,
  stepText,
  type OpenSet,
} from "./sets";

interface QuestionSetCardProps {
  slug: string;
  set: OpenSet;
  /** How many other sets of the chat wait behind this one. */
  more: number;
  /** The question heading, which takes the focus when the card shows the next question, so a screen reader reads it; only the script focuses it, so it draws no focus ring while the choices keep theirs. */
  headingRef: RefObject<HTMLHeadingElement | null>;
  /** Resolves once the daemon has stored the one reply with every answer; rejects with its error. */
  onAnswer: (ask: string, answers: string[]) => Promise<void>;
  /** The person picked the first answer of this set. */
  onStart: () => void;
}

const ROW =
  "flex w-full min-h-44 items-center gap-12 rounded-m px-14 py-10 text-left text-15";

function Key({ children }: { children: string }) {
  return (
    <kbd
      aria-hidden="true"
      className="grid size-22 shrink-0 place-items-center rounded-s bg-surface font-body text-13 text-text-2"
    >
      {children}
    </kbd>
  );
}

/**
 * The oldest open question set of a chat, one question at a time: a choice, its number key or Enter on an own answer shows the next question, and Back returns to change an answer. After the last answer fold sends every answer as one reply, and the card stays pending until the daemon closes the set.
 */
export function QuestionSetCard({
  slug,
  set,
  more,
  headingRef,
  onAnswer,
  onStart,
}: QuestionSetCardProps) {
  const { message, thread } = set;
  const questions = setQuestions(message);
  const [draft, setDraft] = useState(() =>
    startDraft(message.id, questions.length),
  );
  const [writing, setWriting] = useState(false);
  const [own, setOwn] = useState("");
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  const [failure, setFailure] = useState<unknown>(null);
  const ownField = useRef<HTMLInputElement>(null);
  const pending = sending || sent;
  const step = shownStep(draft);
  const question = questions[step];
  const context = messageContext(message, thread);

  const send = async (answers: string[]) => {
    setSending(true);
    setFailure(null);
    try {
      await onAnswer(message.id, answers);
      setSent(true);
    } catch (error) {
      setFailure(error);
    } finally {
      setSending(false);
    }
  };

  const answer = (text: string) => {
    if (pending || alreadyAnswered(failure) || text.trim() === "") return;
    if (draft.step === 0) onStart();
    const next = answerStep(draft, text);
    setDraft(next);
    setWriting(false);
    setOwn("");
    if (draftComplete(next)) {
      void send(draftAnswers(next));
    } else {
      headingRef.current?.focus();
    }
  };

  const back = () => {
    const before = stepBack(draft);
    setDraft(before);
    const earlier = questions[before.step];
    const start = earlier === undefined ? "" : ownStart(before, earlier);
    setOwn(start);
    setWriting(start !== "");
    setFailure(null);
    headingRef.current?.focus();
  };

  useEffect(() => {
    if (writing) ownField.current?.focus();
  }, [writing]);

  if (question === undefined) return null;
  const moreText = moreSetsText(more);
  const chosen = draft.answers[step] ?? null;
  const stale = alreadyAnswered(failure);
  // A set the daemon refused as closed takes no more answers; it leaves once the asks are read again.
  const locked = pending || stale;
  const backShown = !pending && canStepBack(draft, stale);
  return (
    <section
      aria-label="Open question set"
      // A number key picks a choice only while the focus is on the card and not in its field, so no key pressed elsewhere on the page, or on a hidden pane, answers a set.
      onKeyDown={(event) => {
        const index = keyChoice(
          {
            key: event.key,
            altKey: event.altKey,
            ctrlKey: event.ctrlKey,
            metaKey: event.metaKey,
            target: event.target instanceof HTMLElement ? event.target : null,
          },
          question.choices.length,
        );
        const label =
          index === null ? undefined : question.choices[index]?.label;
        if (label === undefined) return;
        event.preventDefault();
        answer(label);
      }}
      className="flex max-h-3/5 shrink-0 flex-col overflow-y-auto px-10 pt-6 lg:px-28 lg:pt-8"
    >
      <div className="flex w-full max-w-thread-column flex-col gap-14 rounded-l bg-surface px-16 pt-18 pb-14 shadow-pop lg:gap-16 lg:px-24 lg:pt-22 lg:pb-18">
        <div className="flex flex-wrap items-center justify-between gap-x-12 gap-y-4">
          <p className="text-13 text-text-2">
            From{" "}
            <span className="font-semibold text-text">
              <PersonName name={message.sender} />
            </span>
            {context === null ? null : (
              <>
                {" · "}
                <Id
                  href={idHref(slug, context.kind, context.id)}
                  className="text-13 text-text-2"
                >
                  {context.id}
                </Id>
              </>
            )}
          </p>
          {questions.length > 1 ? (
            <p className="flex items-center gap-6">
              {questions.map((q, i) => (
                <span
                  key={`${String(i)} ${q.text}`}
                  aria-hidden="true"
                  className={cn(
                    "block h-4 w-22 rounded-s",
                    i < draft.step
                      ? "bg-accent"
                      : i === step
                        ? "bg-text"
                        : "bg-fill",
                  )}
                />
              ))}
              <span className="ml-6 text-13 text-text-2 tabular-nums">
                {stepText(step, questions.length)}
              </span>
            </p>
          ) : null}
        </div>
        <div className="flex flex-col gap-4">
          {question.header === null ? null : (
            <p className="font-title text-13 font-semibold text-text-2">
              {question.header}
            </p>
          )}
          <h3
            ref={headingRef}
            tabIndex={-1}
            aria-label={
              questions.length > 1
                ? `Question ${stepText(step, questions.length)}: ${question.text}`
                : undefined
            }
            className={cn(
              "text-text outline-none",
              headingQuestion(question.text)
                ? "font-title text-18 leading-tight font-semibold lg:text-21"
                : "text-15",
            )}
          >
            {headingQuestion(question.text) ? (
              question.text
            ) : (
              <MessageBody body={question.text} slug={slug} />
            )}
          </h3>
        </div>
        <ul aria-label="Choices" className="flex flex-col gap-6">
          {question.choices.map((choice, i) => (
            <li key={choice.label}>
              <button
                type="button"
                disabled={locked}
                aria-pressed={chosen === choice.label}
                aria-keyshortcuts={String(i + 1)}
                onClick={() => {
                  answer(choice.label);
                }}
                className={cn(
                  ROW,
                  chosen === choice.label ? "bg-fill" : "bg-bg",
                  "enabled:hover:bg-fill",
                  DISABLED_CLASSES,
                  focusRing,
                )}
              >
                <Key>{String(i + 1)}</Key>
                <span className="flex min-w-0 flex-1 flex-col gap-2">
                  <span className="text-text">{choice.label}</span>
                  {choice.description === null ? null : (
                    <span className="text-14 text-text-2">
                      {choice.description}
                    </span>
                  )}
                </span>
              </button>
            </li>
          ))}
          <li>
            {writing ? (
              <form
                className={cn(
                  ROW,
                  "bg-surface outline-2 outline-accent focus-within:outline-2",
                )}
                onSubmit={(event) => {
                  event.preventDefault();
                  answer(own);
                }}
              >
                <Pencil
                  aria-hidden="true"
                  size={14}
                  className="mx-4 shrink-0 text-text-2"
                />
                <input
                  ref={ownField}
                  value={own}
                  disabled={locked}
                  aria-label="Your own answer"
                  placeholder="Write your own answer…"
                  onChange={(event) => {
                    setOwn(event.target.value);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") {
                      setWriting(false);
                      headingRef.current?.focus();
                    }
                  }}
                  className={cn(
                    "min-w-0 flex-1 bg-transparent text-15 text-text outline-none placeholder:text-text-2",
                    DISABLED_CLASSES,
                  )}
                />
                <span className="shrink-0 text-13 text-text-2">Enter</span>
              </form>
            ) : (
              <button
                type="button"
                disabled={locked}
                onClick={() => {
                  setWriting(true);
                }}
                className={cn(
                  ROW,
                  "bg-bg text-text-2 enabled:hover:bg-fill",
                  DISABLED_CLASSES,
                  focusRing,
                )}
              >
                <span className="grid size-22 shrink-0 place-items-center">
                  <Pencil aria-hidden="true" size={14} />
                </span>
                Write your own answer…
              </button>
            )}
          </li>
        </ul>
        {backShown || moreText !== null ? (
          <div className="flex min-h-24 items-center justify-between gap-12 text-13 text-text-2">
            {backShown ? (
              <button
                type="button"
                onClick={back}
                className={cn(
                  "inline-flex items-center gap-6 rounded-s hover:text-text",
                  focusRing,
                )}
              >
                <ArrowLeft aria-hidden="true" size={14} />
                Back
              </button>
            ) : (
              <span />
            )}
            {moreText === null ? null : <span>{moreText}</span>}
          </div>
        ) : null}
        {pending ? (
          <p
            role="status"
            className="flex items-center gap-8 text-13 text-text-2"
          >
            <Loader2
              aria-hidden="true"
              size={14}
              className="animate-spin motion-reduce:animate-none"
            />
            fold sends your answers. The next set shows when the daemon confirms
            them.
          </p>
        ) : null}
        {failure === null ? null : stale ? (
          <p role="alert" className="text-13 font-medium text-text-2">
            This set was already answered.
          </p>
        ) : (
          <p role="alert" className="text-13 font-medium text-bad">
            The answers were not sent. {failureText(failure)}
            {apiErrorCode(failure) ? (
              <span className="font-mono text-text-2">
                {" "}
                code: {apiErrorCode(failure)}
              </span>
            ) : null}{" "}
            {draftComplete(draft) ? (
              <button
                type="button"
                onClick={() => {
                  void send(draftAnswers(draft));
                }}
                className={cn(
                  "rounded-s font-semibold text-accent underline",
                  focusRing,
                )}
              >
                Send again
              </button>
            ) : null}
          </p>
        )}
      </div>
    </section>
  );
}
