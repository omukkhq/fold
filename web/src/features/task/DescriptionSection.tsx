import { zodResolver } from "@hookform/resolvers/zod";
import { Pencil } from "lucide-react";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { setDescription, type TaskDetail } from "@/api/tasks";
import { Button } from "@/components/button/Button";
import { Textarea } from "@/components/form/controls";
import { Markdown } from "./Markdown";
import { SectionHead, SmallAction, submit, WriteError } from "./parts";
import { descriptionSchema, type DescriptionForm } from "./schemas";
import { useTaskWrite } from "./useTaskWrite";

function DescriptionEditor({
  slug,
  task,
  onDone,
}: {
  slug: string;
  task: TaskDetail;
  onDone: () => void;
}) {
  const write = useTaskWrite(slug, task.task, (description: string) =>
    setDescription(slug, task.task, description),
  );
  const form = useForm<DescriptionForm>({
    resolver: zodResolver(descriptionSchema),
    defaultValues: { description: task.description },
  });
  return (
    <form
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          write.mutate(values.description, { onSuccess: onDone });
        }),
      )}
    >
      <Textarea
        label="Description"
        hint="Markdown, with tables and task lists. HTML is not shown."
        rows={12}
        autoFocus
        disabled={write.isPending}
        error={form.formState.errors.description?.message}
        className="font-mono text-meta"
        {...form.register("description")}
      />
      <WriteError error={write.error} />
      <div className="flex gap-8">
        <Button type="submit" variant="primary" pending={write.isPending}>
          Save description
        </Button>
        <Button variant="ghost" disabled={write.isPending} onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

export function DescriptionSection({
  slug,
  task,
}: {
  slug: string;
  task: TaskDetail;
}) {
  const [editing, setEditing] = useState(false);
  return (
    <section aria-label="Description" className="flex flex-col gap-10">
      <SectionHead title="Description">
        <span className="flex-1" />
        {editing ? null : (
          <SmallAction
            icon={Pencil}
            aria-label="Edit the description"
            onClick={() => {
              setEditing(true);
            }}
          >
            Edit
          </SmallAction>
        )}
      </SectionHead>
      {editing ? (
        <DescriptionEditor
          slug={slug}
          task={task}
          onDone={() => {
            setEditing(false);
          }}
        />
      ) : task.description.trim() === "" ? (
        <p className="text-body text-text-muted">No description yet.</p>
      ) : (
        <Markdown>{task.description}</Markdown>
      )}
    </section>
  );
}
