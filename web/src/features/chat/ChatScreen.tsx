import { zodResolver } from "@hookform/resolvers/zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, MessagesSquare, Pencil, Send } from "lucide-react";
import { useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { apiErrorCode } from "@/api/errors";
import { keys } from "@/api/keys";
import { sendChat } from "@/api/messages";
import { chatQuery, sessionQuery, workersQuery } from "@/api/queries";
import { updateArea, type AreaEntry } from "@/api/workers";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { Textarea } from "@/components/form/controls";
import { EmptyPanel } from "@/components/states/EmptyPanel";
import { FailurePanel } from "@/components/states/FailurePanel";
import { Skeleton } from "@/components/states/Skeleton";
import { failureText } from "@/format/failure";
import { formatAsOf } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { useNow } from "@/live/useNow";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { PersonName } from "@/people/PersonName";
import { projectPath } from "@/routes/paths";
import { MessageBody } from "@/features/messages/MessageBody";
import { replySchema, type ReplyValues } from "@/features/messages/compose";
import { QuestionHeader } from "@/features/messages/QuestionChoices";
import {
  agentFacts,
  chatLines,
  instructionsChange,
  instructionsSchema,
  ledArea,
  noteDraft,
  type InstructionsValues,
} from "./chat";

const MINUTE_MS = 60_000;

export function ChatScreen({
  slug,
  agent,
  note,
}: {
  slug: string;
  agent: string;
  /** A decision note the human answers about; the message to write starts with its mention. */
  note?: string;
}) {
  const chat = useQuery(chatQuery(slug, agent));
  const workers = useQuery(workersQuery(slug));
  const session = useQuery(sessionQuery());
  const now = useNow(true, MINUTE_MS);
  const human = session.data?.human ?? null;
  const facts = agentFacts(workers.data?.workers ?? [], agent);
  const area = ledArea(workers.data?.areas ?? [], agent);

  return (
    <div className="flex flex-col gap-16 px-24 pt-20 pb-24">
      <nav
        aria-label="Breadcrumb"
        className="flex items-center gap-8 text-bar text-text-muted"
      >
        <HrefLink
          href={projectPath(slug, "agents")}
          className={cn("rounded-s hover:text-text", focusRing)}
        >
          Agents
        </HrefLink>
        <ChevronRight aria-hidden="true" size={13} />
        <span className="font-mono text-code text-text">{agent}</span>
      </nav>
      <header className="flex flex-col gap-4">
        <h1 className="flex items-center gap-8 text-screen leading-snug font-semibold text-text">
          Chat with <PersonName name={agent} />
        </h1>
        <p className="text-body text-text-muted">
          {facts.role} · {facts.delivery} The chat is not tied to a task, and a
          message never changes a task.
        </p>
      </header>
      <div className="flex items-start gap-16">
        <section
          aria-label="Chat"
          className="flex min-w-0 flex-1 flex-col rounded-l bg-surface inset-ring inset-ring-border-subtle"
        >
          {chat.isPending ? (
            <div className="flex flex-col gap-12 p-18">
              <Skeleton />
              <Skeleton />
            </div>
          ) : chat.isError ? (
            <FailurePanel
              className="m-18"
              title="The chat did not load."
              text={failureText(chat.error)}
              code={apiErrorCode(chat.error)}
            />
          ) : chat.data.messages.length === 0 ? (
            <EmptyPanel
              icon={MessagesSquare}
              title="No messages yet"
              className="m-18"
            >
              {`Write to ${agent} below, or run \`fld message ${agent} <text>\` in a terminal of the project.`}
            </EmptyPanel>
          ) : (
            <ol className="flex flex-col gap-12 p-18">
              {chatLines(chat.data.messages).map(({ message, fromHuman }) => (
                <li
                  key={message.id}
                  className={cn(
                    "flex flex-col gap-4 rounded-m px-12 py-10 inset-ring",
                    fromHuman
                      ? "ml-40 bg-sunken inset-ring-border"
                      : "mr-40 bg-surface inset-ring-border-subtle",
                  )}
                >
                  <p className="flex items-center gap-8 font-mono text-caption text-text-muted">
                    <PersonName name={message.sender} slot="messageHead" />
                    <span className="font-mono text-meta text-text-faint">
                      {message.id}
                    </span>
                    <span className="ml-auto font-ui text-meta text-text-faint">
                      {formatAsOf(now, message.sent_at)}
                    </span>
                  </p>
                  {message.header === null ? null : (
                    <QuestionHeader header={message.header} />
                  )}
                  <MessageBody body={message.body} slug={slug} />
                  {message.choices.length === 0 ? null : (
                    <>
                      <ul aria-label="Choices" className="flex flex-col gap-4">
                        {message.choices.map(({ label, description }) => (
                          <li
                            key={label}
                            className="flex flex-col gap-2 rounded-s bg-sunken px-10 py-6"
                          >
                            <span className="text-body font-medium text-text">
                              {label}
                            </span>
                            {description === null ? null : (
                              <span className="text-meta text-text-muted">
                                {description}
                              </span>
                            )}
                          </li>
                        ))}
                      </ul>
                      <p className="text-meta text-text-muted">
                        Answer below with the label of an option, or in your own
                        words.
                      </p>
                    </>
                  )}
                </li>
              ))}
            </ol>
          )}
          {session.isPending ? null : human === null ? (
            <p className="border-t border-border-subtle px-18 py-12 text-meta text-text-muted">
              Tell fold your name to write to {agent}.
            </p>
          ) : (
            <ChatForm slug={slug} agent={agent} note={note} />
          )}
        </section>
        {area === null ? null : (
          <AreaInstructions
            key={area.instructions ?? ""}
            slug={slug}
            area={area}
            canEdit={human !== null}
          />
        )}
      </div>
    </div>
  );
}

function ChatForm({
  slug,
  agent,
  note,
}: {
  slug: string;
  agent: string;
  note: string | undefined;
}) {
  const queryClient = useQueryClient();
  const send = useMutation({
    mutationFn: (body: string) => sendChat(slug, agent, body),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.chat(slug, agent) }),
  });
  const {
    control,
    handleSubmit,
    reset,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<ReplyValues>({
    resolver: zodResolver(replySchema),
    defaultValues: { body: noteDraft(note) },
  });
  const submit = handleSubmit(async ({ body }) => {
    try {
      await send.mutateAsync(body.trim());
      reset({ body: "" });
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });
  return (
    <form
      onSubmit={(event) => void submit(event)}
      noValidate
      className="flex flex-col gap-6 border-t border-border-subtle px-18 py-12"
    >
      <div className="flex items-center gap-8">
        <Controller
          control={control}
          name="body"
          render={({ field }) => (
            <MentionTextarea
              slug={slug}
              rows={2}
              aria-label="Message"
              placeholder={`Write to ${agent}…`}
              readOnly={isSubmitting}
              aria-invalid={errors.body ? true : undefined}
              className="resize-none py-9"
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
          pending={isSubmitting}
          pendingLabel="Sending…"
          icon={<Send aria-hidden="true" size={13} />}
        >
          Send
        </Button>
      </div>
      {errors.body ? (
        <p className="text-meta font-medium text-danger">
          {errors.body.message}
        </p>
      ) : null}
      {errors.root ? (
        <p role="alert" className="text-meta font-medium text-danger">
          The message was not sent. {errors.root.message}
        </p>
      ) : null}
    </form>
  );
}

function AreaInstructions({
  slug,
  area,
  canEdit,
}: {
  slug: string;
  area: AreaEntry;
  canEdit: boolean;
}) {
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState(false);
  const save = useMutation({
    mutationFn: (body: Parameters<typeof updateArea>[2]) =>
      updateArea(slug, area.name, body),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: keys.workers(slug) }),
  });
  const {
    register,
    handleSubmit,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<InstructionsValues>({
    resolver: zodResolver(instructionsSchema),
    defaultValues: { text: area.instructions ?? "" },
  });
  const submit = handleSubmit(async ({ text }) => {
    const change = instructionsChange(area.instructions, text);
    if (change === null) {
      setEditing(false);
      return;
    }
    try {
      await save.mutateAsync(change);
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });
  return (
    <section
      aria-label={`Instructions of area ${area.name}`}
      className="flex w-side-panel shrink-0 flex-col gap-10 rounded-l bg-surface px-16 py-14 inset-ring inset-ring-border-subtle"
    >
      <div className="flex items-center justify-between gap-8">
        <h2 className="text-card leading-snug font-semibold text-text">
          Area {area.name} instructions
        </h2>
        {canEdit && !editing ? (
          <Button
            size="sm"
            icon={<Pencil aria-hidden="true" size={13} />}
            onClick={() => {
              setEditing(true);
            }}
          >
            Edit
          </Button>
        ) : null}
      </div>
      <p className="text-meta text-text-muted">
        Lasting instructions for the lead and the task workers of the area. A
        running worker gets new instructions on its next turn, every other
        worker on its next start.
      </p>
      {editing ? (
        <form
          onSubmit={(event) => void submit(event)}
          noValidate
          className="flex flex-col gap-8"
        >
          <Textarea
            label="Instructions"
            rows={8}
            readOnly={isSubmitting}
            error={errors.text?.message}
            {...register("text")}
          />
          {errors.root ? (
            <p role="alert" className="text-meta font-medium text-danger">
              The instructions were not saved. {errors.root.message}
            </p>
          ) : null}
          <div className="flex items-center gap-8">
            <Button
              type="submit"
              variant="primary"
              size="sm"
              pending={isSubmitting}
              pendingLabel="Saving…"
            >
              Save
            </Button>
            <Button
              size="sm"
              disabled={isSubmitting}
              onClick={() => {
                setEditing(false);
              }}
            >
              Cancel
            </Button>
          </div>
        </form>
      ) : area.instructions === null ? (
        <p className="text-bar text-text-faint">No instructions yet.</p>
      ) : (
        <p className="text-bar leading-normal whitespace-pre-wrap text-text">
          {area.instructions}
        </p>
      )}
    </section>
  );
}
