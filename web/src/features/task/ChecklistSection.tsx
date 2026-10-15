import { zodResolver } from "@hookform/resolvers/zod";
import {
  ArrowDown,
  ArrowUp,
  Check,
  Ellipsis,
  Link2,
  Pencil,
  Plus,
  Trash2,
} from "lucide-react";
import { useRouterState } from "@tanstack/react-router";
import { useState } from "react";
import { useForm, useWatch } from "react-hook-form";
import {
  addChecklistItem,
  moveChecklistItem,
  removeChecklistItem,
  renameChecklistItem,
  tickChecklistItem,
  type Checklist,
  type LinkedItem,
  type StepItem,
  type TaskDetail,
} from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { Button, IconButton } from "@/components/button/Button";
import { Checkbox, Select, TextField } from "@/components/form/controls";
import { Mono } from "@/components/mono/Mono";
import { Menu, type MenuItem } from "@/components/overlay/Menu";
import { taskPath } from "@/features/board/card";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  isTickable,
  linkAddText,
  linkedStateText,
  linkRemoveText,
  moveTargets,
  progress,
} from "./checklist";
import { SectionHead, SmallAction, submit, WriteError } from "./parts";
import {
  checklistItemSchema,
  renameItemSchema,
  type ChecklistItemForm,
  type RenameItemForm,
} from "./schemas";
import { statusLook } from "./text";
import { useTaskWrite } from "./useTaskWrite";

interface RowProps {
  slug: string;
  task: string;
  index: number;
  count: number;
}

function useRowWrites(slug: string, task: string, item: string) {
  const move = useTaskWrite(slug, task, (position: number) =>
    moveChecklistItem(slug, item, position),
  );
  const remove = useTaskWrite(slug, task, () =>
    removeChecklistItem(slug, item),
  );
  return { move, remove };
}

function rowMenu(
  index: number,
  count: number,
  onMove: (position: number) => void,
  onRemove: () => void,
  onRename: (() => void) | null,
): MenuItem[] {
  const targets = moveTargets(index, count);
  const items: MenuItem[] = [];
  if (onRename) {
    items.push({ label: "Rename", icon: Pencil, onSelect: onRename });
  }
  items.push(
    {
      label: "Move up",
      icon: ArrowUp,
      disabled: targets.up === null,
      onSelect: () => {
        if (targets.up !== null) onMove(targets.up);
      },
    },
    {
      label: "Move down",
      icon: ArrowDown,
      disabled: targets.down === null,
      onSelect: () => {
        if (targets.down !== null) onMove(targets.down);
      },
    },
    { label: "Remove", icon: Trash2, danger: true, onSelect: onRemove },
  );
  return items;
}

// W03 draws a row as its text alone, 27 high: the menu overlaps the row padding, and where a pointer can hover it
// shows only on hover, on focus, while open or while its write is pending.
const ROW_MENU =
  "-my-4 pointer-fine:opacity-0 pointer-fine:group-hover:opacity-100 pointer-fine:group-focus-within:opacity-100 pointer-fine:aria-busy:opacity-100 pointer-fine:data-[state=open]:opacity-100";

function RowMenu({
  name,
  items,
  pending,
}: {
  name: string;
  items: MenuItem[];
  pending: boolean;
}) {
  return (
    <Menu
      align="end"
      items={items}
      trigger={
        <IconButton
          size="sm"
          aria-label={`Actions for ${name}`}
          pending={pending}
          className={ROW_MENU}
        >
          <Ellipsis aria-hidden="true" size={13} />
        </IconButton>
      }
    />
  );
}

function RenameForm({
  slug,
  task,
  item,
  onDone,
}: {
  slug: string;
  task: string;
  item: StepItem;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task, (text: string) =>
    renameChecklistItem(slug, item.id, text),
  );
  const form = useForm<RenameItemForm>({
    resolver: zodResolver(renameItemSchema),
    defaultValues: { text: item.text },
  });
  return (
    <li className="flex flex-col gap-8 px-6 py-5">
      <form
        className="flex flex-col gap-8"
        onSubmit={submit(
          form.handleSubmit((values) => {
            write.mutate(values.text, { onSuccess: onDone });
          }),
        )}
      >
        <TextField
          label="Step"
          autoFocus
          disabled={write.isPending}
          error={form.formState.errors.text?.message}
          {...form.register("text")}
        />
        <WriteError error={write.error} />
        <div className="flex gap-8">
          <Button
            type="submit"
            variant="primary"
            size="sm"
            pending={write.isPending}
          >
            Save step
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={write.isPending}
            onClick={onDone}
          >
            Cancel
          </Button>
        </div>
      </form>
    </li>
  );
}

function StepRow({
  slug,
  task,
  index,
  count,
  item,
}: RowProps & { item: StepItem }) {
  const [renaming, setRenaming] = useState(false);
  const tick = useTaskWrite(slug, task, (done: boolean) =>
    tickChecklistItem(slug, item.id, done),
  );
  const { move, remove } = useRowWrites(slug, task, item.id);
  if (renaming) {
    return (
      <RenameForm
        slug={slug}
        task={task}
        item={item}
        onDone={() => {
          setRenaming(false);
        }}
      />
    );
  }
  const error = tick.error ?? move.error ?? remove.error;
  return (
    <li data-item={item.id} className="flex flex-col gap-4">
      <div className="group flex items-center gap-10 px-6 py-5">
        <div className={cn("min-w-0 flex-1", item.done && "text-text-muted")}>
          <Checkbox
            label={item.text}
            checked={item.done}
            disabled={tick.isPending}
            aria-busy={tick.isPending ? true : undefined}
            onCheckedChange={(done) => {
              tick.mutate(done);
            }}
          />
        </div>
        {tick.isPending ? (
          <span className="text-meta text-text-muted">Saving…</span>
        ) : null}
        <RowMenu
          name={item.text}
          pending={move.isPending || remove.isPending}
          items={rowMenu(
            index,
            count,
            (position) => {
              move.mutate(position);
            },
            () => {
              remove.mutate(undefined);
            },
            () => {
              setRenaming(true);
            },
          )}
        />
      </div>
      <WriteError error={error} />
    </li>
  );
}

function LinkedRow({
  slug,
  task,
  index,
  count,
  item,
}: RowProps & { item: LinkedItem }) {
  const [confirming, setConfirming] = useState(false);
  const { move, remove } = useRowWrites(slug, task, item.id);
  const filters = useRouterState({ select: (s) => s.location.searchStr });
  const look = statusLook(item.status);
  return (
    <li data-item={item.id} className="flex flex-col gap-4">
      <div className="group flex items-center gap-10 px-6 py-5">
        {/* fold ticks a linked item; the mark shows the state and takes no input. */}
        <span
          aria-hidden="true"
          className="inline-flex size-16 shrink-0 items-center justify-center rounded-s bg-sunken text-text-muted"
        >
          {item.done ? <Check size={12} /> : null}
        </span>
        <Link2
          aria-hidden="true"
          size={14}
          className="shrink-0 text-text-muted"
        />
        <HrefLink
          href={taskPath(slug, item.task, filters)}
          className={cn(
            "inline-flex min-w-0 flex-1 items-center gap-8 rounded-s text-body text-text hover:underline",
            focusRing,
          )}
        >
          <Mono className="text-meta text-text-muted">{item.task}</Mono>
          <span className="truncate">{item.title}</span>
        </HrefLink>
        <span className="inline-flex shrink-0 items-center gap-5 text-meta text-text-muted">
          <span
            aria-hidden="true"
            className={cn("size-7 rounded-full", look.dot)}
          />
          {linkedStateText(item, look.text)}
        </span>
        <RowMenu
          name={`${item.task} ${item.title}`}
          pending={move.isPending || remove.isPending}
          items={rowMenu(
            index,
            count,
            (position) => {
              move.mutate(position);
            },
            () => {
              setConfirming(true);
            },
            null,
          )}
        />
      </div>
      {confirming ? (
        <div
          role="group"
          aria-label={`Remove the link to ${item.task}`}
          className="flex flex-col gap-8 rounded-m bg-sunken px-12 py-10"
        >
          <p className="text-bar text-text">
            {linkRemoveText(task, item.task)}
          </p>
          <div className="flex gap-8">
            <Button
              variant="danger"
              size="sm"
              pending={remove.isPending}
              onClick={() => {
                remove.mutate(undefined, {
                  onSuccess: () => {
                    setConfirming(false);
                  },
                });
              }}
            >
              Remove link
            </Button>
            <Button
              variant="ghost"
              size="sm"
              disabled={remove.isPending}
              onClick={() => {
                setConfirming(false);
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      ) : null}
      <WriteError error={move.error ?? remove.error} />
    </li>
  );
}

function AddItemForm({
  slug,
  task,
  checklist,
  onDone,
}: {
  slug: string;
  task: string;
  checklist: string;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task, (values: ChecklistItemForm) =>
    addChecklistItem(
      slug,
      task,
      checklist,
      values.kind === "step"
        ? { kind: "step", text: values.text }
        : { kind: "linked", task: values.text },
    ),
  );
  const form = useForm<ChecklistItemForm>({
    resolver: zodResolver(checklistItemSchema),
    defaultValues: { kind: "step", text: "" },
  });
  const kind = useWatch({ control: form.control, name: "kind" });
  return (
    <form
      aria-label={`Add to ${checklist}`}
      className="flex flex-col gap-8 rounded-m bg-sunken px-12 py-10"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values, { onSuccess: onDone });
        }),
      )}
    >
      <div className="flex gap-8">
        <Select
          label="Kind"
          disabled={write.isPending}
          {...form.register("kind")}
        >
          <option value="step">Plain step</option>
          <option value="linked">Linked task</option>
        </Select>
        <div className="flex-1">
          <TextField
            label={kind === "step" ? "Step" : "Task id"}
            placeholder={kind === "step" ? "Describe the step" : "T-12"}
            autoFocus
            disabled={write.isPending}
            hint={kind === "linked" ? linkAddText(task) : undefined}
            error={form.formState.errors.text?.message}
            {...form.register("text")}
          />
        </div>
      </div>
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button
          type="submit"
          variant="primary"
          size="sm"
          pending={write.isPending}
        >
          {kind === "step" ? "Add step" : "Link task"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={write.isPending}
          onClick={onDone}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}

function ChecklistBlock({
  slug,
  task,
  checklist,
}: {
  slug: string;
  task: string;
  checklist: Checklist;
}) {
  const [adding, setAdding] = useState(false);
  const p = progress(checklist);
  const count = checklist.items.length;
  return (
    <section aria-label={checklist.name} className="flex flex-col gap-10">
      <SectionHead title={checklist.name}>
        <span className="text-bar text-text-muted">{p.text}</span>
        <div
          role="progressbar"
          aria-label={`${checklist.name} progress`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={p.percent}
          className="h-6 flex-1 overflow-hidden rounded-full bg-sunken"
        >
          <div
            className="h-full rounded-full bg-primary"
            style={{ width: `${String(p.percent)}%` }}
          />
        </div>
      </SectionHead>
      {/* W03 draws the add row as the last row of the list, with no gap above it. */}
      <div className="flex flex-col">
        <ul className="flex flex-col">
          {checklist.items.map((item, index) =>
            isTickable(item) ? (
              <StepRow
                key={item.id}
                slug={slug}
                task={task}
                index={index}
                count={count}
                item={item}
              />
            ) : (
              <LinkedRow
                key={item.id}
                slug={slug}
                task={task}
                index={index}
                count={count}
                item={item}
              />
            ),
          )}
        </ul>
        {count === 0 ? (
          <p className="px-6 text-bar text-text-muted">No items yet.</p>
        ) : null}
        {adding ? (
          <AddItemForm
            slug={slug}
            task={task}
            checklist={checklist.name}
            onDone={() => {
              setAdding(false);
            }}
          />
        ) : (
          <SmallAction
            icon={Plus}
            className="self-start px-6 py-5 text-bar"
            onClick={() => {
              setAdding(true);
            }}
          >
            Add a step or link a task
          </SmallAction>
        )}
      </div>
    </section>
  );
}

const EMPTY_SUBTASKS: Checklist = { name: "Subtasks", items: [] };

/** Every checklist of the task; a task without one shows an empty `Subtasks` to add to. */
export function ChecklistSection({
  slug,
  task,
}: {
  slug: string;
  task: TaskDetail;
}) {
  const lists = task.checklists.length > 0 ? task.checklists : [EMPTY_SUBTASKS];
  return (
    <>
      {lists.map((checklist) => (
        <ChecklistBlock
          key={checklist.name}
          slug={slug}
          task={task.task}
          checklist={checklist}
        />
      ))}
    </>
  );
}
