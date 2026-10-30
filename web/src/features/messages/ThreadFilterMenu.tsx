import * as RadixPopover from "@radix-ui/react-popover";
import { Check, ChevronDown, ListFilter } from "lucide-react";
import { useId } from "react";
import { Count } from "@/components/badge/Chip";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  choicesWith,
  filterSummary,
  showFilter,
  type ShowOption,
  type ThreadFilter,
} from "./filters";

const NARROW = [
  { key: "agent", label: "Agent", all: "All agents", group: "agents" },
  { key: "task", label: "Task", all: "All tasks", group: "tasks" },
  {
    key: "proposal",
    label: "Proposal",
    all: "All proposals",
    group: "proposals",
  },
] as const;

type NarrowKey = (typeof NARROW)[number]["key"];

interface ThreadFilterMenuProps {
  filter: ThreadFilter;
  shows: ShowOption[];
  options: { agents: string[]; tasks: string[]; proposals: string[] };
  onShow: (show: ShowOption["show"]) => void;
  onNarrow: (key: NarrowKey, value: string | undefined) => void;
}

/** One Filter button above the thread list: what kind of thread shows, then the agent, task and proposal the list narrows to. */
export function ThreadFilterMenu({
  filter,
  shows,
  options,
  onShow,
  onNarrow,
}: ThreadFilterMenuProps) {
  const id = useId();
  const current = showFilter(filter);
  const narrowed = NARROW.some(({ key }) => filter[key] !== undefined);
  return (
    <RadixPopover.Root>
      <RadixPopover.Trigger asChild>
        <button
          type="button"
          aria-label={`Filter: ${filterSummary(filter)}`}
          className={cn(
            "inline-flex min-w-0 items-center gap-8 rounded-s py-7 pr-10 pl-12 font-title text-14 leading-tight font-semibold max-sm:min-h-44",
            current === "all" && !narrowed
              ? "bg-fill text-text hover:bg-line"
              : "bg-accent-soft text-accent",
            "data-[state=open]:bg-accent-soft data-[state=open]:text-accent",
            focusRing,
          )}
        >
          <ListFilter aria-hidden="true" size={16} className="shrink-0" />
          <span className="truncate">{filterSummary(filter)}</span>
          <ChevronDown aria-hidden="true" size={14} className="shrink-0" />
        </button>
      </RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          align="start"
          sideOffset={6}
          collisionPadding={16}
          aria-label="Filter chats"
          className="z-pop flex w-menu max-w-(--width-popover-max) flex-col gap-4 rounded-m bg-surface p-8 shadow-pop outline-none"
        >
          <p
            id={`${id}-show`}
            className="px-8 pt-6 pb-2 font-title text-13 leading-tight font-semibold text-text-2"
          >
            Show
          </p>
          <div
            role="group"
            aria-labelledby={`${id}-show`}
            className="flex flex-col"
          >
            {shows.map((option) => {
              const on = option.show === current;
              return (
                <button
                  key={option.show}
                  type="button"
                  aria-pressed={on}
                  onClick={() => {
                    onShow(option.show);
                  }}
                  className={cn(
                    "flex items-center gap-10 rounded-s px-8 py-7 text-left text-14 text-text hover:bg-sunken max-sm:min-h-44",
                    on && "font-semibold",
                    focusRing,
                  )}
                >
                  <Check
                    aria-hidden="true"
                    size={14}
                    className={cn("shrink-0 text-accent", !on && "invisible")}
                  />
                  {option.label}
                  {option.count ? (
                    <Count
                      value={option.count}
                      label={`${String(option.count)} wait for you`}
                      className="ml-auto"
                    />
                  ) : null}
                </button>
              );
            })}
          </div>
          <p className="px-8 pt-10 pb-2 font-title text-13 leading-tight font-semibold text-text-2">
            Narrow to
          </p>
          {NARROW.map(({ key, label, all, group }) => (
            <label
              key={key}
              className="grid grid-cols-[var(--width-fact-label)_minmax(0,1fr)] items-center gap-8 px-8 py-3"
            >
              <span className="font-title text-13 font-semibold text-text-2">
                {label}
              </span>
              <select
                value={filter[key] ?? ""}
                onChange={(event) => {
                  const value = event.target.value;
                  onNarrow(key, value === "" ? undefined : value);
                }}
                className={cn(
                  "min-w-0 rounded-s bg-fill px-8 py-6 text-14 text-text max-sm:min-h-44",
                  focusRing,
                )}
              >
                <option value="">{all}</option>
                {choicesWith(options[group], filter[key]).map((option) => (
                  <option key={option} value={option}>
                    {option}
                  </option>
                ))}
              </select>
            </label>
          ))}
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
