import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lightbulb } from "lucide-react";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { keys } from "@/api/keys";
import { sessionQuery } from "@/api/queries";
import { createTask } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { NewRequestDialog } from "@/features/requests/NewRequestDialog";
import { cn } from "@/lib/cn";
import { MentionField } from "@/mentions/MentionField";
import { NO_AREA, areaOptions, createLabels } from "./areas";
import { DialogFrame } from "./DialogFrame";
import { KIND_OPTIONS } from "./humanTask";
import { AreaField, LabelsField, useAreas } from "./LabelFields";
import { submit, WriteError } from "./parts";
import { newTaskSchema, type NewTaskForm } from "./schemas";

export interface NewTaskDialogProps {
  slug: string;
  onClose: () => void;
  /** Opens the created task in the dialog. */
  onCreated: (task: string) => void;
  /** Opens a request made from the hint instead of a task. */
  onRequestCreated: (request: string) => void;
}

/** Points to a request first: a lead plans the tasks of a request, with their areas, dependencies and checks. */
function RequestHint({ onRequest }: { onRequest: () => void }) {
  return (
    <div className="flex items-start gap-12 rounded-m bg-accent-soft px-14 py-12 inset-ring inset-ring-accent">
      <Lightbulb
        aria-hidden="true"
        size={18}
        className="mt-2 shrink-0 text-accent"
      />
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <p className="text-14 font-semibold text-accent">
          Usually a request works better
        </p>
        <p className="text-13 leading-normal text-text-2">
          Describe what you want. The main agent or an area lead plans the
          tasks, with areas, dependencies and checks, and you approve the draft.
          Make a task here only when you know exactly what one worker must do.
        </p>
      </div>
      <Button size="sm" className="shrink-0 self-center" onClick={onRequest}>
        New request
      </Button>
    </div>
  );
}

/** The dialog's create form. It stays pending until the daemon names the new task and the board has been read again. */
export function NewTaskDialog({
  slug,
  onClose,
  onCreated,
  onRequestCreated,
}: NewTaskDialogProps) {
  const queryClient = useQueryClient();
  const session = useQuery(sessionQuery());
  const { areas, labels } = useAreas(slug);
  const [requesting, setRequesting] = useState(false);
  const create = useMutation({
    mutationFn: async ({ area, labels: chosen, ...values }: NewTaskForm) => {
      const task = await createTask(slug, {
        ...values,
        labels: createLabels(areas ?? [], area, chosen),
      });
      await queryClient.invalidateQueries({ queryKey: keys.board(slug) });
      return task;
    },
    onSuccess: onCreated,
  });
  const form = useForm<NewTaskForm>({
    resolver: zodResolver(newTaskSchema),
    defaultValues: {
      title: "",
      description: "",
      kind: "proposal",
      area: NO_AREA,
      labels: [],
    },
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
        <RequestHint
          onRequest={() => {
            setRequesting(true);
          }}
        />
        <div className="flex flex-col gap-4">
          <RadixDialog.Title className="text-28 leading-tight font-semibold text-text">
            New task
          </RadixDialog.Title>
          <RadixDialog.Description className="text-14 text-text-2">
            fold calculates its status. Add checklists and dependencies once it
            exists.
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
              className="font-mono text-13"
              ref={field.ref}
              value={field.value}
              onValueChange={field.onChange}
              onBlur={field.onBlur}
            />
          )}
        />
        <div className="grid gap-16 sm:grid-cols-2">
          <Controller
            control={form.control}
            name="area"
            render={({ field }) => (
              <AreaField
                options={areaOptions(areas ?? [], labels)}
                value={field.value}
                onChange={field.onChange}
                disabled={create.isPending || areas === null}
              />
            )}
          />
          <Controller
            control={form.control}
            name="labels"
            render={({ field }) => (
              <LabelsField
                labels={labels}
                areas={areas ?? []}
                value={field.value}
                onChange={field.onChange}
                disabled={create.isPending || areas === null}
              />
            )}
          />
        </div>
        <Controller
          control={form.control}
          name="kind"
          render={({ field }) => (
            <fieldset className="flex flex-col gap-8">
              <legend className="mb-8 text-14 leading-tight font-semibold text-text">
                Who does the work
              </legend>
              {KIND_OPTIONS.map((option) => {
                const chosen = field.value === option.value;
                return (
                  <label
                    key={option.value}
                    className={cn(
                      "flex cursor-pointer gap-10 rounded-m px-12 py-9 inset-ring has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-accent",
                      chosen
                        ? "bg-sunken inset-ring-ink"
                        : "bg-surface inset-ring-line",
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
                      <span className="text-14 font-medium text-text">
                        {option.title}
                      </span>
                      <span className="text-13 leading-normal text-text-2">
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
      {/* Outside the form: a submit inside the request dialog's portal still bubbles through the React tree. */}
      <NewRequestDialog
        slug={slug}
        human={session.data?.human ?? null}
        open={requesting}
        onOpenChange={setRequesting}
        onCreated={onRequestCreated}
      />
    </DialogFrame>
  );
}
