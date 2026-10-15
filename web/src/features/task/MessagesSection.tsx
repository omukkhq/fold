import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, Send } from "lucide-react";
import { useId } from "react";
import { useForm, useWatch } from "react-hook-form";
import {
  messageTargetsQuery,
  sendFeedback,
  type FeedbackRecipient,
  type TaskDetail,
  type TaskMessage,
} from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { Avatar } from "@/components/avatar/Avatar";
import { Button } from "@/components/button/Button";
import { controlClasses } from "@/components/form/Field";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { threadPath } from "./links";
import { SectionHead, submit, WriteError } from "./parts";
import { feedbackSchema, type FeedbackForm } from "./schemas";
import {
  clockTime,
  defaultRecipient,
  feedbackPlaceholder,
  messageCountText,
  parseRecipient,
  recipientValue,
} from "./text";
import { useTaskWrite } from "./useTaskWrite";

function Message({ slug, message }: { slug: string; message: TaskMessage }) {
  return (
    <li className="flex flex-col gap-4 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle">
      <div className="flex items-center gap-8">
        <span className="inline-flex items-center gap-6">
          <Avatar name={message.from} />
          <span className="font-mono text-caption text-text-muted">
            {message.from}
          </span>
        </span>
        <ArrowRight aria-label="to" size={12} className="text-text-faint" />
        <span className="font-mono text-caption text-text-muted">
          {message.to}
        </span>
        <HrefLink
          href={threadPath(slug, message.thread)}
          aria-label={`Open the thread of ${message.id}, sent ${clockTime(message.at)}`}
          className={cn(
            "ml-auto rounded-s text-meta text-text-faint hover:underline",
            focusRing,
          )}
        >
          {clockTime(message.at)}
        </HrefLink>
      </div>
      <p className="text-body leading-normal whitespace-pre-wrap text-text">
        {message.body}
      </p>
    </li>
  );
}

function FeedbackBox({
  slug,
  task,
  holder,
}: {
  slug: string;
  task: string;
  holder: string | null;
}) {
  const id = useId();
  const targets = useQuery(messageTargetsQuery(slug));
  const write = useTaskWrite(
    slug,
    task,
    (feedback: { body: string; to: FeedbackRecipient }) =>
      sendFeedback(slug, task, feedback),
  );
  const fallback = defaultRecipient(holder);
  const form = useForm<FeedbackForm>({
    resolver: zodResolver(feedbackSchema),
    defaultValues: { to: fallback ? recipientValue(fallback) : "", body: "" },
  });
  const to = parseRecipient(useWatch({ control: form.control, name: "to" }));
  const errors = form.formState.errors;
  return (
    <form
      aria-label="Feedback"
      className="flex flex-col gap-8"
      onSubmit={submit(
        form.handleSubmit((values) => {
          const recipient = parseRecipient(values.to);
          if (recipient === null) {
            form.setError("to", { message: "Choose who reads the feedback." });
            return;
          }
          write.mutate(
            { body: values.body, to: recipient },
            {
              onSuccess: () => {
                form.reset({ to: values.to, body: "" });
              },
            },
          );
        }),
      )}
    >
      <div className="flex items-start gap-8">
        <label className="sr-only" htmlFor={`${id}-to`}>
          Send to
        </label>
        <select
          id={`${id}-to`}
          disabled={write.isPending}
          aria-invalid={errors.to ? true : undefined}
          className={cn(controlClasses, focusRing, "w-auto shrink-0")}
          {...form.register("to")}
        >
          {holder === null ? <option value="">Choose a reader</option> : null}
          {holder === null ? null : (
            <option value="holder">{holder} (claim holder)</option>
          )}
          {(targets.data?.agents ?? []).map((agent) => (
            <option key={`agent:${agent}`} value={`agent:${agent}`}>
              {agent}
            </option>
          ))}
          {(targets.data?.tags ?? []).map((tag) => (
            <option key={`tag:${tag}`} value={`tag:${tag}`}>
              tag {tag}
            </option>
          ))}
        </select>
        <label className="sr-only" htmlFor={`${id}-body`}>
          Feedback
        </label>
        <input
          id={`${id}-body`}
          autoComplete="off"
          disabled={write.isPending}
          placeholder={feedbackPlaceholder(to, holder)}
          aria-invalid={errors.body ? true : undefined}
          className={cn(controlClasses, focusRing, "min-w-0 flex-1")}
          {...form.register("body")}
        />
        <Button
          type="submit"
          variant="primary"
          pending={write.isPending}
          icon={<Send aria-hidden="true" size={15} />}
        >
          Send
        </Button>
      </div>
      {(errors.to?.message ?? errors.body?.message) ? (
        <p className="text-meta font-medium text-danger">
          {errors.to?.message ?? errors.body?.message}
        </p>
      ) : null}
      <WriteError error={write.error} />
    </form>
  );
}

/** Messages linked to the task, and the feedback box that adds one from the signed-in human. */
export function MessagesSection({
  slug,
  task,
}: {
  slug: string;
  task: TaskDetail;
}) {
  const holder =
    task.claims.find((c) => c.kind === "implementation")?.holder ?? null;
  return (
    <section aria-label="Messages" className="flex flex-col gap-10">
      <SectionHead title="Messages">
        <span className="text-bar text-text-muted">
          {messageCountText(task.messages.length, task.task)}
        </span>
      </SectionHead>
      {task.messages.length > 0 ? (
        <ul className="flex flex-col gap-10">
          {task.messages.map((message) => (
            <Message key={message.id} slug={slug} message={message} />
          ))}
        </ul>
      ) : null}
      <FeedbackBox slug={slug} task={task.task} holder={holder} />
    </section>
  );
}
