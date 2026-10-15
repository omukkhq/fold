import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import { zodResolver } from "@hookform/resolvers/zod";
import {
  Check,
  CircleAlert,
  CircleCheck,
  CircleDashed,
  CircleSlash,
  CircleX,
  GitMerge,
  Hourglass,
  MessageSquare,
  Undo2,
  type LucideIcon,
} from "lucide-react";
import { useId, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import type {
  ConflictReport,
  GateStatus,
  ProposalDetail,
  ProposalVersion,
  Verdict,
} from "@/api/proposals";
import { HrefLink } from "@/app/HrefLink";
import { Button } from "@/components/button/Button";
import { MentionInline, MentionText } from "@/mentions/MentionText";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import { formatAsOfMinute } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import type { HumanAction } from "./actions";
import { requestChangesSchema, type RequestChangesValues } from "./compose";
import {
  conflictKindText,
  fileSides,
  hunkSides,
  nextStepText,
  reportTrees,
} from "./conflict";
import { gateText, shownGates, waitingSummary } from "./gates";
import type { IntegrationView } from "./integration";
import {
  evidenceLines,
  listedReviews,
  reviewText,
  versionRef,
} from "./present";
import type { DetailView } from "./view";

const OVERLINE =
  "text-overline leading-snug font-semibold text-primary-text uppercase";

const GATE_ICONS: Record<
  GateStatus,
  { icon: LucideIcon; label: string; className: string }
> = {
  met: { icon: CircleCheck, label: "met", className: "text-primary-text" },
  unmet: { icon: CircleX, label: "not met", className: "text-danger" },
  checked_at_integration: {
    icon: CircleDashed,
    label: "checked at integration",
    className: "text-text-muted",
  },
};

const VERDICT_ICONS: Record<Verdict, { icon: LucideIcon; className: string }> =
  {
    approve: { icon: CircleCheck, className: "text-primary-text" },
    request_changes: { icon: CircleAlert, className: "text-danger" },
    reject: { icon: CircleSlash, className: "text-danger" },
    comment: { icon: MessageSquare, className: "text-text-muted" },
  };

export interface ActionHandlers {
  visible: readonly HumanAction[];
  pending: HumanAction | null;
  /** The sentence for the last failed write the screen shows on the actions. */
  failure: string | null;
  /** Why Integrate cannot work now, from the daemon's gate report; null when it can. */
  integrateBlock: string | null;
  onApprove: () => void;
  onIntegrate: () => void;
  /** Resolves once the daemon has taken the verdict; rejects with its error. */
  onRequestChanges: (body: string) => Promise<void>;
}

function ConflictBlock({
  report,
  proposal,
}: {
  report: ConflictReport;
  proposal: ProposalDetail;
}) {
  const ref = versionRef(proposal.summary.id, report.version);
  return (
    <section
      aria-label="Conflict report"
      className="flex flex-col gap-10 rounded-m bg-danger-soft px-12 py-10"
    >
      <h3 className="text-bar leading-snug font-semibold text-danger">
        Conflict report
      </h3>
      <dl className="flex flex-col gap-2 font-mono text-meta text-text-muted">
        {reportTrees(report, ref).map((side) => (
          <div key={side.label} className="flex gap-6">
            <dt className="shrink-0">{side.label}</dt>
            <dd className="min-w-0 truncate">{side.entry}</dd>
          </div>
        ))}
      </dl>
      <ul className="flex flex-col gap-12">
        {report.files.map((file) => (
          <li key={file.path} className="flex flex-col gap-4">
            <p className="font-mono text-code font-semibold break-all text-text">
              {file.path}
            </p>
            <p
              data-kind={file.kind}
              className="text-code leading-snug text-text"
            >
              {conflictKindText(file.kind)}
            </p>
            <dl className="flex flex-col gap-2 font-mono text-caption text-text-muted">
              {fileSides(file, ref, report).map((side) => (
                <div key={side.label} className="flex gap-6">
                  <dt className="shrink-0">{side.label}</dt>
                  <dd className="min-w-0 truncate">{side.entry}</dd>
                </div>
              ))}
            </dl>
            {file.hunks.map((hunk, index) => (
              <div
                key={index}
                className="flex flex-col gap-4 overflow-x-auto rounded-s bg-surface px-8 py-6 font-mono text-caption"
              >
                {hunkSides(hunk, ref).map(({ label, lines }) => (
                  <div key={label} className="flex flex-col">
                    <span className="text-text-faint">{label}</span>
                    {lines.map((text, n) => (
                      <code key={n} className="whitespace-pre text-text">
                        {text}
                      </code>
                    ))}
                  </div>
                ))}
              </div>
            ))}
          </li>
        ))}
      </ul>
      <p className="text-code leading-snug text-text-muted">
        <ActorText
          segments={nextStepText(proposal.summary.author, report.version)}
        />
      </p>
    </section>
  );
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
      className="flex flex-col gap-8"
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
        <p className="text-meta font-medium text-danger">
          {errors.body.message}
        </p>
      ) : null}
      <div className="flex gap-8">
        <Button
          type="submit"
          variant="danger"
          pending={isSubmitting}
          icon={<Undo2 aria-hidden="true" size={15} />}
          className="flex-1 justify-center"
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

function HumanActions({
  proposal,
  human,
  view,
  slug,
  actions,
}: {
  proposal: ProposalDetail;
  human: string;
  view: DetailView;
  slug: string;
  actions: ActionHandlers;
}) {
  const [requesting, setRequesting] = useState(false);
  const blockId = useId();
  const v = `v${String(proposal.summary.current_version)}`;
  const busy = actions.pending !== null;
  if (view.kind === "version" && !view.current) {
    return (
      <div className="flex flex-col gap-8 border-t border-border-subtle pt-10">
        <p className="text-code leading-snug text-text-muted">
          v{view.version} is an earlier version. A version never changes, so it
          takes no verdict.
        </p>
        <HrefLink
          href={`/p/${slug}/proposals/${proposal.summary.id}`}
          className={cn(
            "text-code font-semibold text-primary-text hover:underline",
            focusRing,
          )}
        >
          Open {v} to review
        </HrefLink>
      </div>
    );
  }
  const shows = (action: HumanAction) => actions.visible.includes(action);
  return (
    <div className="flex flex-col gap-8 border-t border-border-subtle pt-10">
      <p className="text-code text-text-muted">You are reviewing as {human}</p>
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
        <>
          {shows("approve") ? (
            <Button
              variant="primary"
              pending={actions.pending === "approve"}
              disabled={busy}
              icon={<Check aria-hidden="true" size={15} />}
              className="w-full justify-center"
              onClick={actions.onApprove}
            >
              Approve {v}
            </Button>
          ) : null}
          <div className="flex gap-8">
            {shows("request_changes") ? (
              <Button
                disabled={busy}
                icon={<Undo2 aria-hidden="true" size={15} />}
                className="flex-1 justify-center"
                onClick={() => {
                  setRequesting(true);
                }}
              >
                Request changes
              </Button>
            ) : null}
            {shows("integrate") ? (
              <Button
                pending={actions.pending === "integrate"}
                disabled={busy || actions.integrateBlock !== null}
                aria-describedby={
                  actions.integrateBlock === null ? undefined : blockId
                }
                icon={<GitMerge aria-hidden="true" size={15} />}
                className="flex-1 justify-center"
                onClick={actions.onIntegrate}
              >
                Integrate
              </Button>
            ) : null}
          </div>
          {shows("integrate") && actions.integrateBlock !== null ? (
            <p id={blockId} className="text-meta text-text-muted">
              {actions.integrateBlock}
            </p>
          ) : null}
        </>
      )}
      {actions.failure ? (
        <p role="alert" className="text-meta font-medium text-danger">
          {actions.failure}
        </p>
      ) : null}
    </div>
  );
}

function VersionFacts({
  version,
  now,
}: {
  version: ProposalVersion;
  now: number;
}) {
  const rows: [string, string][] = [
    ["base", version.base],
    ["tree", version.tree.slice(0, 12)],
    ["changeset", version.changeset],
    ["sent", formatAsOfMinute(now, version.created_at)],
  ];
  return (
    <section
      aria-label={`Version v${String(version.version)}`}
      className="flex flex-col gap-8"
    >
      <h2 className={OVERLINE}>Version v{version.version}</h2>
      <dl className="grid grid-cols-[auto_1fr] gap-x-10 gap-y-2 font-mono text-meta">
        {rows.map(([label, value]) => (
          <div key={label} className="contents">
            <dt className="text-text-muted">{label}</dt>
            <dd className="min-w-0 truncate text-text">{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

export function SidePanel({
  slug,
  proposal,
  view,
  human,
  now,
  main,
  integration,
  actions,
}: {
  slug: string;
  proposal: ProposalDetail;
  view: DetailView;
  human: string | null;
  now: number;
  /** The head of main; null until the revisions arrive. */
  main: string | null;
  /** Null until a revision of main names the proposal. */
  integration: IntegrationView | null;
  actions: ActionHandlers;
}) {
  const summary = waitingSummary(proposal);
  const reviews = listedReviews(proposal.reviews);
  const shown = proposal.versions.find((v) => v.version === view.version);
  const clock = (at: number) => formatAsOfMinute(now, at);
  return (
    <aside className="flex w-review-panel shrink-0 flex-col gap-18">
      <section
        aria-label="Integration"
        className="flex flex-col gap-10 rounded-l bg-surface p-16 inset-ring inset-ring-border-subtle"
      >
        <h2 className="text-section leading-snug font-semibold text-text">
          Integration
        </h2>
        <ul className="flex flex-col gap-10">
          {shownGates(proposal.gates).map((gate) => {
            const spec = GATE_ICONS[gate.status];
            const Icon = spec.icon;
            return (
              <li
                key={gate.gate}
                data-state={gate.status}
                className="flex gap-8"
              >
                <Icon
                  role="img"
                  aria-label={spec.label}
                  size={15}
                  className={cn("mt-2 shrink-0", spec.className)}
                />
                <span className="min-w-0 flex-1 text-bar leading-snug text-text">
                  <ActorText
                    segments={gateText(gate, proposal, main)}
                    personClassName="text-body"
                  />
                </span>
              </li>
            );
          })}
        </ul>
        {summary ? (
          <p className="flex items-center gap-8 rounded-m bg-sunken px-10 py-8 text-code leading-snug text-text-muted">
            <Hourglass aria-hidden="true" size={14} className="shrink-0" />
            <ActorText segments={summary} personClassName="text-bar" />
          </p>
        ) : null}
        {integration ? (
          <div className="flex flex-col gap-4 rounded-m bg-primary-soft px-12 py-10">
            <p className="flex items-center gap-8 text-bar font-semibold text-primary-text">
              <GitMerge aria-hidden="true" size={14} />
              Integrated as{" "}
              <HrefLink
                href={`/p/${slug}/repository?revision=${integration.revision}`}
                className={cn("font-mono underline", focusRing)}
              >
                {integration.revision}
              </HrefLink>
            </p>
            <p className="font-mono text-caption text-text-muted">
              {integration.actor ? (
                <>
                  <PersonName name={integration.actor} /> ·{" "}
                </>
              ) : null}
              v{integration.version} · {clock(integration.at)}
            </p>
          </div>
        ) : null}
        {proposal.conflict ? (
          <ConflictBlock report={proposal.conflict} proposal={proposal} />
        ) : null}
        {human !== null && proposal.summary.state === "open" ? (
          <HumanActions
            proposal={proposal}
            human={human}
            view={view}
            slug={slug}
            actions={actions}
          />
        ) : null}
      </section>

      <section aria-label="Reviews" className="flex flex-col gap-10">
        <h2 className={OVERLINE}>Reviews</h2>
        {reviews.length === 0 ? (
          <p className="text-bar text-text-muted">No review yet.</p>
        ) : (
          <ul className="flex flex-col gap-10">
            {reviews.map((review, index) => {
              const spec = VERDICT_ICONS[review.verdict];
              const Icon = spec.icon;
              const earlier =
                review.verdict === "approve" &&
                review.version !== proposal.summary.current_version;
              return (
                <li
                  key={index}
                  data-verdict={review.verdict}
                  className="flex gap-8"
                >
                  <Icon
                    aria-hidden="true"
                    size={15}
                    className={cn("mt-2 shrink-0", spec.className)}
                  />
                  <div className="flex min-w-0 flex-1 flex-col gap-2">
                    <MentionText
                      text={reviewText(review)}
                      slug={slug}
                      className="text-bar leading-snug"
                    />
                    <p className="font-mono text-caption text-text-muted">
                      <PersonName name={review.reviewer} /> · v{review.version}
                      {earlier ? " · earlier version" : ""} ·{" "}
                      {clock(review.created_at)}
                    </p>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {shown ? <VersionFacts version={shown} now={now} /> : null}

      <section
        aria-label="Evidence from the author"
        className="flex flex-col gap-8"
      >
        <h2 className={OVERLINE}>Evidence from the author</h2>
        {shown?.evidence ? (
          <pre className="flex flex-col gap-2 overflow-x-auto rounded-m bg-sunken px-12 py-10 font-mono text-label">
            {evidenceLines(shown.evidence).map((line, index) => (
              <span
                key={index}
                className={line.command ? "text-text" : "text-text-muted"}
              >
                <MentionInline text={line.text} slug={slug} />
              </span>
            ))}
          </pre>
        ) : (
          <p className="text-bar text-text-muted">
            The author attached no evidence to v{view.version}.
          </p>
        )}
      </section>
    </aside>
  );
}
