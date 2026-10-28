import * as RadixTabs from "@radix-ui/react-tabs";
import { skipToken, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { Bot, Terminal, UserPlus } from "lucide-react";
import { useState } from "react";
import { boardCards, boardQuery } from "@/api/board";
import { keys } from "@/api/keys";
import {
  agentsQuery,
  reviewBatchesQuery,
  sessionQuery,
  workerQuestionsQuery,
  workersQuery,
} from "@/api/queries";
import { labelsQuery } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { TAB_BY_STATE, TAB_LIST } from "@/components/overlay/tabStyles";
import {
  SEGMENT_LIST,
  segmentClasses,
} from "@/components/segments/segmentStyles";
import { Skeleton } from "@/components/states/Skeleton";
import { CostTab } from "@/features/cost/CostTab";
import { COST_RANGES } from "@/features/cost/cost";
import { PermissionPanel } from "@/features/workers/PermissionPanel";
import { StartLeadDialog } from "@/features/workers/StartLeadDialog";
import { TeamTable } from "@/features/workers/TeamTable";
import {
  askingRow,
  hasNoLeads,
  isEmptyTeam,
  mainAgentName,
  teamGroups,
} from "@/features/workers/team";
import type { AgentsSearch } from "@/features/workers/search";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import type { WorkerSteps } from "@/live/apply";
import { useNow } from "@/live/useNow";

const NO_STEPS: WorkerSteps = {};

export function AgentsScreen({
  slug,
  search,
  onSearch,
}: {
  slug: string;
  search: AgentsSearch;
  onSearch: (next: AgentsSearch) => void;
}) {
  const { data: workers } = useSuspenseQuery(workersQuery(slug));
  const { data: agents } = useSuspenseQuery(agentsQuery(slug));
  const now = useNow(true, 30_000);
  const questions = useQuery(workerQuestionsQuery(slug));
  const batches = useQuery(reviewBatchesQuery(slug));
  const board = useQuery(boardQuery(slug));
  const labels = useQuery(labelsQuery(slug));
  const session = useQuery(sessionQuery());
  // Worker steps are written only by the event stream, so this query observes the cache and never fetches.
  const steps = useQuery<WorkerSteps>({
    queryKey: keys.workerSteps(slug),
    queryFn: skipToken,
  });
  const [starting, setStarting] = useState(false);
  const human = session.data?.human ?? null;
  const open = questions.data ?? [];
  const groups = teamGroups({
    workers,
    agents,
    questions: open,
    steps: steps.data ?? NO_STEPS,
    batches: batches.data ?? [],
    cards: board.data ? boardCards(board.data) : [],
    labels: labels.data ?? [],
    human,
  });
  const asking = search.question;
  const tab = search.tab ?? "team";
  const range = search.range ?? "all";
  // Only a named human starts a lead, so the controls wait for a name.
  const startLead =
    human === null
      ? null
      : () => {
          setStarting(true);
        };

  return (
    <div className="flex flex-col">
      <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
        <div className="flex items-start justify-between gap-16">
          <div className="flex flex-col gap-4">
            <h1 className="text-24 leading-tight font-semibold text-text">
              Agents
            </h1>
            <p className="max-w-measure text-15 text-text-2">
              The main agent, the area leads, the review lead and their
              short-lived workers. fold starts, watches and stops every worker;
              the agents decide what to start.
            </p>
          </div>
          {startLead === null ? null : (
            <Button
              icon={<UserPlus aria-hidden="true" size={16} />}
              onClick={startLead}
            >
              Start a lead
            </Button>
          )}
        </div>
        <RadixTabs.Root
          value={tab}
          onValueChange={(value) => {
            onSearch(
              value === "cost" ? { tab: "cost", range: search.range } : {},
            );
          }}
          className="flex flex-col gap-16"
        >
          <div className="flex flex-wrap items-center justify-between gap-12">
            <RadixTabs.List aria-label="Agents view" className={TAB_LIST}>
              <RadixTabs.Trigger
                value="team"
                className={cn(TAB_BY_STATE, focusRing)}
              >
                Team
              </RadixTabs.Trigger>
              <RadixTabs.Trigger
                value="cost"
                className={cn(TAB_BY_STATE, focusRing)}
              >
                Cost
              </RadixTabs.Trigger>
            </RadixTabs.List>
            {tab === "cost" ? (
              <div role="group" aria-label="Range" className={SEGMENT_LIST}>
                {COST_RANGES.map((option) => (
                  <button
                    key={option.value}
                    type="button"
                    aria-pressed={range === option.value}
                    onClick={() => {
                      onSearch(
                        option.value === "all"
                          ? { tab: "cost" }
                          : { tab: "cost", range: option.value },
                      );
                    }}
                    className={cn(
                      segmentClasses(range === option.value),
                      focusRing,
                    )}
                  >
                    {option.label}
                  </button>
                ))}
              </div>
            ) : null}
          </div>
          <RadixTabs.Content value="team" className={focusRing}>
            {isEmptyTeam(groups) ? (
              <EmptyTeam onStartLead={startLead} />
            ) : (
              <div className="flex flex-col gap-16 lg:flex-row lg:items-start">
                <TeamTable
                  slug={slug}
                  groups={groups}
                  human={human}
                  noLeads={hasNoLeads(groups)}
                  mainName={mainAgentName(groups)}
                  onStartLead={startLead}
                />
                {asking === undefined ? null : (
                  <PermissionPanel
                    key={asking}
                    slug={slug}
                    id={asking}
                    open={open}
                    row={askingRow(groups, open, asking)}
                    human={human}
                    now={now}
                    onClose={() => {
                      onSearch({});
                    }}
                  />
                )}
              </div>
            )}
          </RadixTabs.Content>
          <RadixTabs.Content value="cost" className={focusRing}>
            {tab === "cost" ? <CostTab slug={slug} range={range} /> : null}
          </RadixTabs.Content>
        </RadixTabs.Root>
        <StartLeadDialog
          slug={slug}
          open={starting}
          onOpenChange={setStarting}
          areas={workers.areas}
          roles={workers.roles}
          labels={labels.data ?? []}
        />
      </div>
    </div>
  );
}

function EmptyTeam({ onStartLead }: { onStartLead: (() => void) | null }) {
  return (
    <section
      aria-label="No agents yet"
      className="flex flex-col items-center gap-12 rounded-m bg-surface px-24 py-40 text-center"
    >
      <Bot aria-hidden="true" size={28} className="text-text-2" />
      <h2 className="text-18 leading-tight font-semibold text-text">
        No agents yet
      </h2>
      <p className="max-w-measure text-15 text-text-2">
        The main agent is your own Claude Code session. Run this in a terminal
        of the project, then ask it to plan the work. It starts the leads.
      </p>
      <p className="flex items-center gap-10 rounded-m bg-fill px-14 py-8">
        <Terminal aria-hidden="true" size={16} className="text-text-2" />
        <code className="font-mono text-14 text-text">fld claude</code>
      </p>
      {onStartLead === null ? null : (
        <div className="flex items-center gap-10">
          <span className="text-13 text-text-2">or</span>
          <Button
            variant="primary"
            icon={<UserPlus aria-hidden="true" size={16} />}
            onClick={onStartLead}
          >
            Start a lead
          </Button>
        </div>
      )}
      <p className="text-13 text-text-2">
        fold stops a worker when its work is done or it has been idle.
      </p>
    </section>
  );
}

export function AgentsPending() {
  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <Skeleton />
      <Skeleton />
      <Skeleton />
    </div>
  );
}
