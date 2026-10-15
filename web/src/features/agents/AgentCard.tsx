import { useQueryClient } from "@tanstack/react-query";
import { ShieldCheck, ShieldOff, ShieldPlus } from "lucide-react";
import { useState } from "react";
import { HrefLink } from "@/app/HrefLink";
import { setIntegrator } from "@/api/agents";
import type { AgentView } from "@/api/agents";
import { keys } from "@/api/keys";
import { Avatar, PresenceDot } from "@/components/avatar/Avatar";
import { Badge } from "@/components/badge/Badge";
import { Button } from "@/components/button/Button";
import { Label } from "@/components/label/Label";
import { formatSince } from "@/format/time";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { IntegratorDialog } from "./IntegratorDialog";
import {
  activityText,
  claimBadges,
  claimTarget,
  claimText,
  footLine,
  hasIntegrator,
  orderWorkers,
  plainTags,
  presenceLine,
  sessionLine,
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
  const live = agent.sessions;

  return (
    <section
      aria-label={`Agent ${agent.name}`}
      className="flex flex-col overflow-hidden rounded-l border border-border-subtle bg-surface"
    >
      <header className="flex flex-col gap-10 p-16">
        <div className="flex items-center gap-10">
          <Avatar name={agent.name} className="size-32 text-caption" />
          <div className="flex min-w-0 flex-col gap-2">
            <h2 className="text-item-title leading-snug font-semibold text-text">
              {agent.name}
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
          <p key={session.id} className="font-mono text-label text-text-muted">
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
              const claim = claimText(worker, agent.claims);
              const badges = workerBadges(worker, agent.attention);
              return (
                <li
                  key={worker.label}
                  className={cn(
                    "flex flex-col gap-4",
                    depth > 0 && "border-l border-border-subtle pl-14",
                  )}
                >
                  <div className="flex items-center gap-8">
                    <span className="min-w-0 flex-1 font-mono text-code leading-snug text-text">
                      {worker.label}
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

      {agent.claims.length > 0 ? (
        <div className="flex flex-col gap-8 border-t border-border-subtle px-16 py-12">
          <h3 className={OVERLINE}>Claims</h3>
          <ul className="flex flex-col gap-6">
            {agent.claims.map((claim) => (
              <li
                key={`${claim.kind}:${claim.task}`}
                className="flex items-center gap-8 text-code"
              >
                <IdLink {...claimTarget(slug, claim)} />
                <span className="text-text-muted">
                  {claim.kind} · {claim.worker} · lease {claim.lease}
                </span>
                {claimBadges(claim, agent.attention).map((tone) => (
                  <Badge key={tone} tone={tone} />
                ))}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {agent.workspaces.length > 0 ? (
        <div className="flex flex-col gap-8 border-t border-border-subtle px-16 py-12">
          <h3 className={OVERLINE}>Workspaces</h3>
          <ul className="flex flex-col gap-6">
            {agent.workspaces.map((ws) => (
              <li
                key={ws.workspace}
                className="flex items-center gap-8 text-code"
              >
                <span className="font-mono text-label text-text">
                  {ws.workspace}
                </span>
                <IdLink href={`/p/${slug}/t/${ws.task}`} id={ws.task} />
                <span className="text-text-muted">
                  {ws.worker} · {ws.summary} ·{" "}
                  {formatSince(now, ws.last_activity_at)}
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
                    {event.actor} · {formatSince(now, event.created_at)}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}

      <footer className="flex items-center gap-8 border-t border-border-subtle bg-bg px-16 py-10">
        <p className="min-w-0 flex-1 text-code text-text-muted">
          {footLine(agent)}
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
          await setIntegrator(agent.name, !granted);
          await queryClient.invalidateQueries({ queryKey: keys.agents(slug) });
        }}
      />
    </section>
  );
}
