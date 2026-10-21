import { PersonList, PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Send } from "lucide-react";
import { useEffect, useId, useMemo } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { keys } from "@/api/keys";
import { olderNotListed } from "@/api/pages";
import {
  sendFeedback,
  taskMessagesQuery,
  taskReadersQuery,
  type FeedbackRecipient,
  type TaskDetail,
  type TaskMessage,
} from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { controlClasses } from "@/components/form/Field";
import { MarkAnswered } from "@/features/messages/MarkAnswered";
import { taskCanMarkAnswered } from "@/features/messages/answered";
import { MessageBody } from "@/features/messages/MessageBody";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { threadPath } from "./links";
import { keptChoice, recipientOptions } from "./recipients";
import { SectionHead, submit, WriteError } from "./parts";
import { feedbackSchema, type FeedbackForm } from "./schemas";
import {
  clockTime,
  feedbackPlaceholder,
  messageCountText,
  parseRecipient,
} from "./text";
import { useTaskWrite } from "./useTaskWrite";

function Message({ slug, message }: { slug: string; message: TaskMessage }) {
  return (
    <li className="flex flex-col gap-4 rounded-m bg-surface px-12 py-10 inset-ring inset-ring-border-subtle">
      <div className="flex items-center gap-8">
        <PersonName name={message.sender} slot="messageHead" />
        <ArrowRight aria-label="to" size={12} className="text-text-faint" />
        <PersonList
          names={message.recipients.map((r) => r.name)}
          className="font-mono text-caption text-text-muted"
          nameClassName="text-bar"
        />
        <HrefLink
          href={threadPath(slug, message.thread)}
          aria-label={`Open the thread of ${message.id}, sent ${clockTime(message.sent_at)}`}
          className={cn(
            "ml-auto rounded-s text-meta text-text-faint hover:underline",
            focusRing,
          )}
        >
          {clockTime(message.sent_at)}
        </HrefLink>
      </div>
      <MessageBody body={message.body} slug={slug} />
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
  const queryClient = useQueryClient();
  const readers = useQuery(taskReadersQuery(slug, task));
  const options = useMemo(
    () => recipientOptions(readers.data?.readers),
    [readers.data],
  );
  // The sent message joins the list before the box clears, so the write stays pending until the list is read again.
  const write = useTaskWrite(
    slug,
    task,
    async (feedback: { body: string; to: FeedbackRecipient }) => {
      await sendFeedback(slug, task, feedback);
      await queryClient.invalidateQueries({
        queryKey: keys.taskMessages(slug, task),
      });
    },
  );
  const form = useForm<FeedbackForm>({
    resolver: zodResolver(feedbackSchema),
    defaultValues: { to: keptChoice("", options), body: "" },
  });
  // The readers arrive after the box and change with the claim and the task's area; a choice the daemon stops offering falls back to the default.
  useEffect(() => {
    const current = form.getValues("to");
    const kept = keptChoice(current, options);
    if (kept !== current) {
      form.setValue("to", kept);
    }
  }, [form, options]);
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
          {options.length === 0 ? (
            <option value="">Choose a reader</option>
          ) : null}
          {options.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
        <label className="sr-only" htmlFor={`${id}-body`}>
          Feedback
        </label>
        <Controller
          control={form.control}
          name="body"
          render={({ field }) => (
            <MentionTextarea
              slug={slug}
              id={`${id}-body`}
              rows={1}
              disabled={write.isPending}
              placeholder={feedbackPlaceholder(to, holder)}
              aria-invalid={errors.body ? true : undefined}
              className="min-w-0 flex-1 resize-none py-9"
              ref={field.ref}
              value={field.value}
              onValueChange={field.onChange}
              onBlur={field.onBlur}
            />
          )}
        />
        <Button
          type="submit"
          variant="primary"
          pending={write.isPending}
          pendingLabel="Sending…"
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
      <WriteError error={readers.error ?? write.error} />
    </form>
  );
}

/** Messages linked to the task, and the feedback box that adds one from the signed-in human. The count shows once the messages are read. */
export function MessagesSection({
  slug,
  task,
}: {
  slug: string;
  task: TaskDetail;
}) {
  // Every claim of a task read is its implementation claim.
  const holder = task.claims[0]?.holder ?? null;
  const query = useQuery(taskMessagesQuery(slug, task.task));
  const messages = query.data?.items ?? [];
  return (
    <section aria-label="Messages" className="flex flex-col gap-10">
      <SectionHead title="Messages">
        {query.isSuccess ? (
          <span className="text-bar text-text-muted">
            {messageCountText(messages.length, task.task)}
          </span>
        ) : null}
        {taskCanMarkAnswered(task.attention) ? (
          <MarkAnswered
            slug={slug}
            scope={{ task: task.task }}
            className="ml-auto"
          />
        ) : null}
      </SectionHead>
      <WriteError error={query.error} />
      {messages.length > 0 ? (
        <ul className="flex flex-col gap-10">
          {messages.map((message) => (
            <Message key={message.id} slug={slug} message={message} />
          ))}
        </ul>
      ) : null}
      {query.data?.more ? (
        <p className="text-bar text-text-muted">
          {olderNotListed(messages.length, "messages")}
        </p>
      ) : null}
      <FeedbackBox slug={slug} task={task.task} holder={holder} />
    </section>
  );
}
