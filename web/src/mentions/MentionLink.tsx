import {
  FileText,
  GitCompare,
  GitPullRequest,
  History,
  Inbox,
  ListChecks,
  MessageSquare,
  type LucideIcon,
} from "lucide-react";
import { Children, type ReactNode } from "react";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { chipLabel } from "./chip";

const ICONS: Record<string, LucideIcon> = {
  task: ListChecks,
  proposal: GitPullRequest,
  request: Inbox,
  revision: History,
  message: MessageSquare,
  path: FileText,
};

/** A person mention stays a coloured `@name`; an item or file mention is a link chip with its icon and no mark. */
export function MentionLink({
  href,
  tag,
  children,
}: {
  href: string | null;
  /** The item kind, `path` for a file, `human` or `agent` for a person. */
  tag: string;
  children: ReactNode;
}) {
  if (tag === "human") {
    return (
      <span
        data-mention="human"
        className="font-ui font-medium text-primary-text"
      >
        {children}
      </span>
    );
  }
  if (tag === "agent" && href !== null) {
    return (
      <HrefLink
        href={href}
        data-mention="agent"
        className={cn(
          "font-ui font-medium text-secondary-text hover:underline",
          focusRing,
        )}
      >
        {children}
      </HrefLink>
    );
  }
  const Icon = ICONS[tag] ?? GitCompare;
  return (
    <HrefLink
      href={href ?? ""}
      data-mention={tag}
      className={cn(
        "inline-flex items-center gap-4 rounded-s bg-primary-soft px-5 align-baseline font-mono text-code text-primary-text hover:underline",
        focusRing,
      )}
    >
      <Icon aria-hidden="true" size={12} className="shrink-0" />
      {Children.map(children, (child) =>
        typeof child === "string" ? chipLabel(child) : child,
      )}
    </HrefLink>
  );
}
