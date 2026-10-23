import { useQuery } from "@tanstack/react-query";
import { agentBarQuery } from "../api/agentBar";
import { projectQuery } from "../api/queries";
import { PresenceDot } from "../components/avatar/PresenceDot";
import { cn } from "../lib/cn";
import { focusRing } from "../lib/focus";
import { useLiveStatus } from "../live/status";
import { useNow } from "../live/useNow";
import { PersonName } from "../people/PersonName";
import {
  agentBoardPath,
  chipAgents,
  chipErrorTitle,
  chipPresence,
  chipStateText,
  isPaused,
} from "./agentChipsModel";
import { HrefLink } from "./HrefLink";

const MINUTE_MS = 60_000;

/** Below `lg` the chips show only in the top bar's More popover, so there they stack as full-width 44px rows. */
export function AgentChips({
  slug,
  onOpen,
}: {
  slug: string;
  /** Called when a chip opens the board, so a popover that holds the chips can close. */
  onOpen?: () => void;
}) {
  const agents = useQuery(agentBarQuery(slug));
  const project = useQuery(projectQuery(slug));
  const paused = isPaused(project.data?.overview?.paused_since);
  const status = useLiveStatus();
  const now = useNow(status.state === "reconnecting", MINUTE_MS);
  if (!agents.data || agents.data.length === 0) {
    return null;
  }
  return (
    <ul
      aria-label="Agents"
      className="flex items-center gap-6 max-lg:flex-col max-lg:items-stretch"
    >
      {chipAgents(agents.data, paused).map((agent) => (
        <li key={agent.name}>
          <HrefLink
            href={agentBoardPath(slug, agent.name)}
            title={chipErrorTitle(agent)}
            onClick={onOpen}
            className={cn(
              "inline-flex items-center gap-6 rounded-chip bg-surface px-9 py-3 text-meta inset-ring inset-ring-border hover:bg-sunken max-lg:flex max-lg:min-h-44 max-lg:px-14",
              focusRing,
            )}
          >
            <PersonName name={agent.name} className="text-bar" />
            <PresenceDot state={chipPresence(agent, paused)} />
            <span className="text-meta text-text-muted">
              {chipStateText(agent, status, now, paused)}
            </span>
          </HrefLink>
        </li>
      ))}
    </ul>
  );
}
