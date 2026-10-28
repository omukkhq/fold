import {
  TABLE_CELL,
  TABLE_HEAD,
  TABLE_ROW,
} from "@/components/table/tableStyles";
import { Id } from "@/components/id/Id";
import { IdText } from "@/components/text/IdText";
import { IdLink } from "@/components/id/IdLink";
import {
  skipToken,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import * as RadixTabs from "@radix-ui/react-tabs";
import { useState } from "react";
import {
  Check,
  ChevronRight,
  Copy,
  Diff,
  GitMerge,
  Loader2,
  UserCheck,
  type LucideIcon,
} from "lucide-react";
import { HrefLink } from "@/app/HrefLink";
import { keys } from "@/api/keys";
import type { ComparisonTotals } from "@/api/proposals";
import {
  proposalChangesQuery,
  proposalsQuery,
  reviewBatchQuery,
  sessionQuery,
  workersQuery,
} from "@/api/queries";
import { integrateReviewBatch } from "@/api/reviews";
import type { WorkerProcess } from "@/api/workers";
import { Button, IconButton } from "@/components/button/Button";
import {
  buttonClasses,
  buttonIconClass,
} from "@/components/button/buttonStyles";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { Skeleton } from "@/components/states/Skeleton";
import { detailHref } from "@/features/proposals/view";
import { cn } from "@/lib/cn";
import { focusRing, insetFocusRing } from "@/lib/focus";
import { PersonName } from "@/people/PersonName";
import { StatePill, StepText } from "@/features/workers/parts";
import { StopWorker } from "@/features/workers/StopWorker";
import { stopControl, workerPill, workerStep } from "@/features/workers/team";
import type { WorkerSteps } from "@/live/apply";
import {
  VERDICT_TEXT,
  checkoutView,
  conflictedMembers,
  hasSince,
  memberChanges,
  memberCountText,
  memberRows,
  memberSearch,
  reviewLead,
  reviewerLine,
  workerTotals,
  type ChangeMode,
  type MemberChange,
  type MemberRow,
  type VerdictKind,
} from "./present";
import { integrateFailure, integrationNote } from "./integration";
import {
  batchTab,
  batchTabSearch,
  isBatchTab,
  type BatchSearch,
} from "./merged";
import { MergedChanges } from "./MergedChanges";
import { projectPath } from "@/routes/paths";

const HEAD = cn(TABLE_HEAD, "px-14 pt-10 pb-6 whitespace-nowrap");
const CELL = cn(TABLE_CELL, "px-14 py-12");
const CARD = "flex flex-col gap-10 rounded-m bg-surface px-16 py-14";
/** A tab of the batch's bar, drawn as the proposal page draws its tabs. */
const TAB =
  "inline-flex shrink-0 items-center rounded-s px-12 py-7 font-title text-15 leading-tight font-medium whitespace-nowrap text-text-2 hover:text-text data-[state=active]:bg-sunken data-[state=active]:font-semibold data-[state=active]:text-text";

const VERDICT_LOOK: Record<
  VerdictKind,
  { icon: LucideIcon | null; className: string }
> = {
  approved: { icon: Check, className: "bg-ok-soft text-ok" },
  changes: { icon: null, className: "bg-bad-soft text-bad" },
  rejected: { icon: null, className: "bg-bad-soft text-bad" },
  commented: { icon: null, className: "bg-fill text-text" },
  conflict: { icon: GitMerge, className: "bg-bad-soft text-bad" },
  reviewing: { icon: Loader2, className: "bg-fill text-text" },
  left: { icon: null, className: "bg-fill text-text" },
  none: { icon: null, className: "bg-fill text-text" },
};

function VerdictCell({ row }: { row: MemberRow }) {
  const look = VERDICT_LOOK[row.verdict];
  const Icon = look.icon;
  return (
    <div className="flex flex-col items-start gap-4">
      <span
        className={cn(
          "inline-flex items-center gap-6 rounded-pill px-9 py-2 text-13 leading-tight font-medium whitespace-nowrap",
          look.className,
        )}
      >
        {Icon ? <Icon aria-hidden="true" size={14} /> : null}
        {VERDICT_TEXT[row.verdict]}
      </span>
      {row.needsHuman === null ? null : (
        <span className="flex items-center gap-4 text-13 text-text-2">
          <UserCheck aria-hidden="true" size={14} className="text-text-2" />
          needs
          {row.needsHuman === "" ? (
            " a human"
          ) : (
            <PersonName name={row.needsHuman} />
          )}
        </span>
      )}
    </div>
  );
}

const CHANGE_OPTIONS: { mode: ChangeMode; label: string }[] = [
  { mode: "full", label: "Full change" },
  { mode: "since", label: "Since last review" },
];

function ChangeCell({ change }: { change: MemberChange | undefined }) {
  return (
    <div className="flex flex-col gap-2">
      <span className="text-14 whitespace-nowrap text-text tabular-nums">
        {change?.counts ?? "…"}
      </span>
      <span className="text-13 whitespace-nowrap text-text-2">
        {change?.note}
      </span>
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
  const control = stopControl(worker.state);
  if (control === null) return null;
  return (
    <StopWorker
      slug={slug}
      name={worker.name}
      control={control}
      size="md"
      idleLabel="Stop reviewer"
      busyLabel="Stopping reviewer…"
      className="items-end"
    />
  );
}

/** The batch's integration: where it stands and, for a signed-in human, the action that integrates the ready members together. The action shows its pending state until the daemon answers, and the screen follows the batch's events after. */
function BatchIntegration({
  slug,
  id,
  note,
  human,
}: {
  slug: string;
  id: string;
  note: ReturnType<typeof integrationNote>;
  human: boolean;
}) {
  const queryClient = useQueryClient();
  const integrate = useMutation({
    mutationFn: () => integrateReviewBatch(slug, id),
    onSettled: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.reviewBatches(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.proposals(slug) }),
        queryClient.invalidateQueries({ queryKey: keys.repository(slug) }),
      ]);
    },
  });
  return (
    <section
      aria-label="Integration"
      className="flex flex-wrap items-center gap-12 rounded-m bg-surface py-12 pr-12 pl-16"
    >
      <p className="flex min-w-0 flex-1 items-start gap-10 text-15 text-text">
        <GitMerge
          aria-hidden="true"
          size={16}
          className="mt-4 shrink-0 text-text-2"
        />
        <IdText text={note.text} />
      </p>
      {human && note.canIntegrate ? (
        <Button
          variant="primary"
          pending={integrate.isPending}
          pendingLabel="Integrating…"
          icon={<GitMerge aria-hidden="true" size={16} />}
          onClick={() => {
            integrate.mutate();
          }}
        >
          Integrate the batch
        </Button>
      ) : null}
      {integrate.isError ? (
        <p role="alert" className="w-full text-13 font-medium text-bad">
          {integrateFailure(integrate.error)}
        </p>
      ) : null}
    </section>
  );
}

export interface BatchScreenProps {
  slug: string;
  id: string;
  search: BatchSearch;
  /** A tab change pushes a history entry, so Back returns to the earlier tab; a file on the same tab replaces the entry. */
  onSearch: (next: BatchSearch, history?: "push" | "replace") => void;
}

export function BatchScreen({ slug, id, search, onSearch }: BatchScreenProps) {
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
  const switchable = hasSince(batch);
  const [mode, setMode] = useState<ChangeMode>("since");
  const fullChanges = useQueries({
    queries: batch.members.map((m) =>
      proposalChangesQuery(slug, m.proposal, {
        version: m.current_version,
        since: null,
      }),
    ),
  });
  const full = new Map<string, ComparisonTotals>();
  batch.members.forEach((m, index) => {
    const totals = fullChanges[index]?.data?.totals;
    if (totals) full.set(m.proposal, totals);
  });
  const changes = memberChanges(batch, switchable ? mode : "full", full);
  const conflicts = conflictedMembers(rows);
  const all = workers.data?.workers ?? [];
  const reviewer = all.find((w) => w.name === batch.holder) ?? null;
  const lead = reviewLead(all);
  const checkout = checkoutView(batch);

  return (
    <div className="flex min-w-0 flex-col gap-16 px-16 pt-20 pb-24 md:px-24">
      <div className="flex flex-wrap items-start justify-between gap-16">
        <div className="flex min-w-0 flex-col gap-4">
          <nav aria-label="Breadcrumb" className="flex items-center gap-6">
            <HrefLink
              href={projectPath(slug, "proposals")}
              className={cn(
                "rounded-s text-14 text-text-2 hover:text-text",
                focusRing,
              )}
            >
              Proposals
            </HrefLink>
            <ChevronRight
              aria-hidden="true"
              size={14}
              className="text-text-2"
            />
            <Id aria-current="page" className="text-14 font-medium text-text">
              {batch.id}
            </Id>
          </nav>
          <h1 className="text-24 leading-tight font-semibold text-text">
            Review batch{" "}
            <Id className="font-title font-medium text-text-2">{batch.id}</Id>
          </h1>
          <p className="mt-2 text-14 text-text-2">
            {lead === null ? null : (
              <>
                Grouped by <PersonName name={lead} />
                {" · "}
              </>
            )}
            reviewed by <PersonName name={batch.holder} />
            {" · "}
            {memberCountText(batch)}
          </p>
        </div>
        {/* Only a named human stops a worker. */}
        {reviewer !== null &&
        reviewer.role === "reviewer" &&
        (session.data?.human ?? null) !== null ? (
          <StopReviewer slug={slug} worker={reviewer} />
        ) : null}
      </div>
      <RadixTabs.Root
        value={batchTab(search)}
        onValueChange={(value) => {
          if (isBatchTab(value))
            onSearch(batchTabSearch(search, value), "push");
        }}
        className="flex min-w-0 flex-col gap-16"
      >
        <RadixTabs.List
          aria-label="Batch"
          className="flex gap-2 self-start rounded-m bg-surface p-6"
        >
          <RadixTabs.Trigger value="members" className={cn(TAB, focusRing)}>
            Members
          </RadixTabs.Trigger>
          <RadixTabs.Trigger value="changes" className={cn(TAB, focusRing)}>
            Merged changes
          </RadixTabs.Trigger>
        </RadixTabs.List>
        <RadixTabs.Content value="changes" className={insetFocusRing}>
          <MergedChanges
            slug={slug}
            batch={batch}
            path={search.path}
            onPath={(path) => {
              onSearch({ tab: "changes", path });
            }}
          />
        </RadixTabs.Content>
        <RadixTabs.Content value="members" className={insetFocusRing}>
          <div className="flex min-w-0 flex-col gap-20 lg:flex-row lg:items-start">
            <div className="flex min-w-0 flex-1 flex-col gap-16">
              {conflicts.map((proposal) => (
                <div
                  key={proposal}
                  role="alert"
                  className="flex flex-wrap items-start gap-12 rounded-m bg-bad-soft px-16 py-14"
                >
                  <GitMerge
                    aria-hidden="true"
                    size={16}
                    className="mt-4 shrink-0 text-bad"
                  />
                  <div className="flex min-w-0 flex-1 flex-col gap-2">
                    <p className="text-15 font-semibold text-bad">
                      {proposal} conflicts with main
                    </p>
                    <p className="max-w-measure text-15 text-text">
                      The other members keep their verdicts. fold resumes the
                      task worker of {proposal} to resolve the conflict, then
                      the reviewer checks the batch again.
                    </p>
                  </div>
                  <HrefLink
                    href={projectPath(slug, "proposals", proposal)}
                    className={cn(
                      buttonClasses("quiet", "md"),
                      "ml-auto bg-surface hover:bg-surface",
                      focusRing,
                    )}
                  >
                    <span className={buttonIconClass("quiet")}>
                      <Diff aria-hidden="true" size={16} />
                    </span>
                    Show conflict
                  </HrefLink>
                </div>
              ))}
              {switchable ? (
                <div className="flex flex-wrap items-center gap-12">
                  <div
                    role="group"
                    aria-label="Change"
                    className={SEGMENT_LIST}
                  >
                    {CHANGE_OPTIONS.map((option) => (
                      <button
                        key={option.mode}
                        type="button"
                        aria-pressed={mode === option.mode}
                        onClick={() => {
                          setMode(option.mode);
                        }}
                        className={cn(
                          segmentClasses(mode === option.mode),
                          focusRing,
                        )}
                      >
                        {option.label}
                      </button>
                    ))}
                  </div>
                  <p className="max-w-measure text-14 text-text-2">
                    A member with a newer version shows only what changed after
                    the newest earlier version that got a verdict.
                  </p>
                </div>
              ) : null}
              <div className="overflow-x-auto rounded-m bg-surface">
                <table className="w-full border-collapse" aria-label="Members">
                  <thead>
                    <tr>
                      {["Proposal", "Author", "Change", "Verdict"].map(
                        (head) => (
                          <th
                            key={head}
                            scope="col"
                            className={cn(
                              HEAD,
                              head === "Proposal" && "w-full",
                            )}
                          >
                            {head}
                          </th>
                        ),
                      )}
                    </tr>
                  </thead>
                  <tbody>
                    {rows.map((row) => (
                      <tr key={row.proposal} className={TABLE_ROW}>
                        <td className={cn(CELL, "min-w-(--width-column)")}>
                          <div className="flex min-w-0 flex-col gap-2">
                            <span className="flex min-w-0 items-baseline gap-12">
                              <IdLink
                                className="min-w-32 shrink-0"
                                href={detailHref(
                                  slug,
                                  row.proposal,
                                  memberSearch(changes.get(row.proposal)),
                                )}
                              >
                                {row.proposal}
                              </IdLink>
                              <span className="min-w-0 text-15 font-medium wrap-anywhere text-text">
                                {row.title ?? row.proposal}
                              </span>
                            </span>
                            <span className="pl-44 text-13 text-text-2">
                              for <Id>{row.task}</Id> ·{" "}
                              <span>{row.version}</span>
                            </span>
                          </div>
                        </td>
                        <td className={cn(CELL, "whitespace-nowrap")}>
                          {row.author === null ? (
                            <span className="text-13 text-text-2">-</span>
                          ) : (
                            <PersonName name={row.author} />
                          )}
                        </td>
                        <td className={CELL}>
                          <ChangeCell change={changes.get(row.proposal)} />
                        </td>
                        <td className={CELL}>
                          <VerdictCell row={row} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <BatchIntegration
                slug={slug}
                id={batch.id}
                note={integrationNote(batch)}
                human={(session.data?.human ?? null) !== null}
              />
            </div>
            <div className="flex min-w-0 flex-col gap-16 lg:w-review-panel lg:shrink-0">
              <section aria-label="Batch checkout" className={CARD}>
                <h2 className="text-16 leading-tight font-semibold text-text">
                  Batch checkout
                </h2>
                <p className="flex flex-wrap items-center gap-8">
                  <StatePill pill={checkout} />
                  {checkout.line ? (
                    <span className="text-14 text-text tabular-nums">
                      {checkout.line}
                    </span>
                  ) : null}
                </p>
                <p className="text-15 text-text">
                  The checkout holds main with every proposal of the batch
                  applied, so the reviewer runs the tests on the whole.
                </p>
                {checkout.path ? (
                  <p className="flex items-center gap-8 rounded-s bg-sunken py-6 pr-6 pl-12">
                    <span className="min-w-0 flex-1 truncate font-mono text-13 text-text">
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
                  <h2 className="text-16 leading-tight font-semibold text-text">
                    Reviewer
                  </h2>
                  <div className="flex items-start justify-between gap-8">
                    <div className="flex flex-col gap-2">
                      <PersonName name={reviewer.name} />
                      <span className="text-13 text-text-2">
                        {reviewerLine(reviewer)}
                      </span>
                    </div>
                    <StatePill pill={workerPill(reviewer.state)} />
                  </div>
                  <StepText step={workerStep(reviewer, steps.data ?? {})} />
                  <p className="text-13 text-text-2">
                    {workerTotals(reviewer)}
                  </p>
                </section>
              ) : null}
            </div>
          </div>
        </RadixTabs.Content>
      </RadixTabs.Root>
    </div>
  );
}

export function BatchPending() {
  return (
    <div className="flex min-w-0 flex-col gap-16 px-16 pt-20 pb-24 md:px-24">
      <Skeleton />
      <Skeleton />
    </div>
  );
}
