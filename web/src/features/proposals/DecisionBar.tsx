import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import * as RadixPopover from "@radix-ui/react-popover";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Check,
  Ellipsis,
  GitMerge,
  MessageCircleQuestion,
  RotateCcw,
  Send,
  Undo2,
} from "lucide-react";
import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Controller, useForm } from "react-hook-form";
import type { ProposalDetail } from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { Button, IconButton } from "@/components/button/Button";
import { menuContent, menuItem } from "@/components/overlay/menuStyles";
import { failureText } from "@/format/failure";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { MessageComposer } from "@/mentions/MessageComposer";
import type { HumanAction } from "./actions";
import {
  questionSchema,
  requestChangesSchema,
  type QuestionValues,
  type RequestChangesValues,
} from "./compose";
import {
  approveReason,
  barNotes,
  moreLabel,
  phoneMore,
  phonePendingText,
  reviewerLine,
} from "./decisions";
import { keptApprovalsNote } from "./edit";
import { NAME_NOTE } from "./lineQuestions";
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

type Panel = "request" | "ask";

const LINK = cn(
  "rounded-s text-13 font-semibold text-accent hover:underline",
  focusRing,
);

function RequestChangesForm({
  slug,
  version,
  onSubmit,
  onCancel,
}: {
  slug: string;
  version: number;
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
      // The decision bar shows the failure under its buttons.
    }
  });
  return (
    <form
      aria-label="Request changes"
      noValidate
      onSubmit={(event) => void submit(event)}
      className="flex flex-col gap-10"
    >
      <h2 className="text-16 font-semibold text-text">
        Request changes on v{version}
      </h2>
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
      <div className="flex flex-wrap items-center justify-end gap-8">
        <Button variant="ghost" disabled={isSubmitting} onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="submit"
          variant="danger"
          pending={isSubmitting}
          pendingLabel="Sending request…"
          icon={<Undo2 aria-hidden="true" size={16} />}
        >
          Send request
        </Button>
      </div>
    </form>
  );
}

/** A question to the author about the whole proposal; the author decides on its own whether it needs a human's answer. */
function AskForm({
  slug,
  human,
  changesHref,
  onAsk,
  onDone,
}: {
  slug: string;
  human: string | null;
  changesHref: string;
  onAsk: (body: string) => Promise<void>;
  onDone: () => void;
}) {
  const {
    control,
    handleSubmit,
    setError,
    formState: { errors, isSubmitting },
  } = useForm<QuestionValues>({
    resolver: zodResolver(questionSchema),
    defaultValues: { body: "" },
  });
  const submit = handleSubmit(async ({ body }) => {
    try {
      await onAsk(body.trim());
      onDone();
    } catch (error) {
      setError("root", { message: failureText(error) });
    }
  });
  return (
    <form
      aria-label="Ask the author"
      noValidate
      onSubmit={(event) => void submit(event)}
      className="flex flex-col gap-10"
    >
      <h2 className="text-16 font-semibold text-text">
        Ask the author about the whole proposal
      </h2>
      {human === null ? (
        <p className="text-14 text-text-2">{NAME_NOTE}</p>
      ) : (
        <>
          <Controller
            control={control}
            name="body"
            render={({ field }) => (
              <MessageComposer
                slug={slug}
                aria-label="Question to the author"
                placeholder="What do you want to know? This changes nothing in the review."
                autoFocus
                pending={isSubmitting}
                aria-invalid={errors.body ? true : undefined}
                ref={field.ref}
                value={field.value}
                onValueChange={field.onChange}
                onBlur={field.onBlur}
              />
            )}
          />
          {errors.body ? (
            <p className="text-13 font-medium text-bad">
              {errors.body.message}
            </p>
          ) : null}
          {errors.root ? (
            <p role="alert" className="text-13 font-medium text-bad">
              The question was not sent. {errors.root.message}
            </p>
          ) : null}
          <div className="flex flex-wrap items-center justify-end gap-8">
            <span className="mr-auto text-13 text-text-2">
              To ask about some lines, select them in{" "}
              <HrefLink href={changesHref} className={LINK} onClick={onDone}>
                Changes
              </HrefLink>
              .
            </span>
            <Button variant="ghost" disabled={isSubmitting} onClick={onDone}>
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              pending={isSubmitting}
              pendingLabel="Asking…"
              icon={<Send aria-hidden="true" size={16} />}
            >
              Ask the author
            </Button>
          </div>
        </>
      )}
    </form>
  );
}

/** The heights the bar takes, in pixels. */
export interface BarLayout {
  /** The bar at the top of the page, tabs included. */
  bar: number;
  /** The decisions alone, which a phone draws at the bottom of the screen. */
  phoneBar: number;
}

export interface DecisionBarProps {
  slug: string;
  proposal: ProposalDetail;
  /** The signed-in human; null before the name prompt is answered. */
  human: string | null;
  /** The version the buttons act on: the one on screen on Changes, the current one elsewhere. */
  view: DetailView;
  actions: ActionHandlers;
  /** The proposal's tabs, which share the bar. */
  tabs: ReactNode;
  changesHref: string;
  /** Resolves once the question is stored; rejects with its error. */
  onAsk: (body: string) => Promise<void>;
  /** Hears the bar's height, which a side column held under it while the page scrolls stands below, and the height of the decisions, which a phone draws over the bottom of the screen, so the page leaves that much room under its end. */
  onLayout?: (layout: BarLayout) => void;
}

/**
 * The tabs with the human's decisions, the same on every tab and held at the top while the page scrolls. On a phone the decisions sit in a bar at the bottom of the screen, with Integrate and Ask the author under More.
 * A request for changes and a question to the author open in a panel under the bar.
 */
export function DecisionBar({
  slug,
  proposal,
  human,
  view,
  actions,
  tabs,
  changesHref,
  onAsk,
  onLayout,
}: DecisionBarProps) {
  const [panel, setPanel] = useState<Panel | null>(null);
  const bar = useRef<HTMLDivElement>(null);
  const group = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const outer = bar.current;
    const inner = group.current;
    if (
      outer === null ||
      inner === null ||
      onLayout === undefined ||
      typeof ResizeObserver === "undefined"
    ) {
      return;
    }
    const observer = new ResizeObserver(() => {
      onLayout({ bar: outer.offsetHeight, phoneBar: inner.offsetHeight });
    });
    observer.observe(outer);
    observer.observe(inner);
    return () => {
      observer.disconnect();
    };
  }, [onLayout]);
  const opener = useRef<HTMLElement | null>(null);
  const moreButton = useRef<HTMLButtonElement>(null);
  const approveId = useId();
  const integrateId = useId();
  const current = proposal.summary.current_version;
  const v = `v${String(current)}`;
  const decides = human !== null && proposal.summary.state === "open";
  const visible = decides && view.current ? actions.visible : [];
  const shows = (action: HumanAction) => visible.includes(action);
  const busy = actions.pending !== null;
  const notes = barNotes(
    visible,
    actions.approveBlock,
    actions.integrateBlock,
    visible.length > 0 ? keptApprovalsNote(proposal, current) : null,
  );
  const reason = approveReason(notes, actions.approveBlock);
  const more = phoneMore(visible);
  const noteId = (kind: string) =>
    kind === "approve"
      ? approveId
      : kind === "integrate"
        ? integrateId
        : undefined;

  const open = (next: Panel) => {
    opener.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setPanel(next);
  };
  const close = () => {
    setPanel(null);
  };

  return (
    <div
      ref={bar}
      className="sticky top-0 z-3 mx-16 mt-14 flex flex-wrap items-center gap-x-12 gap-y-6 rounded-m bg-surface p-6 md:mx-24"
    >
      {tabs}
      <RadixPopover.Root
        open={panel !== null}
        onOpenChange={(next) => {
          if (!next) close();
        }}
      >
        <RadixPopover.Anchor asChild>
          <div
            ref={group}
            role="group"
            aria-label="Your review"
            className="flex min-w-0 flex-col items-end gap-4 md:ml-auto max-md:fixed max-md:inset-x-0 max-md:bottom-0 max-md:z-3 max-md:items-stretch max-md:border-t max-md:border-line max-md:bg-surface max-md:px-12 max-md:pt-8 max-md:pb-16"
          >
            <div className="flex flex-wrap items-center justify-end gap-6 max-md:order-2 max-md:flex-nowrap">
              {decides && !view.current ? (
                <p className="mr-auto text-13 text-text-2">
                  v{view.version} is an earlier version. A version never
                  changes, so it takes no verdict.{" "}
                  <HrefLink
                    href={detailHref(slug, proposal.summary.id, {
                      tab: "changes",
                    })}
                    className={LINK}
                  >
                    Open {v} to review
                  </HrefLink>
                </p>
              ) : null}
              {visible.length > 0 ? (
                <span className="mr-6 text-13 text-text-2 max-md:hidden">
                  {reviewerLine(current, human ?? "")}
                </span>
              ) : null}
              {shows("approve") ? (
                <Button
                  variant="primary"
                  className="max-md:flex-1"
                  pending={actions.pending === "approve"}
                  pendingLabel={`Approving ${v}…`}
                  disabled={busy || actions.approveBlock !== null}
                  aria-describedby={
                    reason === null ? undefined : noteId(reason)
                  }
                  icon={<Check aria-hidden="true" size={16} />}
                  onClick={actions.onApprove}
                >
                  Approve {v}
                </Button>
              ) : null}
              {shows("request_changes") ? (
                <Button
                  className="max-md:flex-1"
                  disabled={busy}
                  aria-expanded={panel === "request"}
                  aria-haspopup="dialog"
                  icon={<Undo2 aria-hidden="true" size={16} />}
                  onClick={() => {
                    open("request");
                  }}
                >
                  Request changes
                </Button>
              ) : null}
              {shows("integrate") ? (
                <Button
                  className="max-md:hidden"
                  pending={actions.pending === "integrate"}
                  pendingLabel="Integrating…"
                  disabled={busy || actions.integrateBlock !== null}
                  aria-describedby={
                    actions.integrateBlock === null ? undefined : integrateId
                  }
                  icon={<GitMerge aria-hidden="true" size={16} />}
                  onClick={actions.onIntegrate}
                >
                  Integrate
                </Button>
              ) : null}
              {shows("review_again") ? (
                <Button
                  className="max-md:hidden"
                  pending={actions.pending === "review_again"}
                  pendingLabel="Sending for review…"
                  disabled={busy}
                  icon={<RotateCcw aria-hidden="true" size={16} />}
                  onClick={actions.onReviewAgain}
                >
                  Send {v} for review again
                </Button>
              ) : null}
              <Button
                className="max-md:hidden"
                aria-expanded={panel === "ask"}
                aria-haspopup="dialog"
                icon={<MessageCircleQuestion aria-hidden="true" size={16} />}
                onClick={() => {
                  open("ask");
                }}
              >
                Ask the author
              </Button>
              <DropdownMenu.Root
                onOpenChange={(next) => {
                  if (next) opener.current = null;
                }}
              >
                <DropdownMenu.Trigger asChild>
                  <IconButton
                    ref={moreButton}
                    variant="quiet"
                    className="md:hidden"
                    aria-label={moreLabel(more, current)}
                  >
                    <Ellipsis aria-hidden="true" size={16} />
                  </IconButton>
                </DropdownMenu.Trigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.Content
                    align="end"
                    sideOffset={6}
                    className={cn(menuContent, "min-w-max")}
                    onCloseAutoFocus={(event) => {
                      // A panel opened from the menu keeps the focus in its field.
                      if (opener.current === moreButton.current) {
                        event.preventDefault();
                      }
                    }}
                  >
                    {more.includes("integrate") ? (
                      <DropdownMenu.Item
                        className={cn(menuItem, "data-[disabled]:opacity-60")}
                        disabled={busy || actions.integrateBlock !== null}
                        onSelect={actions.onIntegrate}
                      >
                        <GitMerge aria-hidden="true" size={16} />
                        {actions.pending === "integrate"
                          ? "Integrating…"
                          : "Integrate"}
                      </DropdownMenu.Item>
                    ) : null}
                    {more.includes("review_again") ? (
                      <DropdownMenu.Item
                        className={cn(menuItem, "data-[disabled]:opacity-60")}
                        disabled={busy}
                        onSelect={actions.onReviewAgain}
                      >
                        <RotateCcw aria-hidden="true" size={16} />
                        Send {v} for review again
                      </DropdownMenu.Item>
                    ) : null}
                    <DropdownMenu.Item
                      className={menuItem}
                      onSelect={() => {
                        opener.current = moreButton.current;
                        setPanel("ask");
                      }}
                    >
                      <MessageCircleQuestion aria-hidden="true" size={16} />
                      Ask the author
                    </DropdownMenu.Item>
                  </DropdownMenu.Content>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
            </div>
            {notes.length > 0 ? (
              <div className="flex flex-wrap justify-end gap-x-8 px-8 pb-4 text-right text-13 text-text-2 max-md:order-1 max-md:justify-start max-md:px-2 max-md:pb-0 max-md:text-left">
                {notes.map((note) => (
                  <p key={note.kind} id={noteId(note.kind)}>
                    {note.text}
                  </p>
                ))}
              </div>
            ) : null}
            {phonePendingText(actions.pending, current) === null ? null : (
              <p
                role="status"
                className="px-2 text-13 font-medium text-text md:hidden max-md:order-1"
              >
                {phonePendingText(actions.pending, current)}
              </p>
            )}
            {actions.failure ? (
              <p
                role="alert"
                className="px-8 text-13 font-medium text-bad max-md:order-1 max-md:px-2"
              >
                {actions.failure}
              </p>
            ) : null}
          </div>
        </RadixPopover.Anchor>
        <RadixPopover.Portal>
          <RadixPopover.Content
            align="end"
            sideOffset={10}
            collisionPadding={16}
            onFocusOutside={(event) => {
              // The phone's More menu hands the focus back to its button as the panel opens.
              event.preventDefault();
            }}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              opener.current?.focus();
            }}
            className="z-10 flex w-history max-w-(--width-popover-max) flex-col rounded-m bg-surface p-16 shadow-pop outline-none"
          >
            {panel === "request" ? (
              <RequestChangesForm
                slug={slug}
                version={current}
                onSubmit={async (body) => {
                  await actions.onRequestChanges(body);
                  close();
                }}
                onCancel={close}
              />
            ) : panel === "ask" ? (
              <AskForm
                slug={slug}
                human={human}
                changesHref={changesHref}
                onAsk={onAsk}
                onDone={close}
              />
            ) : null}
          </RadixPopover.Content>
        </RadixPopover.Portal>
      </RadixPopover.Root>
    </div>
  );
}
