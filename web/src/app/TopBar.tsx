import { AgentChips } from "./AgentChips";
import { HrefLink } from "./HrefLink";
import { focusRing } from "../lib/focus";
import { cn } from "../lib/cn";
import { NeedsYou } from "./NeedsYou";
import { PauseControl } from "./PauseControl";
import { Wordmark } from "@/components/Wordmark";
import { NotificationBell } from "@/features/notifications/NotificationBell";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { SignedInName } from "./SignedInName";
import { StreamStatus } from "./StreamStatus";
import { ThemeToggle } from "./ThemeToggle";

export function TopBar({ slug }: { slug: string }) {
  return (
    <header className="flex h-bar shrink-0 items-center gap-16 overflow-x-auto justify-between border-b border-border-subtle bg-surface px-24">
      <div className="flex shrink-0 items-center gap-14">
        <HrefLink
          href="/projects"
          aria-label="fold, all projects"
          className={cn("rounded-s", focusRing)}
        >
          <Wordmark />
        </HrefLink>
        <span className="h-20 w-px bg-border" aria-hidden />
        <ProjectSwitcher slug={slug} />
      </div>
      <div className="flex shrink-0 items-center gap-16">
        <StreamStatus />
        <PauseControl slug={slug} />
        <NeedsYou slug={slug} />
        <AgentChips slug={slug} />
        <NotificationBell slug={slug} />
        <SignedInName />
        <ThemeToggle />
      </div>
    </header>
  );
}
