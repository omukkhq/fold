import { Bot, Hand, OctagonAlert, Search, Tag } from "lucide-react";
import type { ProjectAgent } from "@/api/agentBar";
import type { BoardLabel } from "@/api/board";
import { Button } from "@/components/button/Button";
import { Menu, type MenuItem } from "@/components/overlay/Menu";
import { cn } from "@/lib/cn";
import { toggleValue, type BoardSearch } from "./search";
import { needsYouLook } from "./view";

const PRESSED = "bg-primary-soft text-primary-text inset-ring-primary";

interface ToolbarProps {
  search: BoardSearch;
  onSearch: (next: BoardSearch) => void;
  labels: readonly BoardLabel[];
  agents: readonly ProjectAgent[];
  /** null while the board loads; the button then shows `–` as W02's loading frame draws it. */
  needsYou: number | null;
  summary: string;
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

export function BoardToolbar({
  search,
  onSearch,
  labels,
  agents,
  needsYou,
  summary,
}: ToolbarProps) {
  const needs = needsYouLook(needsYou);
  const set = (patch: Partial<BoardSearch>) => {
    onSearch({ ...search, ...patch });
  };
  return (
    <div className="flex flex-wrap items-center justify-between gap-8 px-24 pt-14 pb-6">
      <div
        role="group"
        aria-label="Filters"
        className="flex flex-wrap items-center gap-8"
      >
        <label className="flex w-search items-center gap-8 rounded-m bg-surface px-10 py-7 inset-ring inset-ring-border has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-focus-ring">
          <Search
            aria-hidden="true"
            size={15}
            className="shrink-0 text-text-faint"
          />
          <input
            type="search"
            aria-label="Search tasks"
            placeholder="Search tasks"
            value={search.q ?? ""}
            onChange={(event) => {
              set({ q: event.target.value });
            }}
            className="min-w-0 flex-1 bg-transparent text-body leading-snug text-text outline-none placeholder:text-text-faint"
          />
        </label>
        <Button
          variant={needs.variant}
          aria-pressed={search.needs === true}
          icon={<Hand aria-hidden="true" size={15} />}
          onClick={() => {
            set({ needs: search.needs ? undefined : true });
          }}
          className={cn(
            "px-12",
            needs.pill === null && "text-text-muted",
            search.needs && "outline-2 outline-offset-2 outline-primary",
          )}
        >
          Attention
          {needs.pill === null ? (
            " · 0"
          ) : (
            <span className="rounded-full bg-on-ink px-6 text-meta font-bold text-ink">
              {needs.pill}
            </span>
          )}
        </Button>
        <Menu
          trigger={
            <Button
              icon={<Tag aria-hidden="true" size={15} />}
              className={cn(search.label && PRESSED)}
            >
              {search.label ? `Label: ${search.label}` : "Labels"}
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
              icon={<Bot aria-hidden="true" size={15} />}
              className={cn(search.agent && PRESSED)}
            >
              {search.agent ? `Agent: ${search.agent}` : "Agent"}
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
          icon={<OctagonAlert aria-hidden="true" size={15} />}
          onClick={() => {
            set({ blocked: search.blocked ? undefined : true });
          }}
          className={cn(search.blocked && PRESSED)}
        >
          Blocked only
        </Button>
      </div>
      <p className="text-bar text-text-muted">{summary}</p>
    </div>
  );
}
