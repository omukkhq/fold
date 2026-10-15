import * as RadixPopover from "@radix-ui/react-popover";
import { useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp } from "lucide-react";
import {
  forwardRef,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ComponentProps,
  type KeyboardEvent,
} from "react";
import { boardCards, boardQuery } from "@/api/board";
import { proposalsQuery, requestsQuery } from "@/api/queries";
import { controlClasses } from "@/components/form/Field";
import {
  repositoryQuery,
  revisionsQuery,
  treeQuery,
  workspaceTreeQuery,
} from "@/api/repositoryQueries";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { usePeople } from "./usePeople";
import {
  GROUP_LABELS,
  activeMention,
  applyPick,
  groupRuns,
  itemSuggestions,
  optionLook,
  optionMark,
  pathQuery,
  pathSuggestions,
  personSuggestions,
  stepIndex,
  type ActiveMention,
  type OptionLook,
  type Suggestion,
} from "./suggest";

const LOOKS: Record<OptionLook, string> = {
  mono: "font-mono text-code text-primary-text",
  human: "font-ui text-bar font-medium text-primary-text",
  agent: "font-ui text-bar font-medium text-secondary-text",
};

/** Reads the lists a `#` or `@` needs only while one is being typed, from the same cached queries the screens use. */
function useSuggestions(
  slug: string,
  active: ActiveMention | null,
): Suggestion[] {
  const items = active?.mark === "#";
  const paths = active?.mark === "@";
  const people = usePeople(paths ? slug : null);
  const board = useQuery({ ...boardQuery(slug), enabled: items });
  const proposals = useQuery({ ...proposalsQuery(slug), enabled: items });
  const requests = useQuery({ ...requestsQuery(slug), enabled: items });
  const revisions = useQuery({ ...revisionsQuery(slug), enabled: items });
  const repository = useQuery({ ...repositoryQuery(slug), enabled: paths });
  const query = paths ? pathQuery(active.query) : null;
  const head = repository.data?.head ?? null;
  const tree = useQuery({
    ...treeQuery(slug, head ?? "", query?.dir ?? ""),
    enabled: head !== null && query !== null && query.workspace === null,
  });
  const workspaceTree = useQuery({
    ...workspaceTreeQuery(slug, query?.workspace ?? "", query?.dir ?? ""),
    enabled: query?.workspace != null,
  });
  if (active === null) return [];
  if (active.mark === "#") {
    return itemSuggestions(active.query, {
      tasks: board.data
        ? boardCards(board.data).map((c) => ({ id: c.task, title: c.title }))
        : undefined,
      proposals: proposals.data?.map((p) => ({ id: p.id, title: p.title })),
      requests: requests.data?.map((r) => ({
        id: r.id,
        title: r.title ?? r.excerpt,
      })),
      revisions: revisions.data?.items.map((r) => ({
        id: r.id,
        title: r.title ?? "",
      })),
    });
  }
  const entries = query?.workspace == null ? tree.data : workspaceTree.data;
  const files = query && entries ? pathSuggestions(query, entries) : [];
  return [...personSuggestions(active.query, people), ...files];
}

export type MentionTextareaProps = Omit<
  ComponentProps<"textarea">,
  "value" | "onChange" | "ref"
> & {
  slug: string;
  value: string;
  onValueChange: (value: string) => void;
};

/**
 * A textarea that suggests fold items after `#` and files after `@`. Focus stays in the textarea: the arrow keys move through the suggestions, Enter or Tab writes the highlighted one and Escape closes the list.
 */
export const MentionTextarea = forwardRef<
  HTMLTextAreaElement,
  MentionTextareaProps
>(function MentionTextarea(
  { slug, value, onValueChange, className, onKeyDown, onBlur, ...rest },
  ref,
) {
  const inner = useRef<HTMLTextAreaElement | null>(null);
  // A pick moves the caret behind what it wrote once React has put the new value in the textarea.
  const placeCaret = useRef<number | null>(null);
  useLayoutEffect(() => {
    const at = placeCaret.current;
    if (at !== null && inner.current) {
      inner.current.setSelectionRange(at, at);
      placeCaret.current = null;
    }
  });
  const [caret, setCaret] = useState<number | null>(null);
  const [index, setIndex] = useState(0);
  const [dismissed, setDismissed] = useState<number | null>(null);
  const listId = useId();
  const active = caret === null ? null : activeMention(value, caret);
  const suggestions = useSuggestions(slug, active);
  const open =
    active !== null && dismissed !== active.start && suggestions.length > 0;
  const current = Math.min(index, Math.max(0, suggestions.length - 1));

  const track = (element: HTMLTextAreaElement) => {
    setCaret(
      element.selectionStart === element.selectionEnd
        ? element.selectionStart
        : null,
    );
  };

  const pick = (suggestion: Suggestion) => {
    if (active === null) return;
    const next = applyPick(value, active, suggestion.insert, suggestion.open);
    onValueChange(next.text);
    setCaret(next.caret);
    setIndex(0);
    placeCaret.current = next.caret;
  };

  const keys = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!open) return false;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      setIndex(
        stepIndex(
          current,
          suggestions.length,
          event.key === "ArrowDown" ? 1 : -1,
        ),
      );
      return true;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      const chosen = suggestions[current];
      if (chosen) pick(chosen);
      return true;
    }
    if (event.key === "Escape") {
      setDismissed(active.start);
      return true;
    }
    return false;
  };

  return (
    <RadixPopover.Root
      open={open}
      onOpenChange={(next) => {
        if (!next && active !== null) setDismissed(active.start);
      }}
    >
      <RadixPopover.Anchor asChild>
        <textarea
          {...rest}
          ref={(element) => {
            inner.current = element;
            if (typeof ref === "function") ref(element);
            else if (ref) ref.current = element;
          }}
          value={value}
          role="combobox"
          aria-expanded={open}
          aria-controls={open ? listId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={
            open ? `${listId}-${String(current)}` : undefined
          }
          className={cn(controlClasses, focusRing, className)}
          onChange={(event) => {
            onValueChange(event.target.value);
            track(event.target);
            setIndex(0);
          }}
          onSelect={(event) => {
            track(event.currentTarget);
          }}
          onKeyDown={(event) => {
            if (keys(event)) {
              event.preventDefault();
              return;
            }
            onKeyDown?.(event);
          }}
          onBlur={(event) => {
            setCaret(null);
            onBlur?.(event);
          }}
        />
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
            if (event.target === inner.current) event.preventDefault();
          }}
          className="z-20 flex w-side-panel flex-col rounded-m bg-raised py-6 shadow-dialog inset-ring inset-ring-border outline-none"
        >
          <ul
            id={listId}
            role="listbox"
            aria-label="Suggestions"
            className="flex flex-col"
          >
            {groupRuns(suggestions).map((run) => (
              <li key={run.group} role="presentation" className="flex flex-col">
                <p className="px-12 pt-6 pb-4 text-overline leading-snug font-semibold tracking-overline text-text-muted uppercase">
                  {GROUP_LABELS[run.group]}
                </p>
                <ul role="presentation" className="flex flex-col px-4">
                  {run.items.map(({ suggestion, index: at }) => (
                    <li
                      key={`${suggestion.group}:${suggestion.detail}:${suggestion.insert}`}
                      id={`${listId}-${String(at)}`}
                      role="option"
                      aria-selected={at === current}
                      className={cn(
                        "flex cursor-pointer items-center gap-8 rounded-s px-8 py-5",
                        at === current && "bg-sunken",
                      )}
                      onMouseDown={(event) => {
                        event.preventDefault();
                        pick(suggestion);
                      }}
                    >
                      <span className={LOOKS[optionLook(suggestion)]}>
                        {optionMark(run.group)}
                        {suggestion.label}
                      </span>
                      <span className="min-w-0 truncate text-bar text-text-muted">
                        {suggestion.detail}
                      </span>
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
          <p className="mt-4 flex items-center gap-4 border-t border-border-subtle px-12 pt-6 text-caption text-text-faint">
            <ArrowUp aria-hidden="true" size={11} />
            <ArrowDown aria-hidden="true" size={11} />
            to move · Enter to insert · Esc to close
          </p>
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
});
