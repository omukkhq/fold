import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { Pencil } from "lucide-react";
import { useForm } from "react-hook-form";
import { setTitle, type TaskDetail } from "@/api/tasks";
import { Button, IconButton } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { useState } from "react";
import { cn } from "@/lib/cn";
import { PersonName } from "@/people/PersonName";
import type { LiveStrip } from "./liveStrip";
import { submit, WriteError } from "./parts";
import { titleSchema, type TitleForm } from "./schemas";
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
        <Button
          type="submit"
          variant="primary"
          pending={write.isPending}
          pendingLabel="Saving title…"
        >
          Save title
        </Button>
        <Button variant="ghost" disabled={write.isPending} onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
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
      <span className="shrink-0 text-bar">
        <PersonName name={strip.worker} />
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

/** The title block: the title, the calculated status and the live strip. */
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
        {/* An empty reason draws nothing, so the line is left out for a task read without one. */}
        {task.status_reason ? (
          <RadixDialog.Description className="text-bar text-text-muted">
            {task.status_reason}
          </RadixDialog.Description>
        ) : null}
      </div>
      {strip ? <Strip strip={strip} /> : null}
    </div>
  );
}
