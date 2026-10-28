import {
  Bot,
  ChevronDown,
  Hand,
  OctagonAlert,
  Search,
  SlidersHorizontal,
  Tag,
} from "lucide-react";
import type { ProjectAgent } from "@/api/agentBar";
import type { BoardLabel } from "@/api/board";
import { Button, IconButton } from "@/components/button/Button";
import { COUNT_CLASSES } from "@/components/badge/chips";
import { Menu, type MenuItem } from "@/components/overlay/Menu";
import { Popover } from "@/components/overlay/Popover";
import { cn } from "@/lib/cn";
import { toggleValue, type BoardSearch } from "./search";
import { needsYouPill } from "./view";

/** Toolbar controls sit on the page ground as surface blocks; a set filter takes the accent tint. */
const SURFACE = "bg-surface hover:bg-fill";
const CONTROL = `${SURFACE} px-12`;
const PRESSED = "bg-accent-soft text-accent hover:bg-accent-soft";

interface ToolbarProps {
  search: BoardSearch;
  onSearch: (next: BoardSearch) => void;
  labels: readonly BoardLabel[];
  agents: readonly ProjectAgent[];
  /** null while the board loads; the button then shows `–`. */
  needsYou: number | null;
  summary: string;
  /** A phone keeps search and Attention in the row and moves the other filters into one button. */
  phone: boolean;
}

function choiceItems(
  names: readonly string[],
  current: string | undefined,
  anyLabel: string,
  choose: (value: string | undefined) => void,
): MenuItem[] {
  const items: MenuItem[] = names.map((name) => ({
    label: current === name ? `${name} ✓` : name,
    onSelect: () => {
      choose(toggleValue(current, name));
    },
  }));
  return current === undefined
    ? items
    : [
        {
          label: anyLabel,
          onSelect: () => {
            choose(undefined);
          },
        },
        ...items,
      ];
}

function OtherFilters({
  search,
  set,
  labels,
  agents,
}: Pick<ToolbarProps, "search" | "labels" | "agents"> & {
  set: (patch: Partial<BoardSearch>) => void;
}) {
  return (
    <>
      <Menu
        trigger={
          <Button
            icon={<Tag aria-hidden="true" size={16} />}
            className={cn(CONTROL, search.label && PRESSED)}
          >
            {search.label ? `Label: ${search.label}` : "Labels"}
            <ChevronDown aria-hidden="true" size={14} className="-ml-2" />
          </Button>
        }
        items={choiceItems(
          labels.map((label) => label.name),
          search.label,
          "Any label",
          (label) => {
            set({ label });
          },
        )}
      />
      <Menu
        trigger={
          <Button
            icon={<Bot aria-hidden="true" size={16} />}
            className={cn(CONTROL, search.agent && PRESSED)}
          >
            {search.agent ? `Agent: ${search.agent}` : "Agent"}
            <ChevronDown aria-hidden="true" size={14} className="-ml-2" />
          </Button>
        }
        items={choiceItems(
          agents.map((agent) => agent.name),
          search.agent,
          "Any agent",
          (agent) => {
            set({ agent });
          },
        )}
      />
      <Button
        aria-pressed={search.blocked === true}
        icon={<OctagonAlert aria-hidden="true" size={16} />}
        onClick={() => {
          set({ blocked: search.blocked ? undefined : true });
        }}
        className={cn(CONTROL, search.blocked && PRESSED)}
      >
        Blocked only
      </Button>
    </>
  );
}

export function BoardToolbar({
  search,
  onSearch,
  labels,
  agents,
  needsYou,
  summary,
  phone,
}: ToolbarProps) {
  const pill = needsYouPill(needsYou);
  const set = (patch: Partial<BoardSearch>) => {
    onSearch({ ...search, ...patch });
  };
  const othersSet = Boolean(search.label ?? search.agent ?? search.blocked);
  return (
    <div
      className={cn(
        "flex shrink-0 items-center gap-8",
        phone ? "px-12 pt-12" : "flex-wrap justify-between px-22 pt-16 pb-4",
      )}
    >
      <div
        role="group"
        aria-label="Filters"
        className={cn(
          "flex items-center gap-8",
          phone ? "min-w-0 flex-1" : "flex-wrap",
        )}
      >
        <label
          className={cn(
            "flex items-center gap-8 rounded-s bg-surface px-12 py-7 has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-accent",
            phone ? "min-w-0 flex-1" : "w-search",
          )}
        >
          <Search
            aria-hidden="true"
            size={16}
            className="shrink-0 text-text-2"
          />
          <input
            type="search"
            aria-label="Search tasks"
            placeholder="Search tasks"
            value={search.q ?? ""}
            onChange={(event) => {
              set({ q: event.target.value });
            }}
            className="min-w-0 flex-1 bg-transparent text-14 leading-tight text-text outline-none placeholder:text-text-2"
          />
        </label>
        <Button
          aria-pressed={search.needs === true}
          icon={<Hand aria-hidden="true" size={16} />}
          onClick={() => {
            set({ needs: search.needs ? undefined : true });
          }}
          className={cn(
            CONTROL,
            "shrink-0",
            pill === null && "text-text-2",
            search.needs && PRESSED,
          )}
        >
          Attention
          {pill === null ? (
            " · 0"
          ) : (
            <span className={COUNT_CLASSES}>{pill}</span>
          )}
        </Button>
        {phone ? (
          <Popover
            align="end"
            trigger={
              <IconButton
                aria-label={
                  othersSet
                    ? "Filters: labels, agent, blocked only; one is set"
                    : "Filters: labels, agent, blocked only"
                }
                className={cn(
                  SURFACE,
                  "relative shrink-0",
                  othersSet && PRESSED,
                )}
              >
                <SlidersHorizontal aria-hidden="true" size={16} />
                {othersSet ? (
                  <span
                    aria-hidden="true"
                    className="absolute top-6 right-6 size-6 rounded-full bg-accent"
                  />
                ) : null}
              </IconButton>
            }
          >
            <div className="flex flex-col items-start gap-8">
              <OtherFilters
                search={search}
                set={set}
                labels={labels}
                agents={agents}
              />
            </div>
          </Popover>
        ) : (
          <OtherFilters
            search={search}
            set={set}
            labels={labels}
            agents={agents}
          />
        )}
      </div>
      {phone ? null : (
        <p className="text-14 text-text-2 tabular-nums">{summary}</p>
      )}
    </div>
  );
}
