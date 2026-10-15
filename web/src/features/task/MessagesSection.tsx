import { PersonList, PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Send } from "lucide-react";
import { useId } from "react";
import { Controller, useForm, useWatch } from "react-hook-form";
import { keys } from "@/api/keys";
import { olderNotListed } from "@/api/pages";
import {
  messageTargetsQuery,
  sendFeedback,
  taskMessagesQuery,
  type FeedbackRecipient,
  type TaskDetail,
  type TaskMessage,
} from "@/api/tasks";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { controlClasses } from "@/components/form/Field";
import { MessageBody } from "@/features/messages/MessageBody";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionTextarea } from "@/mentions/MentionTextarea";
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
        <PersonName
          name={message.sender}
          className="font-mono text-caption text-text-muted"
        />
        <ArrowRight aria-label="to" size={12} className="text-text-faint" />
        <PersonList
          names={message.recipients.map((r) => r.name)}
          className="font-mono text-caption text-text-muted"
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
  const targets = useQuery(messageTargetsQuery(slug));
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
