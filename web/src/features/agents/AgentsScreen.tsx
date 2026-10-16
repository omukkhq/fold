import { skipToken, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { Bot, Terminal, UserPlus } from "lucide-react";
import { useState } from "react";
import { PausedBanner } from "@/app/PausedBanner";
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
import { Skeleton } from "@/components/states/Skeleton";
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
  // Only a named human starts a lead, so the controls wait for a name.
  const startLead =
    human === null
      ? null
      : () => {
          setStarting(true);
        };

  return (
    <div className="flex flex-col">
      <PausedBanner slug={slug} />
      <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
        <div className="flex items-start justify-between gap-16">
          <div className="flex flex-col gap-4">
            <h1 className="text-screen leading-snug font-semibold text-text">
              Agents
            </h1>
            <p className="text-body text-text-muted">
              The main agent, the area leads, the review lead and their
              short-lived workers. fold starts, watches and stops every worker;
              the agents decide what to start.
            </p>
          </div>
          {startLead === null ? null : (
            <Button
              icon={<UserPlus aria-hidden="true" size={15} />}
              onClick={startLead}
            >
              Start a lead
            </Button>
          )}
        </div>
        {isEmptyTeam(groups) ? (
          <EmptyTeam onStartLead={startLead} />
        ) : (
          <div className="flex items-start gap-16">
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
      className="flex flex-col items-center gap-12 rounded-l border border-border-subtle bg-surface px-24 py-40 text-center"
    >
      <Bot aria-hidden="true" size={28} className="text-text-faint" />
      <h2 className="text-item-title leading-snug font-semibold text-text">
        No agents yet
      </h2>
      <p className="text-body text-text-muted">
        The main agent is your own Claude Code session. Run this in a terminal
        of the project, then ask it to plan the work. It starts the leads.
      </p>
      <p className="flex items-center gap-10 rounded-m bg-sunken px-14 py-8 inset-ring inset-ring-border">
        <Terminal aria-hidden="true" size={15} className="text-text-muted" />
        <code className="font-mono text-body text-text">fld claude</code>
      </p>
      {onStartLead === null ? null : (
        <div className="flex items-center gap-10">
          <span className="text-code text-text-faint">or</span>
          <Button
            variant="primary"
            icon={<UserPlus aria-hidden="true" size={15} />}
            onClick={onStartLead}
          >
            Start a lead
          </Button>
        </div>
      )}
      <p className="text-meta text-text-faint">
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
