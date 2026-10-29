import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import * as RadixPopover from "@radix-ui/react-popover";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronDown, X } from "lucide-react";
import { useId, useRef, useState, type ReactNode } from "react";
import type { BoardLabel } from "@/api/board";
import { workersQuery } from "@/api/queries";
import { labelsQuery } from "@/api/tasks";
import { IconButton } from "@/components/button/Button";
import { controlClasses, SENTENCE_HINT } from "@/components/form/Field";
import { labelColors } from "@/components/label/labelColors";
import { menuContent, menuItem } from "@/components/overlay/menuStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { stepIndex } from "@/mentions/suggest";
import {
  NO_AREA,
  NO_AREA_TEXT,
  labelChoices,
  labelNamed,
  ownerText,
  withMainArea,
  type AreaOption,
  type AreaRef,
} from "./areas";

// The same label and help text as the shared form fields around them.
const FIELD_LABEL =
  "font-title text-13 leading-tight font-semibold text-text-2";
const FIELD_HINT = SENTENCE_HINT;

/** The project's areas, with the built-in area Main first, and its labels. The areas are null until the worker list is read; a refused read leaves them empty, so every label counts as one that is no area, and sets `failed`. */
export function useAreas(slug: string): {
  areas: AreaRef[] | null;
  labels: BoardLabel[];
  failed: boolean;
  retry: () => void;
} {
  const workers = useQuery(workersQuery(slug));
  const labels = useQuery(labelsQuery(slug));
  const failed = workers.data === undefined && workers.isError;
  return {
    areas:
      workers.data === undefined
        ? failed
          ? []
          : null
        : withMainArea(workers.data.areas, workers.data.main_label),
    labels: labels.data ?? [],
    failed,
    retry: () => {
      void workers.refetch();
    },
  };
}

export function LabelDot({ color }: { color: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn("size-6 shrink-0 rounded-full", labelColors(color).dot)}
    />
  );
}

function AreaRow({ option }: { option: AreaOption }) {
  return (
    <>
      <LabelDot color={option.label.color} />
      <span className="min-w-0 flex-1 truncate text-14 text-text">
        {option.label.name}
      </span>
      <span className="shrink-0 text-13 text-text-2">{ownerText(option)}</span>
    </>
  );
}

/** The single choice of a task's area: "Decide automatically" first, then Main, which the main agent builds, then every area with its lead. */
export function AreaMenu({
  options,
  value,
  onValueChange,
  children,
}: {
  options: readonly AreaOption[];
  /** The marked area, or `UNMARKED` for none. */
  value: string;
  onValueChange: (area: string) => void;
  /** The trigger. */
  children: ReactNode;
}) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>{children}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={4}
          className={cn(menuContent, "w-side-panel")}
        >
          <DropdownMenu.RadioGroup value={value} onValueChange={onValueChange}>
            {[null, ...options].map((option) => (
              <DropdownMenu.RadioItem
                key={option?.name ?? NO_AREA}
                value={option?.name ?? NO_AREA}
                className={cn(menuItem, "text-14")}
              >
                <span className="flex w-14 shrink-0 justify-center">
                  <DropdownMenu.ItemIndicator>
                    <Check aria-hidden="true" size={14} />
                  </DropdownMenu.ItemIndicator>
                </span>
                {option === null ? (
                  <span className="flex-1">{NO_AREA_TEXT}</span>
                ) : (
                  <AreaRow option={option} />
                )}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** The New task form's area: one area or none, which leaves the choice to the main agent. */
export function AreaField({
  options,
  value,
  onChange,
  disabled,
}: {
  options: readonly AreaOption[];
  value: string;
  onChange: (area: string) => void;
  disabled: boolean;
}) {
  const ids = useId();
  const chosen = options.find((option) => option.name === value);
  return (
    <div className="flex min-w-0 flex-col gap-6">
      <span id={`${ids}-label`} className={FIELD_LABEL}>
        Area
      </span>
      <AreaMenu options={options} value={value} onValueChange={onChange}>
        <button
          type="button"
          aria-labelledby={`${ids}-label ${ids}-value`}
          aria-describedby={`${ids}-hint`}
          disabled={disabled}
          className={cn(
            controlClasses,
            focusRing,
            "flex items-center gap-8 py-9 text-left text-14",
          )}
        >
          <span id={`${ids}-value`} className="flex min-w-0 flex-1 gap-8">
            {chosen ? (
              <span className="flex min-w-0 flex-1 items-center gap-8">
                <AreaRow option={chosen} />
              </span>
            ) : (
              <span className="flex-1">{NO_AREA_TEXT}</span>
            )}
          </span>
          <ChevronDown
            aria-hidden="true"
            size={14}
            className="shrink-0 text-text-2"
          />
        </button>
      </AreaMenu>
      <p id={`${ids}-hint`} className={FIELD_HINT}>
        The area&apos;s lead starts a worker for the task. With Decide
        automatically, the main agent chooses an area; with Main agent, it
        builds the task itself and a human approves it.
      </p>
    </div>
  );
}

/** A label chip with its remove button, which stays pending while a write removes the label. */
export function RemovableLabel({
  label,
  disabled,
  pending = false,
  onRemove,
}: {
  label: BoardLabel;
  disabled: boolean;
  pending?: boolean;
  onRemove: () => void;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-5 rounded-pill py-2 pr-4 pl-8 text-13 leading-tight font-medium text-text",
        labelColors(label.color).soft,
      )}
    >
      <LabelDot color={label.color} />
      {label.name}
      <IconButton
        size="sm"
        aria-label={`Remove ${label.name}`}
        pending={pending}
        pendingLabel={`Removing ${label.name}…`}
        disabled={disabled}
        className="size-auto rounded-s p-2 text-text-2"
        onClick={onRemove}
      >
        <X aria-hidden="true" size={14} />
      </IconButton>
    </span>
  );
}

/**
 * The New task form's labels: any number of labels that are no area's label. Focus stays in the text box: typing filters the
 * project's labels, the arrow keys move through them, Enter adds the highlighted one and Backspace in an empty box removes the last.
 */
export function LabelsField({
  labels,
  areas,
  value,
  onChange,
  disabled,
}: {
  labels: readonly BoardLabel[];
  areas: readonly AreaRef[];
  value: readonly string[];
  onChange: (labels: string[]) => void;
  disabled: boolean;
}) {
  const ids = useId();
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const choices = labelChoices(labels, areas, value, query);
  const open = focused && !dismissed && !disabled;
  const current = Math.min(index, Math.max(0, choices.length - 1));
  const chosen = value.map((name) => labelNamed(labels, name));

  const add = (label: BoardLabel) => {
    onChange([...value, label.name]);
    setQuery("");
    setIndex(0);
  };

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <label htmlFor={`${ids}-input`} className={FIELD_LABEL}>
        Labels
      </label>
      <RadixPopover.Root
        open={open}
        onOpenChange={(next) => {
          if (!next) setDismissed(true);
        }}
      >
        <RadixPopover.Anchor asChild>
          <div
            ref={box}
            className={cn(
              "flex flex-wrap items-center gap-6 rounded-s bg-fill px-8 py-7 has-[input:focus-visible]:outline-2 has-[input:focus-visible]:outline-offset-2 has-[input:focus-visible]:outline-accent",
              disabled && "opacity-60",
            )}
          >
            {chosen.map((label) => (
              <RemovableLabel
                key={label.name}
                label={label}
                disabled={disabled}
                onRemove={() => {
                  onChange(value.filter((name) => name !== label.name));
                  input.current?.focus();
                }}
              />
            ))}
            <input
              ref={input}
              id={`${ids}-input`}
              type="text"
              role="combobox"
              autoComplete="off"
              placeholder="Add a label"
              aria-describedby={`${ids}-hint`}
              aria-expanded={open}
              aria-controls={
                open && choices.length > 0 ? `${ids}-list` : undefined
              }
              aria-autocomplete="list"
              aria-activedescendant={
                open && choices.length > 0
                  ? `${ids}-option-${String(current)}`
                  : undefined
              }
              value={query}
              disabled={disabled}
              className="w-label-input flex-1 bg-transparent text-14 text-text outline-none placeholder:text-text-2"
              onFocus={() => {
                setFocused(true);
              }}
              onBlur={() => {
                setFocused(false);
                setDismissed(false);
              }}
              onChange={(event) => {
                setQuery(event.target.value);
                setIndex(0);
                setDismissed(false);
              }}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                  event.preventDefault();
                  setDismissed(false);
                  setIndex(
                    stepIndex(
                      current,
                      choices.length,
                      event.key === "ArrowDown" ? 1 : -1,
                    ),
                  );
                } else if (event.key === "Enter") {
                  // Enter adds a label; it never sends the form from this box.
                  event.preventDefault();
                  const pick = open ? choices[current] : undefined;
                  if (pick) add(pick);
                } else if (event.key === "Backspace" && query === "") {
                  const last = value.at(-1);
                  if (last !== undefined) {
                    onChange(value.slice(0, -1));
                  }
                }
              }}
            />
          </div>
        </RadixPopover.Anchor>
        <RadixPopover.Portal>
          <RadixPopover.Content
            align="start"
            sideOffset={4}
            onOpenAutoFocus={(event) => {
              event.preventDefault();
            }}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
            }}
            onInteractOutside={(event) => {
              if (
                event.target instanceof Node &&
                box.current?.contains(event.target)
              ) {
                event.preventDefault();
              }
            }}
            className={cn(menuContent, "w-side-panel")}
          >
            {choices.length === 0 ? (
              <p className="px-8 py-6 text-14 text-text-2">
                {query.trim() === ""
                  ? "No more labels that are not areas."
                  : "No label matches."}
              </p>
            ) : (
              <ul
                id={`${ids}-list`}
                role="listbox"
                aria-label="Labels"
                className="flex flex-col"
              >
                {choices.map((label, at) => (
                  <li
                    key={label.name}
                    id={`${ids}-option-${String(at)}`}
                    role="option"
                    aria-selected={at === current}
                    className={cn(
                      "flex cursor-pointer items-center gap-8 rounded-s px-8 py-6 text-14 text-text",
                      at === current && "bg-sunken",
                    )}
                    onMouseDown={(event) => {
                      event.preventDefault();
                      add(label);
                    }}
                  >
                    <LabelDot color={label.color} />
                    {label.name}
                  </li>
                ))}
              </ul>
            )}
          </RadixPopover.Content>
        </RadixPopover.Portal>
      </RadixPopover.Root>
      <p id={`${ids}-hint`} className={FIELD_HINT}>
        Any number of labels that are not areas, such as bug or database.
      </p>
    </div>
  );
}
