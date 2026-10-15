import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { Pencil, Plus, X } from "lucide-react";
import { useForm } from "react-hook-form";
import { addLabel, removeLabel, setTitle, type TaskDetail } from "@/api/tasks";
import { Avatar } from "@/components/avatar/Avatar";
import { Button, IconButton } from "@/components/button/Button";
import { Select, TextField } from "@/components/form/controls";
import { Label } from "@/components/label/Label";
import { LABEL_NAMES } from "@/components/label/labelColors";
import { Popover } from "@/components/overlay/Popover";
import { useState } from "react";
import { cn } from "@/lib/cn";
import type { LiveStrip } from "./liveStrip";
import { SmallAction, submit, WriteError } from "./parts";
import {
  labelSchema,
  titleSchema,
  type LabelForm,
  type TitleForm,
} from "./schemas";
import { statusLook } from "./text";
import { useTaskWrite } from "./useTaskWrite";

interface Props {
  slug: string;
  task: TaskDetail;
  strip: LiveStrip | null;
}

function TitleEditor({
  slug,
  task,
  onDone,
}: {
  slug: string;
  task: TaskDetail;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task.task, (title: string) =>
    setTitle(slug, task.task, title),
  );
  const form = useForm<TitleForm>({
    resolver: zodResolver(titleSchema),
    defaultValues: { title: task.title },
  });
  return (
    <form
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values.title, { onSuccess: onDone });
        }),
      )}
    >
      <TextField
        label="Title"
        autoFocus
        disabled={write.isPending}
        error={form.formState.errors.title?.message}
        {...form.register("title")}
      />
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button type="submit" variant="primary" pending={write.isPending}>
          Save title
        </Button>
        <Button variant="ghost" disabled={write.isPending} onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function LabelEditor({ slug, task }: { slug: string; task: TaskDetail }) {
  const add = useTaskWrite(slug, task.task, (label: LabelForm) =>
    addLabel(slug, task.task, label),
  );
  const remove = useTaskWrite(slug, task.task, (name: string) =>
    removeLabel(slug, task.task, name),
  );
  const form = useForm<LabelForm>({
    resolver: zodResolver(labelSchema),
    defaultValues: { name: "", color: "sage" },
  });
  return (
    <div className="flex w-search flex-col gap-12">
      {task.labels.length > 0 ? (
        <ul aria-label="Labels on this task" className="flex flex-col gap-4">
          {task.labels.map((label) => (
            <li
              key={label.name}
              className="flex items-center justify-between gap-8"
            >
              <Label color={label.color}>{label.name}</Label>
              <IconButton
                size="sm"
                aria-label={`Remove label ${label.name}`}
                pending={remove.isPending && remove.variables === label.name}
                disabled={remove.isPending}
                onClick={() => {
                  remove.mutate(label.name);
                }}
              >
                <X aria-hidden="true" size={13} />
              </IconButton>
            </li>
          ))}
        </ul>
      ) : null}
      <WriteError error={remove.error} />
      <form
        className="flex flex-col gap-8"
        onSubmit={submit(
          form.handleSubmit((values) => {
            add.mutate(values, {
              onSuccess: () => {
                form.reset({ name: "", color: values.color });
              },
            });
          }),
        )}
      >
        <TextField
          label="Label name"
          disabled={add.isPending}
          error={form.formState.errors.name?.message}
          {...form.register("name")}
        />
        <Select
          label="Colour"
          disabled={add.isPending}
          error={form.formState.errors.color?.message}
          {...form.register("color")}
        >
          {LABEL_NAMES.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </Select>
        <WriteError error={add.error} />
        <Button type="submit" variant="primary" pending={add.isPending}>
          Add label
        </Button>
      </form>
    </div>
  );
}

function Strip({ strip }: { strip: LiveStrip }) {
  return (
    <div
      aria-label="Live"
      className={cn(
        "flex min-w-0 items-center gap-10 rounded-m px-12 py-8",
        strip.stalled ? "bg-secondary-soft" : "bg-primary-soft",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-8 shrink-0 rounded-full",
          strip.stalled ? "bg-secondary" : "bg-primary",
        )}
      />
      <span className="inline-flex shrink-0 items-center gap-6">
        <Avatar name={strip.worker} />
        <span className="font-mono text-meta text-primary-text">
          {strip.worker}
        </span>
      </span>
      <span
        className={cn(
          "shrink-0 text-bar",
          strip.stalled ? "text-secondary-text" : "text-primary-text",
        )}
      >
        {strip.verb}
      </span>
      {strip.file ? (
        <span className="min-w-0 truncate font-mono text-meta font-semibold text-primary-text">
          {strip.file}
        </span>
      ) : null}
      <span className="min-w-0 flex-1 truncate text-bar text-text-muted">
        {strip.rest}
      </span>
    </div>
  );
}

/** W03's title block: the title, the calculated status, the live strip and the labels. */
export function TitleBlock({ slug, task, strip }: Props) {
  const [editing, setEditing] = useState(false);
  const look = statusLook(task.status);
  return (
    <div className="flex flex-col gap-10">
      {editing ? (
        <>
          <RadixDialog.Title className="sr-only">
            {task.title}
          </RadixDialog.Title>
          <TitleEditor
            slug={slug}
            task={task}
            onDone={() => {
              setEditing(false);
            }}
          />
        </>
      ) : (
        <div className="flex items-start gap-8">
          <RadixDialog.Title className="min-w-0 flex-1 text-page leading-snug font-semibold text-text">
            {task.title}
          </RadixDialog.Title>
          <IconButton
            aria-label="Edit the title"
            onClick={() => {
              setEditing(true);
            }}
          >
            <Pencil aria-hidden="true" size={15} />
          </IconButton>
        </div>
      )}
      <div className="flex flex-wrap items-center gap-10">
        <span
          className={cn(
            "inline-flex items-center gap-6 rounded-s px-9 py-3 text-meta font-semibold",
            look.pill,
          )}
        >
          <span
            aria-hidden="true"
            className={cn("size-7 rounded-full", look.dot)}
          />
          {look.text}
        </span>
        {task.blocked ? (
          <span className="text-bar font-medium text-danger">Blocked</span>
        ) : null}
        <RadixDialog.Description className="text-bar text-text-muted">
          {task.status_reason}
        </RadixDialog.Description>
      </div>
      {strip ? <Strip strip={strip} /> : null}
      <div className="flex flex-wrap items-center gap-6">
        {task.labels.map((label) => (
          <Label key={label.name} color={label.color}>
            {label.name}
          </Label>
        ))}
        <Popover trigger={<SmallAction icon={Plus}>Label</SmallAction>}>
          <LabelEditor slug={slug} task={task} />
        </Popover>
      </div>
    </div>
  );
}
