import { zodResolver } from "@hookform/resolvers/zod";
import { Check, GitMerge, RotateCcw, Undo2 } from "lucide-react";
import { useId, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import type { ProposalDetail } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { showsApproveLine, type HumanAction } from "./actions";
import { requestChangesSchema, type RequestChangesValues } from "./compose";
import { keptApprovalsNote } from "./edit";
import { detailHref, type DetailView } from "./view";

export interface ActionHandlers {
  visible: readonly HumanAction[];
  pending: HumanAction | null;
  /** The sentence for the last failed write the screen shows on the actions. */
  failure: string | null;
  /** Why Integrate cannot work now, from the daemon's gate report; null when it can. */
  integrateBlock: string | null;
  /** Why Approve cannot work now; null when it can. */
  approveBlock: string | null;
  onApprove: () => void;
  onIntegrate: () => void;
  /** Sends the current version, a human edit, for review again. */
  onReviewAgain: () => void;
  /** Resolves once the daemon has taken the verdict; rejects with its error. */
  onRequestChanges: (body: string) => Promise<void>;
}

function RequestChangesForm({
  slug,
  onSubmit,
  onCancel,
}: {
  slug: string;
  onSubmit: (body: string) => Promise<void>;
  onCancel: () => void;
}) {
  const {
    control,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<RequestChangesValues>({
    resolver: zodResolver(requestChangesSchema),
    defaultValues: { body: "" },
  });
  const submit = handleSubmit(async ({ body }) => {
    try {
      await onSubmit(body.trim());
    } catch {
      // The screen shows the failure under the actions.
    }
  });
  return (
    <form
      aria-label="Request changes"
      noValidate
      onSubmit={(event) => void submit(event)}
      className="flex w-full flex-col gap-8"
    >
      <Controller
        control={control}
        name="body"
        render={({ field }) => (
          <MentionTextarea
            slug={slug}
            rows={3}
            aria-label="What needs to change"
            autoFocus
            readOnly={isSubmitting}
            aria-invalid={errors.body ? true : undefined}
            className="resize-y py-9"
            ref={field.ref}
            value={field.value}
            onValueChange={field.onChange}
            onBlur={field.onBlur}
          />
        )}
      />
      {errors.body ? (
        <p className="text-13 font-medium text-bad">{errors.body.message}</p>
      ) : null}
      <div className="flex flex-wrap gap-8">
        <Button
          type="submit"
          variant="danger"
          pending={isSubmitting}
          pendingLabel="Sending request…"
          icon={<Undo2 aria-hidden="true" size={15} />}
        >
          Send request
        </Button>
        <Button variant="ghost" disabled={isSubmitting} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

/**
 * Approve, Request changes, Integrate and Send for review again on the current version of an open proposal. The Overview's integration card draws them as a block; the Changes toolbar draws them in one row that wraps.
 */
export function HumanActions({
  proposal,
  human,
  view,
  slug,
  actions,
  compact = false,
}: {
  proposal: ProposalDetail;
  human: string;
  view: DetailView;
  slug: string;
  actions: ActionHandlers;
  compact?: boolean;
}) {
  const [requesting, setRequesting] = useState(false);
  const blockId = useId();
  const approveBlockId = useId();
  const v = `v${String(proposal.summary.current_version)}`;
  const busy = actions.pending !== null;
  const size = compact ? "sm" : "md";
  if (!view.current) {
    return (
      <div
        className={cn(
          "flex flex-wrap items-center gap-x-10 gap-y-4",
          !compact && "border-t border-line pt-12",
        )}
      >
        <p className="text-13 leading-tight text-text-2">
          v{view.version} is an earlier version. A version never changes, so it
          takes no verdict.
        </p>
        <HrefLink
          href={detailHref(slug, proposal.summary.id, { tab: "changes" })}
          className={cn(
            "text-13 font-semibold text-accent hover:underline",
            focusRing,
          )}
        >
          Open {v} to review
        </HrefLink>
      </div>
    );
  }
  const shows = (action: HumanAction) => actions.visible.includes(action);
  const kept = keptApprovalsNote(proposal, proposal.summary.current_version);
  const approveLine =
    shows("approve") &&
    showsApproveLine(
      actions.approveBlock,
      actions.integrateBlock,
      shows("integrate"),
    );
  const notes = (
    <>
      {approveLine ? (
        <p id={approveBlockId} className="text-13 text-text-2">
          {actions.approveBlock}
        </p>
      ) : null}
      {shows("integrate") && actions.integrateBlock !== null ? (
        <p id={blockId} className="text-13 text-text-2">
          {actions.integrateBlock}
        </p>
      ) : null}
      {kept !== null ? <p className="text-13 text-text-2">{kept}</p> : null}
    </>
  );
  return (
    <div
      role="group"
      aria-label="Your review"
      className={cn(
        "flex flex-col gap-8",
        !compact && "border-t border-line pt-12",
      )}
    >
      {compact ? null : (
        <p className="text-13 text-text-2">You are reviewing as {human}</p>
      )}
      {requesting ? (
        <RequestChangesForm
          slug={slug}
          onSubmit={async (body) => {
            await actions.onRequestChanges(body);
            setRequesting(false);
          }}
          onCancel={() => {
            setRequesting(false);
          }}
        />
      ) : (
        <div className={cn("flex flex-wrap gap-8", compact && "justify-end")}>
          {shows("approve") ? (
            <Button
              variant="primary"
              size={size}
              pending={actions.pending === "approve"}
              pendingLabel={`Approving ${v}…`}
              disabled={busy || actions.approveBlock !== null}
              aria-describedby={
                actions.approveBlock === null
                  ? undefined
                  : approveLine
                    ? approveBlockId
                    : blockId
              }
              icon={<Check aria-hidden="true" size={15} />}
              onClick={actions.onApprove}
            >
              Approve {v}
            </Button>
          ) : null}
          {shows("request_changes") ? (
            <Button
              size={size}
              disabled={busy}
              icon={<Undo2 aria-hidden="true" size={15} />}
              onClick={() => {
                setRequesting(true);
              }}
            >
              Request changes
            </Button>
          ) : null}
          {shows("integrate") ? (
            <Button
              size={size}
              pending={actions.pending === "integrate"}
              pendingLabel="Integrating…"
              disabled={busy || actions.integrateBlock !== null}
              aria-describedby={
                actions.integrateBlock === null ? undefined : blockId
              }
              icon={<GitMerge aria-hidden="true" size={15} />}
              onClick={actions.onIntegrate}
            >
              Integrate
            </Button>
          ) : null}
          {shows("review_again") ? (
            <Button
              size={size}
              pending={actions.pending === "review_again"}
              pendingLabel="Sending for review…"
              disabled={busy}
              icon={<RotateCcw aria-hidden="true" size={15} />}
              onClick={actions.onReviewAgain}
            >
              Send {v} for review again
            </Button>
          ) : null}
        </div>
      )}
      {requesting ? null : notes}
      {actions.failure ? (
        <p role="alert" className="text-13 font-medium text-bad">
          {actions.failure}
        </p>
      ) : null}
    </div>
  );
}
