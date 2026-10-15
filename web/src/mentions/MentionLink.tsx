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
import type { ReactNode } from "react";
import { HrefLink } from "@/app/HrefLink";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";

const ICONS: Record<string, LucideIcon> = {
  task: ListChecks,
  proposal: GitPullRequest,
  request: Inbox,
  revision: History,
  message: MessageSquare,
  path: FileText,
};

/** A mention as a link chip: the icon says what it names, the text stays as written. */
export function MentionLink({
  href,
  tag,
  children,
}: {
  href: string;
  /** The item kind, or `path` for a file. */
  tag: string;
  children: ReactNode;
}) {
  const Icon = ICONS[tag] ?? GitCompare;
  return (
    <HrefLink
      href={href}
      data-mention={tag}
      className={cn(
        "inline-flex items-center gap-4 rounded-s bg-primary-soft px-5 align-baseline font-mono text-code text-primary-text hover:underline",
        focusRing,
      )}
    >
      <Icon aria-hidden="true" size={12} className="shrink-0" />
      {children}
    </HrefLink>
  );
}
