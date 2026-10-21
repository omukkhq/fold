import { AgentChips } from "./AgentChips";
import { NeedsYou } from "./NeedsYou";
import { PauseControl } from "./PauseControl";
import { Wordmark } from "@/components/Wordmark";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { StreamStatus } from "./StreamStatus";
import { ThemeToggle } from "./ThemeToggle";

export function TopBar({ slug }: { slug: string }) {
  return (
    <header className="flex h-bar shrink-0 items-center justify-between border-b border-border-subtle bg-surface px-24">
      <div className="flex items-center gap-14">
        <Wordmark />
        <span className="h-20 w-px bg-border" aria-hidden />
        <ProjectSwitcher slug={slug} />
      </div>
      <div className="flex items-center gap-16">
        <StreamStatus />
        <PauseControl slug={slug} />
        <NeedsYou slug={slug} />
        <AgentChips slug={slug} />
        <ThemeToggle />
      </div>
    </header>
  );
}
