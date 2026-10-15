import { useQueryClient } from "@tanstack/react-query";
import { ShieldCheck, ShieldOff, ShieldPlus } from "lucide-react";
import { useState } from "react";
import { HrefLink } from "@/app/HrefLink";
import { setIntegrator } from "@/api/agents";
import type { AgentView } from "@/api/agents";
import { keys } from "@/api/keys";
import { PresenceDot } from "@/components/avatar/PresenceDot";
import { Badge } from "@/components/badge/Badge";
import { Button } from "@/components/button/Button";
import { Label } from "@/components/label/Label";
import { formatSince } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { ActorText } from "@/people/ActorText";
import { PersonName } from "@/people/PersonName";
import { IntegratorDialog } from "./IntegratorDialog";
import {
  activityText,
  agentClaims,
  agentWorkspaces,
  changedFilesText,
  claimBadges,
  claimTarget,
  claimText,
  footLine,
  hasIntegrator,
  orderWorkers,
  plainTags,
  presenceLine,
  sessionKey,
  sessionLine,
  sessionLines,
  workerBadges,
} from "./presence";

interface AgentCardProps {
  slug: string;
  agent: AgentView;
  /** The query's last update time; times in the card are relative to it. */
  now: number;
}

const OVERLINE =
  "text-overline leading-snug font-semibold tracking-overline text-primary-text uppercase";

const ID_LINK = cn(
  "rounded-s font-mono text-code text-primary-text underline-offset-2 hover:underline",
  focusRing,
);

function IdLink({ href, id }: { href: string; id: string }) {
  return (
    <HrefLink href={href} className={ID_LINK}>
      {id}
    </HrefLink>
  );
}

export function AgentCard({ slug, agent, now }: AgentCardProps) {
  const queryClient = useQueryClient();
  const [dialog, setDialog] = useState(false);
  const granted = hasIntegrator(agent);
  const workers = orderWorkers(agent.workers);
  const tags = plainTags(agent);
  const claims = agentClaims(agent);
  const workspaces = agentWorkspaces(agent);
  const live = sessionLines(agent);

  return (
    <section
      aria-label={`Agent ${agent.name}`}
      className="flex flex-col overflow-hidden rounded-l border border-border-subtle bg-surface"
    >
      <header className="flex flex-col gap-10 p-16">
        <div className="flex items-center gap-10">
          <div className="flex min-w-0 flex-col gap-2">
            <h2 className="text-item-title leading-snug">
              <PersonName name={agent.name} />
            </h2>
            <p className="flex items-center gap-6 text-code text-text-muted">
              <PresenceDot state={agent.presence} className="size-7" />
              {presenceLine(agent, now)}
            </p>
          </div>
        </div>
        {tags.length > 0 || granted ? (
          <ul className="flex flex-wrap items-center gap-6" aria-label="Tags">
            {tags.map((tag) => (
              <li key={tag}>
                <Label color="stone">{tag}</Label>
              </li>
            ))}
            {granted ? (
              <li className="inline-flex items-center gap-5 rounded-s bg-ink px-8 py-2 text-label leading-snug font-semibold text-on-ink">
                <ShieldCheck aria-hidden="true" size={12} />
                integrator
              </li>
            ) : null}
          </ul>
        ) : null}
        {live.map((session) => (
          <p
            key={sessionKey(session)}
            className="font-mono text-label text-text-muted"
          >
            {sessionLine(session, now)}
          </p>
        ))}
      </header>

      <div className="flex flex-col gap-12 border-t border-border-subtle px-16 py-12">
        <h3 className={OVERLINE}>Workers</h3>
        {workers.length === 0 ? (
          <p className="text-code text-text-muted">No worker has registered.</p>
        ) : (
          <ul className="flex flex-col gap-12">
            {workers.map(({ worker, depth }) => {
              const claim = claimText(worker);
              const badges = workerBadges(worker, agent.attention);
              return (
                <li
                  key={worker.name}
                  className={cn(
                    "flex flex-col gap-4",
                    depth > 0 && "border-l border-border-subtle pl-14",
                  )}
                >
                  <div className="flex items-center gap-8">
                    <span className="min-w-0 flex-1 text-code leading-snug">
                      <PersonName name={worker.name} />
                    </span>
                    {badges.map((tone) => (
                      <Badge key={tone} tone={tone} />
                    ))}
                    {claim ? (
                      <span className="font-mono text-label text-text-muted">
                        {claim}
                      </span>
                    ) : null}
                  </div>
                  <p
                    className={cn(
                      "text-code",
                      badges.length > 0
                        ? "text-secondary-text"
                        : "text-text-muted",
                    )}
                  >
                    {activityText(worker, now)}
                  </p>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {claims.length > 0 ? (
        <div className="flex flex-col gap-8 border-t border-border-subtle px-16 py-12">
          <h3 className={OVERLINE}>Claims</h3>
          <ul className="flex flex-col gap-6">
            {claims.map(({ worker, claim }) => (
              <li
                key={`${worker}:${claim.kind}:${claim.task}`}
                className="flex items-center gap-8 text-code"
              >
                <IdLink {...claimTarget(slug, claim)} />
                <span className="text-text-muted">
                  {claim.kind} · <PersonName name={worker} />
                </span>
                {claimBadges(claim, agent.attention).map((tone) => (
                  <Badge key={tone} tone={tone} />
                ))}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {workspaces.length > 0 ? (
        <div className="flex flex-col gap-8 border-t border-border-subtle px-16 py-12">
          <h3 className={OVERLINE}>Workspaces</h3>
          <ul className="flex flex-col gap-6">
            {workspaces.map(({ worker, workspace }) => (
              <li
                key={workspace.ws}
                className="flex items-center gap-8 text-code"
              >
                <span className="font-mono text-label text-text">
                  {workspace.ws}
                </span>
                <IdLink
                  href={`/p/${slug}/t/${workspace.task}`}
                  id={workspace.task}
                />
                <span className="text-text-muted">
                  <PersonName name={worker} /> · {changedFilesText(workspace)}
                  {workspace.last_activity_at === null
                    ? ""
                    : ` · ${formatSince(now, workspace.last_activity_at)}`}
                </span>
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {agent.events.length > 0 ? (
        <div className="flex flex-col gap-8 border-t border-border-subtle px-16 py-12">
          <h3 className={OVERLINE}>Recent events</h3>
          <ul className="flex flex-col gap-6">
            {agent.events.map((event) => {
              const target = event.proposal ?? event.task;
              return (
                <li
                  key={event.seq}
                  className="flex items-center gap-8 text-code text-text-muted"
                >
                  <span className="font-mono text-label text-text">
                    {event.type}
                  </span>
                  {target ? (
                    <IdLink
                      id={target}
                      href={
                        event.proposal
                          ? `/p/${slug}/proposals/${event.proposal}`
                          : `/p/${slug}/t/${target}`
                      }
                    />
                  ) : null}
                  <span>
                    <PersonName name={event.actor} /> ·{" "}
                    {formatSince(now, event.created_at)}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}

      <footer className="flex items-center gap-8 border-t border-border-subtle bg-bg px-16 py-10">
        <p className="min-w-0 flex-1 text-code text-text-muted">
          <ActorText segments={footLine(agent)} slot="foot" />
        </p>
        <Button
          size="sm"
          variant="ghost"
          className="px-6 text-meta"
          icon={
            granted ? (
              <ShieldOff
                aria-hidden="true"
                size={13}
                className="text-text-faint"
              />
            ) : (
              <ShieldPlus
                aria-hidden="true"
                size={13}
                className="text-text-faint"
              />
            )
          }
          onClick={() => {
            setDialog(true);
          }}
        >
          {granted ? "Remove integrator" : "Grant integrator"}
        </Button>
      </footer>
      <IntegratorDialog
        agent={agent.name}
        grant={!granted}
        open={dialog}
        onOpenChange={setDialog}
        onConfirm={async () => {
          await setIntegrator(slug, agent.name, !granted);
          await queryClient.invalidateQueries({ queryKey: keys.agents(slug) });
        }}
      />
    </section>
  );
}
