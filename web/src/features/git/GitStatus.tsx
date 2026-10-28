import { useQuery } from "@tanstack/react-query";
import { GitBranch } from "lucide-react";
import type { ReactNode } from "react";
import { gitQuery } from "@/api/queries";
import { InlineCode } from "@/components/code/CodeText";
import { CommitId } from "@/components/mono/CommitId";
import { Mono } from "@/components/mono/Mono";
import { gitSummary, type RemoteLine } from "@/format/git";
import { cn } from "@/lib/cn";

const TONE: Record<RemoteLine["tone"], string> = {
  ok: "text-text-2",
  failed: "text-bad",
  idle: "text-text-2",
};

/** `card` is the full-width block under a project card, `panel` the bordered panel with a divider between rows. */
type Variant = "card" | "panel";

/** Below `md` a row wraps and its location takes a line of its own, where a long path breaks anywhere instead of widening the card. */
const ROW: Record<Variant, string> = {
  card: "flex h-16 items-center gap-12 max-md:h-auto max-md:flex-wrap max-md:gap-y-4",
  panel:
    "flex items-center gap-12 border-b border-line px-14 py-10 last:border-b-0 max-md:flex-wrap max-md:gap-y-4",
};

/** One grid for every row: key column, location column, branch column, then the status and the commit at the right. */
function Row({ variant, children }: { variant: Variant; children: ReactNode }) {
  return <li className={ROW[variant]}>{children}</li>;
}

function Key({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <span className={cn("flex w-git-key shrink-0 items-center", className)}>
      {children}
    </span>
  );
}

function Location({ children }: { children: ReactNode }) {
  return (
    <Mono className="w-git-location shrink-0 truncate text-13 text-text-2 max-md:order-last max-md:w-full max-md:whitespace-normal max-md:wrap-anywhere">
      {children}
    </Mono>
  );
}

function Branch({ name }: { name: string }) {
  return (
    <span className="flex w-git-branch shrink-0 items-center gap-4">
      <GitBranch aria-hidden size={13} className="shrink-0 text-text-2" />
      <Mono className="truncate text-13 text-text-2">{name}</Mono>
    </span>
  );
}

/** The project's Git repository and branch, then each remote with its last push; without a remote, the command that adds one. It renders nothing until the read arrives, and nothing when the read fails, because the screens around it carry their own error states. */
export function GitStatus({
  slug,
  variant = "card",
  className,
}: {
  slug: string;
  variant?: Variant;
  className?: string;
}) {
  const git = useQuery(gitQuery(slug));
  if (!git.data) {
    return null;
  }
  const summary = gitSummary(git.data);
  return (
    <section
      aria-label="Git status"
      className={cn(
        variant === "panel"
          ? "overflow-hidden rounded-l bg-surface inset-ring inset-ring-line"
          : null,
        className,
      )}
    >
      <ul className={cn("flex flex-col", variant === "card" ? "gap-4" : null)}>
        <Row variant={variant}>
          <Key>
            <span className="font-title text-13 leading-tight font-semibold text-text-2">
              GIT
            </span>
          </Key>
          <Location>{summary.repository}</Location>
          <Branch name={summary.branch} />
          <span className="flex-1" />
        </Row>
        {summary.addRemote ? (
          <Row variant={variant}>
            <Key className="max-md:hidden">{null}</Key>
            <span className="flex flex-wrap items-center gap-6 text-14 text-text-2">
              No remote. Add one:
              <InlineCode caption>{summary.addRemote}</InlineCode>
            </span>
          </Row>
        ) : null}
        {summary.remotes.map((remote) => (
          <Row key={remote.name} variant={variant}>
            <Key>
              <Mono className="truncate text-13 text-text">{remote.name}</Mono>
            </Key>
            <Location>{remote.location}</Location>
            <Branch name={remote.branch} />
            <span className="flex-1" />
            <span className={cn("min-w-0 text-14", TONE[remote.tone])}>
              {remote.status}
            </span>
            <span className="flex w-git-commit shrink-0 justify-end">
              <CommitId commit={remote.commit} className="text-text" />
            </span>
          </Row>
        ))}
      </ul>
    </section>
  );
}
