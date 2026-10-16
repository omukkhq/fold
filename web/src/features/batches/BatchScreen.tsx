import {
  skipToken,
  useMutation,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import {
  Bot,
  Check,
  ChevronRight,
  Copy,
  Diff,
  GitMerge,
  Loader2,
  Square,
  UserCheck,
  type LucideIcon,
} from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { keys } from "@/api/keys";
import {
  proposalsQuery,
  reviewBatchQuery,
  sessionQuery,
  workersQuery,
} from "@/api/queries";
import { stopWorker, type WorkerProcess } from "@/api/workers";
import { Button, IconButton } from "@/components/button/Button";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import { Skeleton } from "@/components/states/Skeleton";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { ErrorNote, StatePill, StepText } from "@/features/workers/parts";
import { stopFailure } from "@/features/workers/startLead";
import { stopControl, workerPill, workerStep } from "@/features/workers/team";
import type { WorkerSteps } from "@/live/apply";
import {
  VERDICT_TEXT,
  checkoutView,
  conflictedMembers,
  memberCountText,
  memberRows,
  reviewLead,
  reviewerLine,
  workerTotals,
  type MemberRow,
  type VerdictKind,
} from "./present";

const HEAD =
  "px-6 py-9 text-left text-label leading-snug font-semibold tracking-overline whitespace-nowrap text-text-muted uppercase first:pl-16 last:pr-16";
const CELL = "px-6 py-12 align-top first:pl-16 last:pr-16";
const MONO = "font-mono text-code whitespace-nowrap text-text-muted";
const OVERLINE =
  "text-label leading-snug font-semibold tracking-overline text-primary-text uppercase";
const CARD =
  "flex flex-col gap-12 rounded-l border border-border-subtle bg-surface p-16";

const VERDICT_LOOK: Record<
  VerdictKind,
  { icon: LucideIcon | null; className: string }
> = {
  approved: { icon: Check, className: "bg-success-soft text-success" },
  changes: { icon: null, className: "bg-warning-soft text-warning" },
  rejected: { icon: null, className: "bg-danger-soft text-danger" },
  commented: { icon: null, className: "bg-sunken text-text-muted" },
  conflict: { icon: GitMerge, className: "bg-danger-soft text-danger" },
  reviewing: { icon: Loader2, className: "bg-sunken text-text-muted" },
  left: { icon: null, className: "bg-sunken text-text-faint" },
  none: { icon: null, className: "bg-sunken text-text-faint" },
};

function VerdictCell({ row }: { row: MemberRow }) {
  const look = VERDICT_LOOK[row.verdict];
  const Icon = look.icon;
  return (
    <div className="flex flex-col items-start gap-4">
      <span
        className={cn(
          "inline-flex items-center gap-5 rounded-s px-8 py-2 text-meta font-semibold whitespace-nowrap",
          look.className,
        )}
      >
        {Icon ? <Icon aria-hidden="true" size={12} /> : null}
        {VERDICT_TEXT[row.verdict]}
      </span>
      {row.needsHuman === null ? null : (
        <span className="flex items-center gap-4 text-meta text-text-muted">
          <UserCheck
            aria-hidden="true"
            size={12}
            className="text-primary-text"
          />
          needs
          {row.needsHuman === "" ? (
            " a human"
          ) : (
            <PersonName name={row.needsHuman} className="text-bar" />
          )}
        </span>
      )}
    </div>
  );
}

function StopReviewer({
  slug,
  worker,
}: {
  slug: string;
  worker: WorkerProcess;
}) {
  const queryClient = useQueryClient();
  const stop = useMutation({
    mutationFn: () => stopWorker(slug, worker.name),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
  });
  const control = stopControl(worker.state);
  if (control === null) return null;
  const pending = stop.isPending || control === "stopping";
  return (
    <div className="flex flex-col items-end gap-6">
      <Button
        pending={pending}
        icon={<Square aria-hidden="true" size={15} />}
        onClick={() => {
          stop.mutate();
        }}
      >
        {pending ? "Stopping reviewer" : "Stop reviewer"}
      </Button>
      {stop.isError ? <ErrorNote failure={stopFailure(stop.error)} /> : null}
    </div>
  );
}

export function BatchScreen({ slug, id }: { slug: string; id: string }) {
  const { data: batch } = useSuspenseQuery(reviewBatchQuery(slug, id));
  const proposals = useQuery(proposalsQuery(slug));
  const workers = useQuery(workersQuery(slug));
  const session = useQuery(sessionQuery());
  // Worker steps are written only by the event stream, so this query observes the cache and never fetches.
  const steps = useQuery<WorkerSteps>({
    queryKey: keys.workerSteps(slug),
    queryFn: skipToken,
  });
  const rows = memberRows(batch, proposals.data ?? []);
  const conflicts = conflictedMembers(rows);
  const all = workers.data?.workers ?? [];
  const reviewer = all.find((w) => w.name === batch.holder) ?? null;
  const lead = reviewLead(all);
  const checkout = checkoutView(batch);

  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <div className="flex items-start justify-between gap-16">
        <div className="flex flex-col gap-4">
          <nav aria-label="Breadcrumb" className="flex items-center gap-8">
            <HrefLink
              href={`/p/${slug}/proposals`}
              className={cn(
                "rounded-s text-bar text-text-muted hover:text-text",
                focusRing,
              )}
            >
              Proposals
            </HrefLink>
            <ChevronRight
              aria-hidden="true"
              size={13}
              className="text-text-faint"
            />
            <span className="font-mono text-bar text-text">{batch.id}</span>
          </nav>
          <h1 className="flex items-baseline gap-10">
            <span className="text-screen leading-snug font-semibold text-text">
              Review batch
            </span>
            <span className="font-mono text-section text-text-muted">
              {batch.id}
            </span>
          </h1>
          <p className="text-body text-text-muted">
            {lead === null ? null : (
              <>
                Grouped by <PersonName name={lead} slot="batchHead" />
                {"  ·  "}
              </>
            )}
            reviewed by <PersonName name={batch.holder} slot="batchHead" />
            {"  ·  "}
            {memberCountText(batch)}
          </p>
        </div>
        {/* Only a named human stops a worker. */}
        {reviewer !== null &&
        reviewer.role === "batch_reviewer" &&
        (session.data?.human ?? null) !== null ? (
          <StopReviewer slug={slug} worker={reviewer} />
        ) : null}
      </div>
      <div className="flex items-start gap-16">
        <div className="flex min-w-0 flex-1 flex-col gap-16">
          {conflicts.map((proposal) => (
            <div
              key={proposal}
              role="alert"
              className="flex items-start gap-12 rounded-m bg-danger-soft px-16 py-12"
            >
              <GitMerge
                aria-hidden="true"
                size={16}
                className="mt-2 shrink-0 text-danger"
              />
              <div className="flex min-w-0 flex-1 flex-col gap-4">
                <p className="text-body font-semibold text-danger">
                  {proposal} conflicts with main
                </p>
                <p className="text-bar text-text-muted">
                  The other members keep their verdicts. fold resumes the coder
                  of {proposal} to resolve the conflict, then the reviewer
                  checks the batch again.
                </p>
              </div>
              <HrefLink
                href={`/p/${slug}/proposals/${proposal}`}
                className={cn(buttonClasses("secondary", "md"), focusRing)}
              >
                <span className={buttonIconClass("secondary")}>
                  <Diff aria-hidden="true" size={15} />
                </span>
                Show conflict
              </HrefLink>
            </div>
          ))}
          <div className="overflow-x-auto rounded-l border border-border-subtle bg-surface">
            <table className="w-full border-collapse" aria-label="Members">
              <thead className="bg-bg">
                <tr className="border-b border-border-subtle">
                  {[
                    "Proposal",
                    "Title",
                    "Task",
                    "Author",
                    "Version",
                    "Verdict",
                  ].map((head) => (
                    <th key={head} scope="col" className={HEAD}>
                      {head}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr
                    key={row.proposal}
                    className="border-b border-border-subtle last:border-b-0"
                  >
                    <td className={cn(CELL, MONO)}>
                      <HrefLink
                        href={`/p/${slug}/proposals/${row.proposal}`}
                        className={cn(
                          "rounded-s underline-offset-2 hover:underline",
                          focusRing,
                        )}
                      >
                        {row.proposal}
                      </HrefLink>
                    </td>
                    <td className={cn(CELL, "w-full")}>
                      <span className="text-body font-medium text-text">
                        {row.title ?? row.proposal}
                      </span>
                    </td>
                    <td className={cn(CELL, MONO)}>{row.task}</td>
                    <td className={CELL}>
                      {row.author === null ? (
                        <span className="text-code text-text-faint">-</span>
                      ) : (
                        <PersonName name={row.author} slot="authorCell" />
                      )}
                    </td>
                    <td className={cn(CELL, MONO)}>{row.version}</td>
                    <td className={CELL}>
                      <VerdictCell row={row} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="flex items-center gap-6 text-bar text-text-muted">
            <GitMerge aria-hidden="true" size={14} />
            fold integrates each member after its approval.
          </p>
        </div>
        <div className="flex w-review-panel shrink-0 flex-col gap-16">
          <section aria-label="Batch checkout" className={CARD}>
            <h2 className="flex items-center gap-8">
              <GitMerge
                aria-hidden="true"
                size={15}
                className="text-text-muted"
              />
              <span className={OVERLINE}>Batch checkout</span>
            </h2>
            <p className="flex flex-wrap items-center gap-8">
              <StatePill pill={checkout} />
              {checkout.line ? (
                <span className="font-mono text-meta text-text">
                  {checkout.line}
                </span>
              ) : null}
            </p>
            <p className="text-bar text-text-muted">
              The checkout holds main with every proposal of the batch applied,
              so the reviewer runs the tests on the whole.
            </p>
            {checkout.path ? (
              <p className="flex items-center gap-8 rounded-m bg-sunken px-10 py-7">
                <span className="min-w-0 flex-1 truncate font-mono text-meta text-text">
                  {checkout.path}
                </span>
                <IconButton
                  aria-label="Copy the path"
                  size="sm"
                  onClick={() => {
                    void navigator.clipboard
                      .writeText(checkout.path ?? "")
                      .catch(() => undefined);
                  }}
                >
                  <Copy aria-hidden="true" size={14} />
                </IconButton>
              </p>
            ) : null}
          </section>
          {reviewer ? (
            <section aria-label="Reviewer" className={CARD}>
              <h2 className="flex items-center gap-8">
                <Bot aria-hidden="true" size={15} className="text-text-muted" />
                <span className={OVERLINE}>Reviewer</span>
              </h2>
              <div className="flex items-center justify-between gap-8">
                <div className="flex flex-col gap-2">
                  <PersonName name={reviewer.name} slot="panelName" />
                  <span className="text-meta text-text-muted">
                    {reviewerLine(reviewer)}
                  </span>
                </div>
                <StatePill pill={workerPill(reviewer.state)} />
              </div>
              <StepText step={workerStep(reviewer, steps.data ?? {})} />
              <p className="text-code text-text-muted">
                {workerTotals(reviewer)}
              </p>
            </section>
          ) : null}
        </div>
      </div>
    </div>
  );
}

export function BatchPending() {
  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <Skeleton />
      <Skeleton />
    </div>
  );
}
