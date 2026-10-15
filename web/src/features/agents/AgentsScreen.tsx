import { useSuspenseQuery } from "@tanstack/react-query";
import { Bot } from "lucide-react";
import { agentsQuery } from "@/api/queries";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { Skeleton } from "@/components/states/Skeleton";
import { AgentCard } from "./AgentCard";

export function AgentsScreen({ slug }: { slug: string }) {
  const { data, dataUpdatedAt } = useSuspenseQuery(agentsQuery(slug));
  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <div className="flex flex-col gap-4">
        <h1 className="text-screen leading-snug font-semibold text-text">
          Agents
        </h1>
        <p className="max-w-full text-body text-text-muted">
          Each terminal is one agent. fold records its workers so claims and
          workspaces have an owner; agents talk to their subagents directly.
        </p>
      </div>
      {data.length === 0 ? (
        <EmptyPanel icon={Bot} title="No agent has connected">
          Start an agent session with fold mcp; its workers, claims and
          workspaces appear here.
        </EmptyPanel>
      ) : (
        <div className="grid grid-cols-3 items-start gap-16">
          {data.map((agent) => (
            <AgentCard
              key={agent.name}
              slug={slug}
              agent={agent}
              now={dataUpdatedAt}
            />
          ))}
        </div>
      )}
    </div>
  );
}

export function AgentsPending() {
  return (
    <div className="grid grid-cols-3 gap-16 px-24 pt-20 pb-24">
      <Skeleton />
      <Skeleton />
      <Skeleton />
    </div>
  );
}
