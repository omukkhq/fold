import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  CircleCheck,
  CircleX,
  FilePen,
  Globe,
  Terminal,
  Wrench,
  X,
  type LucideIcon,
} from "lucide-react";
import { useState, type ReactNode } from "react";
import { keys } from "@/api/keys";
import {
  answerWorkerQuestion,
  type Behavior,
  type WorkerQuestion,
} from "@/api/workers";
import { Button, IconButton } from "@/components/button/Button";
import { textFlow } from "@/components/text/textFlow";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { PersonName } from "@/people/PersonName";
import { ErrorNote } from "./parts";
import {
  answerFailure,
  answerLine,
  inputView,
  whyAsksText,
  shownQuestion,
  toolIcon,
  type ToolIcon,
} from "./permission";
import type { TeamRow } from "./team";

const ICONS: Record<ToolIcon, LucideIcon> = {
  terminal: Terminal,
  file: FilePen,
  globe: Globe,
  wrench: Wrench,
};

const OVERLINE =
  "text-label leading-snug font-semibold tracking-overline text-primary-text uppercase";

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-6">
      <h3 className={OVERLINE}>{title}</h3>
      {children}
    </div>
  );
}

export function PermissionPanel({
  slug,
  id,
  open,
  row,
  human,
  now,
  onClose,
}: {
  slug: string;
  id: string;
  /** The open questions. */
  open: readonly WorkerQuestion[];
  /** The asking worker's row of the team, for its role and what it is bound to. */
  row: TeamRow | null;
  human: string | null;
  now: number;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [answered, setAnswered] = useState<WorkerQuestion | null>(null);
  const answer = useMutation({
    mutationFn: (behavior: Behavior) =>
      answerWorkerQuestion(slug, id, behavior),
    onSuccess: async (question) => {
      setAnswered(question);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.workerQuestions(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.needsYou(slug) }),
      ]);
    },
  });
  const question = shownQuestion(open, answered, id);
  const pending = answer.isPending ? answer.variables : null;

  return (
    <aside
      aria-label="Permission question"
      className="flex w-review-panel shrink-0 flex-col self-start rounded-l border border-border-subtle bg-surface"
    >
      <div className="flex items-center justify-between border-b border-border-subtle py-12 pr-12 pl-20">
        <h2 className="text-section leading-snug font-semibold text-text">
          Permission question
        </h2>
        <IconButton aria-label="Close" size="sm" onClick={onClose}>
          <X aria-hidden="true" size={13} />
        </IconButton>
      </div>
      {question === null ? (
        <p className="px-20 py-18 text-bar text-text-muted">
          This question has no open request any more: it has its answer, or fold
          stopped its worker.
        </p>
      ) : (
        <QuestionBody
          question={question}
          row={row}
          human={human}
          now={now}
          pending={pending}
          onAnswer={(behavior) => {
            answer.mutate(behavior);
          }}
          error={answer.isError ? answer.error : null}
        />
      )}
    </aside>
  );
}

function QuestionBody({
  question,
  row,
  human,
  now,
  pending,
  onAnswer,
  error,
}: {
  question: WorkerQuestion;
  row: TeamRow | null;
  human: string | null;
  now: number;
  pending: Behavior | null;
  onAnswer: (behavior: Behavior) => void;
  error: unknown;
}) {
  const Icon = ICONS[toolIcon(question.tool)];
  const input = inputView(question.input);
  const bound = row?.bound;
  return (
    <>
      <div className="flex flex-col gap-18 px-20 pt-18 pb-20">
        <Section title="Worker">
          <PersonName name={question.worker} slot="panelName" />
          {row ? (
            <p className="flex items-center gap-8">
              <span className="text-bar text-text-muted">{row.role}</span>
              {bound && bound.kind === "ids" ? (
                <span className="font-mono text-meta text-text">
                  {bound.text}
                </span>
              ) : null}
            </p>
          ) : null}
        </Section>
        <Section title="Tool">
          <p className="flex items-center gap-8">
            <Icon aria-hidden="true" size={15} className="text-text-muted" />
            <span className="text-body font-semibold text-text">
              {question.tool}
            </span>
          </p>
        </Section>
        <Section title="Input">
          <div className="flex flex-col gap-4 rounded-m bg-sunken px-12 py-10 inset-ring inset-ring-border">
            <pre
              className={cn("font-mono text-code text-text", textFlow("prose"))}
            >
              {input.text}
            </pre>
            {input.note ? (
              <p className="text-code text-text-muted">{input.note}</p>
            ) : null}
          </div>
        </Section>
        <Section title="Why fold asks">
          <p className="text-bar text-text-muted">
            {whyAsksText(question.tool)}
          </p>
        </Section>
      </div>
      <div className="flex flex-col gap-10 border-t border-border-subtle px-20 py-14">
        {question.answer ? (
          <Answered question={question} now={now} />
        ) : human === null ? (
          <p className="text-bar text-text-muted">
            Tell fold your name first: only a named human allows or denies a
            call.
          </p>
        ) : (
          <>
            <div className="flex items-center gap-8">
              <Button
                variant="primary"
                pending={pending === "allow"}
                pendingLabel="Allowing…"
                disabled={pending !== null}
                icon={<Check aria-hidden="true" size={15} />}
                onClick={() => {
                  onAnswer("allow");
                }}
              >
                Allow
              </Button>
              <Button
                pending={pending === "deny"}
                pendingLabel="Denying…"
                disabled={pending !== null}
                icon={<X aria-hidden="true" size={15} />}
                onClick={() => {
                  onAnswer("deny");
                }}
              >
                Deny
              </Button>
            </div>
            {error ? <ErrorNote failure={answerFailure(error)} /> : null}
            <p className="text-meta text-text-faint">
              {pending === null
                ? "The worker waits until you answer."
                : "fold sends your answer to the worker. The panel updates when the daemon confirms it."}
            </p>
          </>
        )}
      </div>
    </>
  );
}

function Answered({
  question,
  now,
}: {
  question: WorkerQuestion;
  now: number;
}) {
  if (!question.answer) return null;
  const line = answerLine(question.answer);
  const Icon = line.allowed ? CircleCheck : CircleX;
  return (
    <div
      role="status"
      className={cn(
        "flex items-start gap-10 rounded-m px-12 py-10",
        line.allowed ? "bg-success-soft" : "bg-danger-soft",
      )}
    >
      <Icon
        aria-hidden="true"
        size={16}
        className={cn(
          "mt-2 shrink-0",
          line.allowed ? "text-success" : "text-danger",
        )}
      />
      <div className="flex flex-col gap-2">
        <p className="text-bar font-semibold text-text">
          {line.lead}
          <PersonName name={line.by} slot="answeredBy" />
        </p>
        <p className="text-code text-text-muted">
          {formatAsOfMinute(now, question.answer.answered_at)} · {line.after}
        </p>
      </div>
    </div>
  );
}
