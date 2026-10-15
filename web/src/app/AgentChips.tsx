import { useQuery } from "@tanstack/react-query";
import { agentBarQuery } from "../api/agentBar";
import { Avatar, PresenceDot } from "../components/avatar/Avatar";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { useLiveStatus } from "../live/status";
import { useNow } from "../live/useNow";
import { agentBoardPath, chipAgents, chipStateText } from "./agentChips";
import { HrefLink } from "./HrefLink";

const MINUTE_MS = 60_000;

export function AgentChips({ slug }: { slug: string }) {
  const agents = useQuery(agentBarQuery(slug));
  const status = useLiveStatus();
  const now = useNow(status.state === "reconnecting", MINUTE_MS);
  if (!agents.data || agents.data.length === 0) {
    return null;
  }
  return (
    <ul aria-label="Agents" className="flex items-center gap-6">
      {chipAgents(agents.data).map((agent) => (
        <li key={agent.name}>
          <HrefLink
            href={agentBoardPath(slug, agent.name)}
            className={cn(
              "inline-flex items-center gap-6 rounded-chip bg-surface py-3 pr-9 pl-4 inset-ring inset-ring-border hover:bg-sunken",
              focusRing,
            )}
          >
            <Avatar name={agent.name} />
            <span className="text-bar font-semibold text-text">
              {agent.name}
            </span>
            <PresenceDot state={agent.presence} />
            <span className="text-meta text-text-muted">
              {chipStateText(agent, status, now)}
            </span>
          </HrefLink>
        </li>
      ))}
    </ul>
  );
}
