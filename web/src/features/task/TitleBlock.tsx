import { zodResolver } from "@hookform/resolvers/zod";
import * as RadixDialog from "@radix-ui/react-dialog";
import * as RadixTooltip from "@radix-ui/react-tooltip";
import { ArrowRight, GitPullRequest, Info, Pencil } from "lucide-react";
import { useForm } from "react-hook-form";
import { setTitle, type TaskDetail } from "@/api/tasks";
import { Chip } from "@/components/badge/Chip";
import { Button, IconButton } from "@/components/button/Button";
import { TextField } from "@/components/form/controls";
import { Mono } from "@/components/mono/Mono";
import { useState } from "react";
import { cn } from "@/lib/cn";
import { PersonName } from "@/people/PersonName";
import type { LiveStrip } from "./liveStrip";
import { LinkButton, submit, WriteError } from "./parts";
import { proposalPath } from "./links";
import { proposalStrip } from "./proposalStrip";
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

/** The details of the live strip: a tooltip on hover and focus, which a tap opens too, as a phone has no hover. */
function StripDetails({ details }: { details: string }) {
  const [open, setOpen] = useState(false);
  return (
    <RadixTooltip.Provider delayDuration={300}>
      <RadixTooltip.Root open={open} onOpenChange={setOpen}>
        <RadixTooltip.Trigger asChild>
          <IconButton
            aria-label="Details"
            className="shrink-0"
            onClick={(event) => {
              // Radix closes a tooltip on a click; a tap opens and closes it instead.
              event.preventDefault();
              setOpen(!open);
            }}
          >
            <Info aria-hidden="true" size={16} />
          </IconButton>
        </RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content
            side="bottom"
            align="end"
            sideOffset={6}
            className="z-tooltip max-w-measure rounded-s bg-ink px-10 py-6 text-13 text-on-ink"
          >
            {details}
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}

/** One fact, who does what now; the files, the checkpoint and the base wait behind the info button. */
function Strip({ strip }: { strip: LiveStrip }) {
  return (
    <div
      aria-label="Live"
      className={cn(
        "flex min-w-0 items-center gap-10 rounded-m py-4 pr-4 pl-14 text-15",
        strip.stalled ? "bg-warn-soft" : "bg-accent-soft",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-8 shrink-0 rounded-full",
          strip.stalled ? "bg-warn" : "bg-accent",
        )}
      />
      <p className="min-w-0 flex-1 py-5 wrap-anywhere">
        <PersonName name={strip.worker} className="font-semibold" />
        {" · "}
        <span>{strip.verb}</span>
        {strip.file ? (
          <>
            {" "}
            <Mono className="text-14">{strip.file}</Mono>
          </>
        ) : null}
        {strip.wait ? ` · ${strip.wait}` : null}
      </p>
      {strip.details === null ? null : <StripDetails details={strip.details} />}
    </div>
  );
}

function ProposalStrip({ slug, task }: { slug: string; task: TaskDetail }) {
  const strip = proposalStrip(task);
  if (strip === null) {
    return null;
  }
  return (
    <div
      aria-label="Proposal"
      className={cn(
        "flex min-w-0 flex-wrap items-center gap-12 rounded-m py-10 pr-10 pl-14",
        strip.waitsForYou ? "bg-warn-soft" : "bg-bg",
      )}
    >
      <GitPullRequest
        aria-hidden="true"
        size={16}
        className={cn(
          "shrink-0",
          strip.waitsForYou ? "text-warn" : "text-text-2",
        )}
      />
      <p className="flex min-w-0 flex-1 flex-col">
        <span
          className={cn(
            "font-semibold",
            strip.waitsForYou ? "text-warn" : "text-text",
          )}
        >
          {strip.lead}
        </span>
        {strip.rest ? (
          <span className="text-13 text-text-2">{strip.rest}</span>
        ) : null}
      </p>
      <LinkButton
        href={proposalPath(slug, strip.proposal)}
        variant={strip.waitsForYou ? "primary" : "quiet"}
        icon={ArrowRight}
      >
        Go to proposal
      </LinkButton>
    </div>
  );
}

/** The title block: the title, the calculated status with its reason, the live strip and the proposal that waits. */
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
          <RadixDialog.Title className="min-w-0 flex-1 text-28 leading-tight font-semibold wrap-anywhere text-text">
            {task.title}
          </RadixDialog.Title>
          <IconButton
            aria-label="Edit the title"
            onClick={() => {
              setEditing(true);
            }}
          >
            <Pencil aria-hidden="true" size={16} />
          </IconButton>
        </div>
      )}
      <div className="flex flex-wrap items-center gap-x-12 gap-y-6">
        <Chip>{look.text}</Chip>
        {task.blocked ? (
          <span className="text-14 font-medium text-bad">Blocked</span>
        ) : null}
        {/* An empty reason draws nothing, so the line is left out for a task read without one. */}
        {task.status_reason ? (
          <RadixDialog.Description className="text-14 text-text-2">
            {task.status_reason}
          </RadixDialog.Description>
        ) : null}
      </div>
      {strip ? <Strip strip={strip} /> : null}
      <ProposalStrip slug={slug} task={task} />
    </div>
  );
}
