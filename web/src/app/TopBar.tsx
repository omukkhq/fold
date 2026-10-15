import { AgentChips } from "./AgentChips";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { StreamStatus } from "./StreamStatus";
import { ThemeToggle } from "./ThemeToggle";

export function TopBar({ slug }: { slug: string }) {
  return (
    <header className="flex h-bar items-center justify-between border-b border-border-subtle bg-surface px-24">
      <div className="flex items-center gap-14">
        <span className="text-wordmark font-bold text-text">fold</span>
        <span className="h-20 w-px bg-border" aria-hidden />
        <ProjectSwitcher slug={slug} />
      </div>
      <div className="flex items-center gap-16">
        <StreamStatus />
        <AgentChips slug={slug} />
        <ThemeToggle />
      </div>
    </header>
  );
}
