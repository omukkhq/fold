import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Controller, useForm } from "react-hook-form";
import { keys } from "@/api/keys";
import { createTask } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { cn } from "@/lib/cn";
import { MentionField } from "@/mentions/MentionField";
import { DialogFrame } from "./DialogFrame";
import { KIND_OPTIONS } from "./humanTask";
import { submit, WriteError } from "./parts";
import { newTaskSchema, type NewTaskForm } from "./schemas";

export interface NewTaskDialogProps {
  slug: string;
  onClose: () => void;
  /** Opens the created task in the dialog. */
  onCreated: (task: string) => void;
}

/** The dialog's create form. It stays pending until the daemon names the new task and the board has been read again. */
export function NewTaskDialog({
  slug,
  onClose,
  onCreated,
}: NewTaskDialogProps) {
  const queryClient = useQueryClient();
  const create = useMutation({
    mutationFn: async (values: NewTaskForm) => {
      const task = await createTask(slug, values);
      await queryClient.invalidateQueries({ queryKey: keys.board(slug) });
      return task;
    },
    onSuccess: onCreated,
  });
  const form = useForm<NewTaskForm>({
    resolver: zodResolver(newTaskSchema),
    defaultValues: { title: "", description: "", kind: "proposal" },
  });
  return (
    <DialogFrame crumb="New task" crumbIsId={false} described onClose={onClose}>
      <form
        className="flex flex-col gap-16 px-24 pt-22 pb-24"
        onSubmit={submit(
          form.handleSubmit((values) => {
            create.mutate(values);
          }),
        )}
      >
        <div className="flex flex-col gap-4">
          <RadixDialog.Title className="text-page leading-snug font-semibold text-text">
            New task
          </RadixDialog.Title>
          <RadixDialog.Description className="text-bar text-text-muted">
            fold calculates its status. Add labels, checklists and dependencies
            once it exists.
          </RadixDialog.Description>
        </div>
        <TextField
          label="Title"
          autoFocus
          disabled={create.isPending}
          error={form.formState.errors.title?.message}
          {...form.register("title")}
        />
        <Controller
          control={form.control}
          name="description"
          render={({ field }) => (
            <MentionField
              slug={slug}
              label="Description"
              hint="Markdown, with tables and task lists. HTML is not shown. Type # for an item and @ for a file."
              rows={8}
              disabled={create.isPending}
              error={form.formState.errors.description?.message}
              className="font-mono text-meta"
              ref={field.ref}
              value={field.value}
              onValueChange={field.onChange}
              onBlur={field.onBlur}
            />
          )}
        />
        <Controller
          control={form.control}
          name="kind"
          render={({ field }) => (
            <fieldset className="flex flex-col gap-8">
              <legend className="mb-8 text-bar leading-snug font-semibold text-text">
                Who does the work
              </legend>
              {KIND_OPTIONS.map((option) => {
                const chosen = field.value === option.value;
                return (
                  <label
                    key={option.value}
                    className={cn(
                      "flex cursor-pointer gap-10 rounded-m px-12 py-9 inset-ring has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-focus-ring",
                      chosen
                        ? "bg-sunken inset-ring-ink"
                        : "bg-surface inset-ring-border-subtle",
                      create.isPending && "cursor-default opacity-60",
                    )}
                  >
                    <input
                      type="radio"
                      name={field.name}
                      value={option.value}
                      checked={chosen}
                      disabled={create.isPending}
                      onChange={() => {
                        field.onChange(option.value);
                      }}
                      onBlur={field.onBlur}
                      className="mt-2 size-16 shrink-0 accent-ink"
                    />
                    <span className="flex min-w-0 flex-col gap-2">
                      <span className="text-bar font-medium text-text">
                        {option.title}
                      </span>
                      <span className="text-meta leading-normal text-text-muted">
                        {option.hint}
                      </span>
                    </span>
                  </label>
                );
              })}
            </fieldset>
          )}
        />
        <WriteError error={create.error} />
        <div className="flex gap-8">
          <Button
            type="submit"
            variant="primary"
            pending={create.isPending}
            pendingLabel="Creating task…"
          >
            Create task
          </Button>
          <Button variant="ghost" disabled={create.isPending} onClick={onClose}>
            Cancel
          </Button>
        </div>
      </form>
    </DialogFrame>
  );
}
