import { Plus } from "lucide-react";
import { Wordmark } from "@/components/Wordmark";
import { buttonClasses } from "@/components/button/buttonStyles";
import { newTaskPath } from "@/features/board/card";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { AgentsControl } from "./AgentsControl";
import { AvatarMenu } from "./AvatarMenu";
import { HrefLink } from "./HrefLink";
import { Inbox } from "./Inbox";
import { NavTabs } from "./NavTabs";
import { PhoneMenu } from "./PhoneMenu";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { StreamAnnouncer } from "./StreamStatus";
import { NARROW_ONLY, WIDE_ONLY } from "./topBarLayout";

/** One bar for every project screen: the wordmark, the project, the tabs, the Agents control, the Inbox, New task and the avatar. Below `xl` the tabs, the agents and the avatar move into the menu. */
export function TopBar({ slug }: { slug: string }) {
  return (
    <header className="flex h-bar-compact shrink-0 items-center gap-4 bg-surface pr-6 pl-16 wide:h-bar wide:gap-16 wide:px-20">
      <HrefLink
        href="/projects"
        aria-label="fold, all projects"
        className={cn(
          "flex shrink-0 items-center rounded-s max-wide:min-h-44",
          focusRing,
        )}
      >
        <Wordmark />
      </HrefLink>
      <ProjectSwitcher slug={slug} />
      <NavTabs slug={slug} className={WIDE_ONLY} />
      <span className="flex-1" />
      <div className={cn("flex", WIDE_ONLY)}>
        <AgentsControl slug={slug} />
      </div>
      <Inbox slug={slug} />
      <HrefLink
        href={newTaskPath(slug)}
        aria-label="New task"
        className={cn(
          buttonClasses("primary", "md"),
          "shrink-0 max-wide:size-40 max-wide:p-0",
          focusRing,
        )}
      >
        <Plus aria-hidden="true" size={16} />
        <span className="max-wide:sr-only">New task</span>
      </HrefLink>
      <AvatarMenu className={WIDE_ONLY} />
      <PhoneMenu slug={slug} className={NARROW_ONLY} />
      <StreamAnnouncer />
    </header>
  );
}
