import * as RadixPopover from "@radix-ui/react-popover";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, ChevronDown } from "lucide-react";
import { useState } from "react";
import { agentBarQuery } from "../api/agentBar";
import { projectQuery, sessionQuery } from "../api/queries";
import { buttonClasses } from "../components/button/buttonStyles";
import { cn } from "../lib/cn";
import { focusRing, insetFocusRing } from "../lib/focus";
import { useLiveStatus } from "../live/status";
import { useNow } from "../live/useNow";
import { PersonName } from "../people/PersonName";
import { projectPath } from "../routes/paths";
import {
  agentBoardPath,
  chipErrorTitle,
  chipStateText,
  isPaused,
  panelAgents,
} from "./agentChipsModel";
import {
  agentRowLook,
  agentsButton,
  workingAgents,
  type AgentDot,
  type AgentsTone,
} from "./agentsModel";
import { Tooltip } from "../components/overlay/Tooltip";
import { HrefLink } from "./HrefLink";
import { listNote, listState } from "./listNote";
import { PanelNote } from "./PanelNote";
import { PauseControl } from "./PauseControl";
import { pauseNote } from "./pause";
import { StreamStatus } from "./StreamStatus";

const MINUTE_MS = 60_000;

const DOTS: Record<AgentDot, string> = {
  work: "bg-ok",
  idle: "inset-ring-2 inset-ring-ok",
  off: "inset-ring-2 inset-ring-text-2",
  fail: "bg-bad",
};

const TONE_DOT: Record<AgentsTone, string | null> = {
  live: "bg-ok",
  connecting: "bg-text-2",
  reconnecting: "bg-warn",
  paused: null,
};

/** The agents of the project as one control: the live state and how many work on the button; every agent, Pause and the stream status in its panel. */
export function AgentsControl({ slug }: { slug: string }) {
  const agents = useQuery(agentBarQuery(slug));
  const project = useQuery(projectQuery(slug));
  const session = useQuery(sessionQuery());
  const status = useLiveStatus();
  const now = useNow(status.state === "reconnecting", MINUTE_MS);
  const [open, setOpen] = useState(false);
  const paused = isPaused(project.data?.overview?.paused_since);
  const list = agents.data ?? [];
  const note = listNote(listState(agents), list.length, {
    loading: "Loading the agents",
    failed: "The agents could not load.",
    empty: "No agent has joined this project yet.",
  });
  const view = agentsButton(status.state, paused, workingAgents(list, paused));
  const dot = TONE_DOT[view.tone];
  const close = () => {
    setOpen(false);
  };
  return (
    <RadixPopover.Root open={open} onOpenChange={setOpen}>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          className={cn(
            buttonClasses("quiet", "md"),
            "shrink-0",
            view.tone === "paused" &&
              "bg-warn-soft text-warn hover:bg-warn-soft",
            focusRing,
          )}
        >
          {dot === null ? null : (
            <span
              aria-hidden="true"
              className={cn("size-8 shrink-0 rounded-full", dot)}
            />
          )}
          {view.label}
          <ChevronDown aria-hidden="true" size={16} className="text-text-2" />
        </button>
      </RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align="end"
          sideOffset={6}
          collisionPadding={16}
          aria-label="Agents"
          className="z-pop flex max-h-(--radix-popover-content-available-height) w-agents max-w-(--width-popover-max) flex-col overflow-hidden rounded-m bg-surface shadow-pop outline-none"
        >
          <div className="flex shrink-0 items-center gap-12 px-16 pt-14 pb-8">
            <h2 className="text-16 font-semibold text-text">Agents</h2>
            <span className="ml-auto">
              <StreamStatus announce={false} />
            </span>
          </div>
          {(session.data?.human ?? null) === null ||
          project.data?.overview == null ? null : (
            <div className="mx-16 mb-6 flex shrink-0 items-center gap-14 rounded-m bg-sunken px-14 py-12">
              <p className="min-w-0 flex-1 text-13 text-text-2">
                {pauseNote(paused)}
              </p>
              <PauseControl slug={slug} onSunken />
            </div>
          )}
          {note !== null ? (
            <PanelNote
              note={note}
              onRetry={() => void agents.refetch()}
              className="px-16 py-10"
            />
          ) : (
            <ul
              aria-label="Agents"
              className="min-h-0 overflow-y-auto px-6 pb-6"
            >
              {panelAgents(list, paused).map((agent) => {
                const look = agentRowLook(agent, paused);
                const error = chipErrorTitle(agent);
                const row = (
                  <HrefLink
                    href={agentBoardPath(slug, agent.name)}
                    onClick={close}
                    className={cn(
                      "flex items-center gap-12 rounded-s px-10 py-8 text-14 hover:bg-sunken",
                      insetFocusRing,
                    )}
                  >
                    <span
                      aria-hidden="true"
                      className={cn(
                        "size-10 shrink-0 rounded-full",
                        DOTS[look.dot],
                      )}
                    />
                    <PersonName
                      name={agent.name}
                      className="min-w-0 flex-1 truncate"
                    />
                    <span
                      className={cn(
                        "shrink-0 text-right text-13 whitespace-nowrap",
                        look.tone === "warn"
                          ? "font-medium text-warn"
                          : look.tone === "bad"
                            ? "font-medium text-bad"
                            : "text-text-2",
                      )}
                    >
                      {chipStateText(agent, status, now, paused)}
                    </span>
                  </HrefLink>
                );
                return (
                  <li key={agent.name}>
                    {error === undefined ? (
                      row
                    ) : (
                      <Tooltip content={error}>{row}</Tooltip>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          <div className="flex shrink-0 items-center justify-between gap-12 px-16 pt-8 pb-14 text-13 text-text-2">
            <span>Choose an agent to see its tasks on the board.</span>
            <HrefLink
              href={projectPath(slug, "agents")}
              onClick={close}
              className={cn(
                "inline-flex shrink-0 items-center gap-6 rounded-s font-title text-14 font-semibold text-accent",
                focusRing,
              )}
            >
              Agents tab
              <ArrowRight aria-hidden="true" size={14} />
            </HrefLink>
          </div>
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
