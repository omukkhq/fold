import type { ComponentProps } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionLink } from "@/mentions/MentionLink";
import type { Known } from "@/mentions/parse";
import { remarkMentions } from "@/mentions/remark";
import { usePeople } from "@/mentions/usePeople";

// Every element takes token classes; W03 draws the description as 14 / 400 body text with 600 subheads.
const COMPONENTS: Components = {
  p: ({ children }) => (
    <p className="text-body leading-normal text-text">{children}</p>
  ),
  h1: ({ children }) => (
    <h4 className="text-section font-semibold text-text">{children}</h4>
  ),
  h2: ({ children }) => (
    <h4 className="text-body font-semibold text-text">{children}</h4>
  ),
  h3: ({ children }) => (
    <h4 className="text-body font-semibold text-text">{children}</h4>
  ),
  h4: ({ children }) => (
    <h4 className="text-body font-semibold text-text">{children}</h4>
  ),
  h5: ({ children }) => (
    <h4 className="text-body font-semibold text-text">{children}</h4>
  ),
  h6: ({ children }) => (
    <h4 className="text-body font-semibold text-text">{children}</h4>
  ),
  strong: ({ children }) => (
    <strong className="font-semibold">{children}</strong>
  ),
  ul: ({ children, className }) => (
    <ul
      className={cn(
        "flex flex-col gap-4 pl-20 text-body text-text",
        className?.includes("contains-task-list")
          ? "list-none pl-0"
          : "list-disc",
      )}
    >
      {children}
    </ul>
  ),
  ol: ({ children }) => (
    <ol className="flex list-decimal flex-col gap-4 pl-20 text-body text-text">
      {children}
    </ol>
  ),
  li: ({ children, className }) => (
    <li
      className={cn(
        "leading-normal",
        className?.includes("task-list-item") && "flex items-center gap-8",
      )}
    >
      {children}
    </li>
  ),
  input: ({ checked, type }) =>
    type === "checkbox" ? (
      <input
        type="checkbox"
        checked={checked ?? false}
        disabled
        readOnly
        className="size-14 accent-primary"
      />
    ) : null,
  a: ({ children, href, node }) => {
    const tag = node?.properties.dataMention;
    return typeof tag === "string" && (href || tag === "human") ? (
      <MentionLink href={href ?? null} tag={tag}>
        {children}
      </MentionLink>
    ) : (
      <a
        href={href}
        className={cn(
          "text-primary-text underline underline-offset-2",
          focusRing,
        )}
      >
        {children}
      </a>
    );
  },
  code: ({ children }) => (
    <code className="rounded-xs bg-sunken px-4 font-mono text-meta">
      {children}
    </code>
  ),
  pre: ({ children }) => (
    <pre className="overflow-x-auto rounded-m bg-sunken p-12 font-mono text-meta">
      {children}
    </pre>
  ),
  blockquote: ({ children }) => (
    <blockquote className="border-l-2 border-border pl-12 text-text-muted">
      {children}
    </blockquote>
  ),
  table: ({ children }) => (
    <div className="overflow-x-auto">
      <table className="border-collapse text-bar text-text">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border border-border-subtle bg-sunken px-10 py-5 text-left font-semibold">
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td className="border border-border-subtle px-10 py-5">{children}</td>
  ),
  hr: () => <hr className="border-border-subtle" />,
  img: ({ alt }) => <span className="text-text-muted">{alt}</span>,
};

/**
 * The task description. GFM adds tables and task lists; raw HTML is dropped, never rendered, because a description is
 * written by agents and humans and shown to everyone. Images show their alt text: the bundle loads nothing remote.
 * With `mentions`, `#` and `@` mentions in the text link to their screens.
 */
export function Markdown({
  children,
  mentions,
}: {
  children: string;
  mentions?: { slug: string; known?: Known };
}) {
  return mentions ? (
    <WithMentions mentions={mentions}>{children}</WithMentions>
  ) : (
    <Body plugins={[remarkGfm]}>{children}</Body>
  );
}

function WithMentions({
  children,
  mentions,
}: {
  children: string;
  mentions: { slug: string; known?: Known };
}) {
  const people = usePeople(mentions.slug);
  return (
    <Body plugins={[remarkGfm, remarkMentions({ ...mentions, people })]}>
      {children}
    </Body>
  );
}

function Body({
  children,
  plugins,
}: {
  children: string;
  plugins: NonNullable<ComponentProps<typeof ReactMarkdown>["remarkPlugins"]>;
}) {
  return (
    <div className="flex flex-col gap-6">
      <ReactMarkdown remarkPlugins={plugins} skipHtml components={COMPONENTS}>
        {children}
      </ReactMarkdown>
    </div>
  );
}
