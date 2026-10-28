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
  Bot,
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
import { TAB_BY_STATE, TAB_LIST } from "@/components/overlay/tabStyles";
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

const HEAD =
  "px-6 py-9 text-left font-title text-13 leading-tight font-semibold whitespace-nowrap text-text-2 first:pl-16 last:pr-16";
const CELL = "px-6 py-12 align-top first:pl-16 last:pr-16";
const MONO = "font-mono text-13 whitespace-nowrap text-text-2";
const OVERLINE = "font-title text-13 leading-tight font-semibold text-text-2";
const CARD =
  "flex flex-col gap-12 rounded-l border border-line bg-surface p-16";

const VERDICT_LOOK: Record<
  VerdictKind,
  { icon: LucideIcon | null; className: string }
> = {
  approved: { icon: Check, className: "bg-ok-soft text-ok" },
  changes: { icon: null, className: "bg-warn-soft text-warn" },
  rejected: { icon: null, className: "bg-bad-soft text-bad" },
  commented: { icon: null, className: "bg-sunken text-text-2" },
  conflict: { icon: GitMerge, className: "bg-bad-soft text-bad" },
  reviewing: { icon: Loader2, className: "bg-sunken text-text-2" },
  left: { icon: null, className: "bg-sunken text-text-2" },
  none: { icon: null, className: "bg-sunken text-text-2" },
};

function VerdictCell({ row }: { row: MemberRow }) {
  const look = VERDICT_LOOK[row.verdict];
  const Icon = look.icon;
  return (
    <div className="flex flex-col items-start gap-4">
      <span
        className={cn(
          "inline-flex items-center gap-5 rounded-s px-8 py-2 text-13 font-semibold whitespace-nowrap",
          look.className,
        )}
      >
        {Icon ? <Icon aria-hidden="true" size={12} /> : null}
        {VERDICT_TEXT[row.verdict]}
      </span>
      {row.needsHuman === null ? null : (
        <span className="flex items-center gap-4 text-13 text-text-2">
          <UserCheck aria-hidden="true" size={12} className="text-accent" />
          needs
          {row.needsHuman === "" ? (
            " a human"
          ) : (
            <PersonName name={row.needsHuman} className="text-14" />
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
      <span className="font-mono text-13 whitespace-nowrap text-text">
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
      className="flex flex-wrap items-center gap-12"
    >
      <p className="flex min-w-0 flex-1 items-center gap-6 text-14 text-text-2">
        <GitMerge aria-hidden="true" size={14} className="shrink-0" />
        {note.text}
      </p>
      {human && note.canIntegrate ? (
        <Button
          variant="primary"
          pending={integrate.isPending}
          pendingLabel="Integrating…"
          icon={<GitMerge aria-hidden="true" size={15} />}
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
          <nav aria-label="Breadcrumb" className="flex items-center gap-8">
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
              size={13}
              className="text-text-2"
            />
            <span className="font-mono text-14 text-text">{batch.id}</span>
          </nav>
          <h1 className="flex items-baseline gap-10">
            <span className="text-24 leading-tight font-semibold text-text">
              Review batch
            </span>
            <span className="font-mono text-16 text-text-2">{batch.id}</span>
          </h1>
          <p className="text-15 text-text-2">
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
        <RadixTabs.List aria-label="Batch" className={TAB_LIST}>
          <RadixTabs.Trigger
            value="members"
            className={cn(TAB_BY_STATE, focusRing)}
          >
            Members
          </RadixTabs.Trigger>
          <RadixTabs.Trigger
            value="changes"
            className={cn(TAB_BY_STATE, focusRing)}
          >
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
          <div className="flex min-w-0 flex-col gap-16 lg:flex-row lg:items-start">
            <div className="flex min-w-0 flex-1 flex-col gap-16">
              {conflicts.map((proposal) => (
                <div
                  key={proposal}
                  role="alert"
                  className="flex items-start gap-12 rounded-m bg-bad-soft px-16 py-12"
                >
                  <GitMerge
                    aria-hidden="true"
                    size={16}
                    className="mt-2 shrink-0 text-bad"
                  />
                  <div className="flex min-w-0 flex-1 flex-col gap-4">
                    <p className="text-15 font-semibold text-bad">
                      {proposal} conflicts with main
                    </p>
                    <p className="text-14 text-text-2">
                      The other members keep their verdicts. fold resumes the
                      task worker of {proposal} to resolve the conflict, then
                      the reviewer checks the batch again.
                    </p>
                  </div>
                  <HrefLink
                    href={projectPath(slug, "proposals", proposal)}
                    className={cn(buttonClasses("secondary", "md"), focusRing)}
                  >
                    <span className={buttonIconClass("secondary")}>
                      <Diff aria-hidden="true" size={15} />
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
                  <p className="text-14 text-text-2">
                    A member with a newer version shows only what changed after
                    the newest earlier version that got a verdict.
                  </p>
                </div>
              ) : null}
              <div className="overflow-x-auto rounded-l border border-line bg-surface">
                <table className="w-full border-collapse" aria-label="Members">
                  <thead className="bg-bg">
                    <tr className="border-b border-line">
                      {[
                        "Proposal",
                        "Title",
                        "Task",
                        "Author",
                        "Version",
                        "Change",
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
                        className="border-b border-line last:border-b-0"
                      >
                        <td className={cn(CELL, MONO)}>
                          <HrefLink
                            href={detailHref(
                              slug,
                              row.proposal,
                              memberSearch(changes.get(row.proposal)),
                            )}
                            className={cn(
                              "rounded-s underline-offset-2 hover:underline",
                              focusRing,
                            )}
                          >
                            {row.proposal}
                          </HrefLink>
                        </td>
                        <td
                          className={cn(CELL, "w-full min-w-(--width-column)")}
                        >
                          <span className="text-15 font-medium text-text">
                            {row.title ?? row.proposal}
                          </span>
                        </td>
                        <td className={cn(CELL, MONO)}>{row.task}</td>
                        <td className={CELL}>
                          {row.author === null ? (
                            <span className="text-13 text-text-2">-</span>
                          ) : (
                            <PersonName name={row.author} slot="authorCell" />
                          )}
                        </td>
                        <td className={cn(CELL, MONO)}>{row.version}</td>
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
                <h2 className="flex items-center gap-8">
                  <GitMerge
                    aria-hidden="true"
                    size={15}
                    className="text-text-2"
                  />
                  <span className={OVERLINE}>Batch checkout</span>
                </h2>
                <p className="flex flex-wrap items-center gap-8">
                  <StatePill pill={checkout} />
                  {checkout.line ? (
                    <span className="font-mono text-13 text-text">
                      {checkout.line}
                    </span>
                  ) : null}
                </p>
                <p className="text-14 text-text-2">
                  The checkout holds main with every proposal of the batch
                  applied, so the reviewer runs the tests on the whole.
                </p>
                {checkout.path ? (
                  <p className="flex items-center gap-8 rounded-m bg-sunken px-10 py-7">
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
                  <h2 className="flex items-center gap-8">
                    <Bot aria-hidden="true" size={15} className="text-text-2" />
                    <span className={OVERLINE}>Reviewer</span>
                  </h2>
                  <div className="flex items-center justify-between gap-8">
                    <div className="flex flex-col gap-2">
                      <PersonName name={reviewer.name} slot="panelName" />
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
