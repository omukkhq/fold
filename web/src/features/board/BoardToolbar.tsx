import {
  ChevronDown,
  Hand,
  OctagonAlert,
  Search,
  SlidersHorizontal,
  Tag,
  UserRound,
  X,
} from "lucide-react";
import type { BoardLabel } from "@/api/board";
import { Button, IconButton } from "@/components/button/Button";
import { COUNT_CLASSES } from "@/components/badge/chips";
import { Menu, type MenuItem } from "@/components/overlay/Menu";
import { Popover } from "@/components/overlay/Popover";
import { cn } from "@/lib/cn";
import { choiceTitle } from "./filters";
import type { OwnerOption } from "./owners";
import { toggleName, type BoardSearch } from "./search";
import { needsYouPill } from "./view";

/** Toolbar controls sit on the page ground as surface blocks; a set filter takes the accent tint. */
const SURFACE = "bg-surface hover:bg-fill";
const CONTROL = `${SURFACE} px-12`;
const PRESSED = "bg-accent-soft text-accent hover:bg-accent-soft";

interface ToolbarProps {
  search: BoardSearch;
  onSearch: (next: BoardSearch) => void;
  labels: readonly BoardLabel[];
  owners: readonly OwnerOption[];
  /** null while the board loads; the button then shows `–`. */
  needsYou: number | null;
  summary: string;
  /** A phone keeps search and Attention in the row and moves the other filters into one button. */
  phone: boolean;
}

/** Tick boxes for each entry, OR inside the menu. Clear always leads the list, inactive while nothing is ticked, so a tick never moves the rows under the pointer. */
function tickItems(
  options: readonly { key: string; label: string; hint?: string }[],
  ticked: readonly string[] | undefined,
  choose: (value: string[] | undefined) => void,
): MenuItem[] {
  return [
    {
      label: "Clear",
      icon: X,
      disabled: ticked === undefined,
      onSelect: () => {
        choose(undefined);
      },
    },
    ...options.map((option) => ({
      id: `option:${option.key}`,
      label: option.label,
      hint: option.hint,
      checked: ticked?.includes(option.key) ?? false,
      onSelect: () => {
        choose(toggleName(ticked, option.key));
      },
    })),
  ];
}

function OtherFilters({
  search,
  set,
  labels,
  owners,
}: Pick<ToolbarProps, "search" | "labels" | "owners"> & {
  set: (patch: Partial<BoardSearch>) => void;
}) {
  const ownerNames = (search.owner ?? []).map(
    (key) => owners.find((owner) => owner.key === key)?.label ?? key,
  );
  return (
    <>
      <Menu
        trigger={
          <Button
            icon={<Tag aria-hidden="true" size={16} />}
            className={cn(CONTROL, search.label && PRESSED)}
          >
            {choiceTitle(
              { none: "Labels", one: "Label", many: "Labels" },
              search.label ?? [],
            )}
            <ChevronDown aria-hidden="true" size={14} className="-ml-2" />
          </Button>
        }
        items={tickItems(
          labels.map((label) => ({ key: label.name, label: label.name })),
          search.label,
          (label) => {
            set({ label });
          },
        )}
      />
      <Menu
        trigger={
          <Button
            icon={<UserRound aria-hidden="true" size={16} />}
            className={cn(CONTROL, search.owner && PRESSED)}
          >
            {choiceTitle(
              { none: "Owner", one: "Owner", many: "Owners" },
              ownerNames,
            )}
            <ChevronDown aria-hidden="true" size={14} className="-ml-2" />
          </Button>
        }
        items={tickItems(owners, search.owner, (owner) => {
          set({ owner });
        })}
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
  owners,
  needsYou,
  summary,
  phone,
}: ToolbarProps) {
  const pill = needsYouPill(needsYou);
  const set = (patch: Partial<BoardSearch>) => {
    onSearch({ ...search, ...patch });
  };
  const othersSet = Boolean(search.label ?? search.owner ?? search.blocked);
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
                    ? "Filters: labels, owner, blocked only; a filter is set"
                    : "Filters: labels, owner, blocked only"
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
                owners={owners}
              />
            </div>
          </Popover>
        ) : (
          <OtherFilters
            search={search}
            set={set}
            labels={labels}
            owners={owners}
          />
        )}
      </div>
      {phone ? null : (
        <p className="text-14 text-text-2 tabular-nums">{summary}</p>
      )}
    </div>
  );
}
