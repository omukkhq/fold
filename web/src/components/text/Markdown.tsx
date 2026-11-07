import {
  createContext,
  memo,
  useContext,
  useMemo,
  type ComponentProps,
  type ReactNode,
} from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { HrefLink } from "@/app/HrefLink";
import { InlineCode } from "@/components/code/CodeText";
import { SyntaxText } from "@/components/code/SyntaxText";
import { ID_LINK_CLASSES } from "@/components/id/idStyles";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionLink } from "@/mentions/MentionLink";
import type { Known } from "@/mentions/parse";
import { remarkMentions } from "@/mentions/remark";
import { usePeople } from "@/mentions/usePeople";
import { languageForFence } from "@/syntax/languages";
import type { ClampLines } from "./clamp";
import {
  headingSizeClass,
  linkTarget,
  markdownBoxClass,
  markdownSizeClass,
  remarkHtmlAsText,
  remarkLineBreaks,
  type MarkdownSize,
} from "./markdownStyle";
import { CODE_BOX } from "./textFlow";

export type MarkdownPlugins = NonNullable<
  ComponentProps<typeof ReactMarkdown>["remarkPlugins"]
>;

/** A fenced block's text, highlighted when its fence names a language Shiki has a grammar for; a bare fence stays plain. */
function FencedCode({
  code,
  className,
}: {
  code: string;
  className: string | undefined;
}) {
  const language = languageForFence(className);
  if (language === null) return code;
  return <SyntaxText code={code} language={language} />;
}

/** True inside a `pre`: react-markdown draws a fenced block and inline code with the same `code` component. */
const InFence = createContext(false);

/** True when the text sits on the sunken surface, where inline code takes the plain surface. */
const OnSunken = createContext(false);

/** Inline code is drawn the same way as a command in the app's own sentences, except that a long path in it wraps; a fenced block keeps its block look. */
function Code({
  children,
  className,
}: {
  children?: ReactNode;
  className: string | undefined;
}) {
  const onSunken = useContext(OnSunken);
  if (!useContext(InFence)) {
    return (
      <InlineCode onSunken={onSunken} wrap>
        {children}
      </InlineCode>
    );
  }
  return (
    <code className="rounded-s bg-sunken px-4 font-mono text-13">
      {typeof children === "string" ? (
        <FencedCode code={children} className={className} />
      ) : (
        children
      )}
    </code>
  );
}

/** Every element takes token classes and the size of the text around it. */
function components(size: MarkdownSize): Components {
  const heading = (level: number) =>
    function Heading({ children }: { children?: ReactNode }) {
      return (
        <h4 className={cn(headingSizeClass(size, level), "font-semibold")}>
          {children}
        </h4>
      );
    };
  return {
    p: ({ children }) => <p>{children}</p>,
    h1: heading(1),
    h2: heading(2),
    h3: heading(3),
    h4: heading(4),
    h5: heading(5),
    h6: heading(6),
    strong: ({ children }) => (
      <strong className="font-semibold">{children}</strong>
    ),
    // Lists flow as blocks, not flex columns: a line clamp counts only the lines of block flow, so a clamped preview
    // with a list would otherwise run past its lines.
    ul: ({ children, className }) => (
      <ul
        className={cn(
          "space-y-4 pl-20",
          className?.includes("contains-task-list")
            ? "list-none pl-0"
            : "list-disc",
        )}
      >
        {children}
      </ul>
    ),
    ol: ({ children }) => (
      <ol className="list-decimal space-y-4 pl-20">{children}</ol>
    ),
    li: ({ children, className }) => (
      <li
        className={cn(
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
          className="size-14 accent-accent"
        />
      ) : null,
    a: ({ children, href, node }) => {
      const tag = node?.properties.dataMention;
      if (typeof tag === "string" && (href || tag === "human")) {
        return (
          <MentionLink href={href ?? null} tag={tag}>
            {children}
          </MentionLink>
        );
      }
      if (node?.properties.dataId !== undefined && href) {
        return (
          <HrefLink href={href} className={cn(ID_LINK_CLASSES, focusRing)}>
            {children}
          </HrefLink>
        );
      }
      return (
        <a
          href={href}
          {...linkTarget(href)}
          className={cn("text-accent underline underline-offset-2", focusRing)}
        >
          {children}
        </a>
      );
    },
    code: ({ children, className }) => (
      <Code className={className}>{children}</Code>
    ),
    pre: ({ children }) => (
      <pre
        className={cn(CODE_BOX, "rounded-m bg-sunken p-12 font-mono text-13")}
      >
        <InFence.Provider value>{children}</InFence.Provider>
      </pre>
    ),
    blockquote: ({ children }) => (
      <blockquote className="border-l-2 border-line pl-12 text-text-2">
        {children}
      </blockquote>
    ),
    table: ({ children }) => (
      <div className="overflow-x-auto wrap-normal">
        <table className="border-collapse text-14">{children}</table>
      </div>
    ),
    th: ({ children }) => (
      <th className="border border-line bg-sunken px-10 py-5 text-left font-semibold">
        {children}
      </th>
    ),
    td: ({ children }) => (
      <td className="border border-line px-10 py-5">{children}</td>
    ),
    hr: () => <hr className="border-line" />,
    img: ({ alt }) => <span className="text-text-2">{alt}</span>,
  };
}

const COMPONENTS: Record<MarkdownSize, Components> = {
  13: components(13),
  14: components(14),
  15: components(15),
  16: components(16),
};

/** A document drops raw HTML and joins its lines as Markdown does; other text shows HTML as typed and keeps its line ends. */
const DOCUMENT: MarkdownPlugins = [remarkGfm];
const PROSE: MarkdownPlugins = [remarkGfm, remarkHtmlAsText, remarkLineBreaks];

interface Look {
  size: MarkdownSize;
  clamp: ClampLines | null;
  className: string | undefined;
}

export interface MarkdownProps {
  children: string;
  /** Links `#` and `@` mentions in the text to their screens. */
  mentions?: { slug: string; known?: Known };
  /** More remark plugins, run after the mentions, such as one that links bare ids. Keep the array stable between renders. */
  plugins?: MarkdownPlugins;
  size?: MarkdownSize;
  /** Cuts the rendered text to its first lines with an ellipsis. */
  clamp?: ClampLines;
  /** The text sits on the sunken surface. */
  onSunken?: boolean;
  /** The text was written in a Markdown form, such as a task description: raw HTML is dropped, as the form says, and lines join into paragraphs as Markdown joins them. */
  document?: boolean;
  /** The colour and width of the text box, such as `text-text-2` or `max-w-measure`. */
  className?: string;
}

/**
 * Text that an agent or a person wrote, rendered as Markdown. GFM adds tables and task lists, and each line end is a
 * line break, except in a `document`. Raw HTML never becomes an element, because the text is shown to everyone: it
 * shows as typed, or in a `document` not at all. A link keeps only a safe address. Images show their alt text: the bundle loads nothing remote. A long word or path wraps anywhere.
 */
export function Markdown({
  children,
  mentions,
  plugins,
  size = 15,
  clamp,
  onSunken = false,
  document: isDocument = false,
  className,
}: MarkdownProps) {
  const look: Look = { size, clamp: clamp ?? null, className };
  const base = isDocument ? DOCUMENT : PROSE;
  return (
    <OnSunken.Provider value={onSunken}>
      {mentions ? (
        <WithMentions
          mentions={mentions}
          base={base}
          extra={plugins}
          look={look}
        >
          {children}
        </WithMentions>
      ) : (
        <WithPlugins base={base} extra={plugins} look={look}>
          {children}
        </WithPlugins>
      )}
    </OnSunken.Provider>
  );
}

function WithPlugins({
  children,
  base,
  extra,
  look,
}: {
  children: string;
  base: MarkdownPlugins;
  extra: MarkdownPlugins | undefined;
  look: Look;
}) {
  const plugins = useMemo(
    () => (extra === undefined ? base : [...base, ...extra]),
    [base, extra],
  );
  return (
    <Body plugins={plugins} {...look}>
      {children}
    </Body>
  );
}

function WithMentions({
  children,
  mentions,
  base,
  extra,
  look,
}: {
  children: string;
  mentions: { slug: string; known?: Known };
  base: MarkdownPlugins;
  extra: MarkdownPlugins | undefined;
  look: Look;
}) {
  const people = usePeople(mentions.slug);
  const { slug, known } = mentions;
  const plugins = useMemo(
    () => [...base, remarkMentions({ slug, known, people }), ...(extra ?? [])],
    [base, slug, known, people, extra],
  );
  return (
    <Body plugins={plugins} {...look}>
      {children}
    </Body>
  );
}

/** Re-parses only when the text, the plugins or the look change, so a live frame that leaves the text alone costs nothing. */
const Body = memo(function Body({
  children,
  plugins,
  size,
  clamp,
  className,
}: Look & { children: string; plugins: MarkdownPlugins }) {
  return (
    <div
      className={cn(
        markdownBoxClass(clamp),
        markdownSizeClass(size),
        "leading-normal wrap-anywhere text-text",
        className,
      )}
    >
      <ReactMarkdown
        remarkPlugins={plugins}
        skipHtml
        components={COMPONENTS[size]}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
});
