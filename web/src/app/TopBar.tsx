import { Fragment, type ReactNode } from "react";
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
import { TopBarMore } from "./TopBarMore";
import {
  TOP_BAR_ITEMS,
  moreItems,
  rowItemClass,
  type TopBarItem,
} from "./topBarLayout";

/** An item of the top bar; `more` is given when the item sits in the More popover, with the way to close it. */
function part(
  item: TopBarItem,
  slug: string,
  more?: { close: () => void },
): ReactNode {
  switch (item) {
    case "stream":
      return <StreamStatus />;
    case "pause":
      return <PauseControl slug={slug} />;
    case "needsYou":
      return <NeedsYou slug={slug} />;
    case "agents":
      return <AgentChips slug={slug} onOpen={more?.close} />;
    case "bell":
      return <NotificationBell slug={slug} />;
    case "name":
      return <SignedInName />;
    case "theme":
      return <ThemeToggle variant={more === undefined ? "icon" : "row"} />;
  }
}

export function TopBar({ slug }: { slug: string }) {
  return (
    <header className="flex h-bar shrink-0 items-center justify-between gap-8 overflow-x-auto border-b border-border-subtle bg-surface px-16 md:px-24 lg:gap-16">
      <div className="flex shrink-0 items-center gap-14 max-lg:contents">
        <HrefLink
          href="/projects"
          aria-label="fold, all projects"
          className={cn(
            "shrink-0 rounded-s max-lg:flex max-lg:min-h-44 max-lg:items-center",
            focusRing,
          )}
        >
          <Wordmark />
        </HrefLink>
        <span className="h-20 w-px bg-border max-lg:hidden" aria-hidden />
        <ProjectSwitcher slug={slug} />
      </div>
      <div className="flex shrink-0 items-center lg:gap-16">
        {TOP_BAR_ITEMS.map((item) => (
          <div key={item} className={cn("contents", rowItemClass(item))}>
            {part(item, slug)}
          </div>
        ))}
        <TopBarMore slug={slug}>
          {(close) =>
            moreItems().map((item) => (
              <Fragment key={item}>{part(item, slug, { close })}</Fragment>
            ))
          }
        </TopBarMore>
      </div>
    </header>
  );
}
