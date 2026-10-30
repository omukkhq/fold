import * as RadixDialog from "@radix-ui/react-dialog";
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, Menu, X } from "lucide-react";
import { useState } from "react";
import { agentBarQuery } from "../api/agentBar";
import { projectQuery, sessionQuery } from "../api/queries";
import { cn } from "../lib/cn";
import { focusRing, insetFocusRing } from "../lib/focus";
import { useLiveStatus } from "../live/status";
import { PersonName } from "../people/PersonName";
import { humanActor } from "../people/names";
import { navPath } from "../routes/nav";
import { projectPath } from "../routes/paths";
import { isPaused } from "./agentChipsModel";
import { agentsSummary, workingAgents } from "./agentsModel";
import { HrefLink } from "./HrefLink";
import { TabCount, useTabs } from "./NavTabs";
import { PauseControl } from "./PauseControl";
import { SignOut } from "./SignOut";
import { StreamStatus } from "./StreamStatus";
import { ThemeChoice } from "./ThemeChoice";
import { menuAlert, menuLabel } from "./topBarLayout";

const ROW =
  "flex min-h-44 items-center gap-12 px-16 text-15 text-text hover:bg-sunken";
const GROUP = "px-16 pt-14 pb-4 font-title text-13 font-semibold text-text-2";

/** The phone's menu below the bar: the tabs, the agents with Pause, the theme and Sign out, as plain rows on one sheet. */
export function PhoneMenu({
  slug,
  className,
}: {
  slug: string;
  className?: string;
}) {
  const tabs = useTabs(slug);
  const project = useQuery(projectQuery(slug));
  const agents = useQuery(agentBarQuery(slug));
  const session = useQuery(sessionQuery());
  const status = useLiveStatus();
  const [open, setOpen] = useState(false);
  const paused = isPaused(project.data?.overview?.paused_since);
  const alert = menuAlert(status.state, paused);
  const list = agents.data ?? [];
  const human = session.data?.human ?? null;
  const close = () => {
    setOpen(false);
  };
  return (
    <RadixDialog.Root open={open} onOpenChange={setOpen}>
      <RadixDialog.Trigger asChild>
        <button
          type="button"
          aria-label={menuLabel(alert)}
          className={cn(
            "relative grid size-44 shrink-0 place-items-center rounded-m text-text hover:bg-sunken data-[state=open]:bg-sunken",
            focusRing,
            className,
          )}
        >
          {open ? (
            <X aria-hidden="true" size={16} />
          ) : (
            <Menu aria-hidden="true" size={16} />
          )}
          {alert === null ? null : (
            <span
              aria-hidden="true"
              className={cn(
                "absolute top-10 right-10 size-8 rounded-full",
                alert.dot,
              )}
            />
          )}
        </button>
      </RadixDialog.Trigger>
      <RadixDialog.Portal>
        <RadixDialog.Content
          aria-describedby={undefined}
          className="fixed inset-x-0 top-(--height-bar-compact) bottom-0 z-dialog overflow-y-auto bg-surface pt-6 pb-20 outline-none"
        >
          <RadixDialog.Title className="sr-only">Menu</RadixDialog.Title>
          <nav aria-label="Project tabs">
            {tabs.entries.map((entry) => {
              const badge = tabs.badge(entry.id);
              const current = entry.id === tabs.active;
              return (
                <HrefLink
                  key={entry.id}
                  href={navPath(slug, entry)}
                  aria-current={current ? "page" : undefined}
                  onClick={close}
                  className={cn(
                    ROW,
                    current && "bg-sunken font-semibold",
                    insetFocusRing,
                  )}
                >
                  {entry.label}
                  {badge === null ? null : (
                    <span className="ml-auto">
                      <TabCount badge={badge} />
                    </span>
                  )}
                </HrefLink>
              );
            })}
          </nav>
          <div className={cn(GROUP, "flex items-center gap-8")}>
            Agents
            <span className="font-body font-regular">
              <StreamStatus announce={false} />
            </span>
          </div>
          <HrefLink
            href={projectPath(slug, "agents")}
            onClick={close}
            className={cn(ROW, insetFocusRing)}
          >
            {agentsSummary(workingAgents(list, paused), list.length)}
            <ChevronRight
              aria-hidden="true"
              size={16}
              className="ml-auto text-text-2"
            />
          </HrefLink>
          {human === null || project.data?.overview == null ? null : (
            <div className="px-16 py-6">
              <PauseControl slug={slug} className="items-start" />
            </div>
          )}
          <div className={GROUP}>Theme</div>
          <div className="px-16 py-6">
            <ThemeChoice />
          </div>
          {human === null ? null : (
            <div className={GROUP}>
              Signed in as{" "}
              <PersonName name={humanActor(human)} className="text-13" />
            </div>
          )}
          <SignOut />
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
