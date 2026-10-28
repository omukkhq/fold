import {
  AtSign,
  CircleAlert,
  CornerDownLeft,
  Loader2,
  Send,
} from "lucide-react";
import { useLayoutEffect, useRef, type ComponentProps } from "react";
import { buttonClasses } from "@/components/button/buttonStyles";
import { IconButton } from "@/components/button/Button";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MessageComposer } from "@/mentions/MessageComposer";
import { PersonName } from "@/people/PersonName";
import { insertMention } from "./compose";

interface ReplyBoxProps {
  slug: string;
  value: string;
  onValueChange: (value: string) => void;
  onBlur: () => void;
  /** Receives the textarea, for the form library and for focus. */
  boxRef: (element: HTMLTextAreaElement | null) => void;
  placeholder: string;
  pending: boolean;
  /** The agent whose question the text answers, or null for a plain reply. */
  answering: string | null;
  invalid: string | null;
  failure: string | null;
  onSubmit: ComponentProps<"form">["onSubmit"];
}

function Key({ children }: { children: string }) {
  return (
    <kbd className="rounded-s px-5 font-mono text-13 text-text-2 inset-ring inset-ring-line">
      {children}
    </kbd>
  );
}

/**
 * The thread's reply box: one bordered box with the text on top and a row with the mention button, the keys and Send below; at phone width a round text box beside a round Send button.
 */
export function ReplyBox({
  slug,
  value,
  onValueChange,
  onBlur,
  boxRef,
  placeholder,
  pending,
  answering,
  invalid,
  failure,
  onSubmit,
}: ReplyBoxProps) {
  const box = useRef<HTMLTextAreaElement | null>(null);
  const caret = useRef<number | null>(null);
  useLayoutEffect(() => {
    const at = caret.current;
    if (at !== null && box.current) {
      box.current.focus();
      box.current.setSelectionRange(at, at);
      caret.current = null;
    }
  }, [value]);

  return (
    <form
      onSubmit={onSubmit}
      noValidate
      className="flex shrink-0 flex-col gap-6 border-t border-line bg-bg px-16 pt-8 pb-14 lg:px-28 lg:pt-12 lg:pb-18"
    >
      <div className="flex w-full max-w-thread-column flex-col gap-6 sm:gap-0 sm:rounded-l sm:bg-surface sm:shadow-pop sm:inset-ring sm:inset-ring-line sm:has-[textarea:focus-visible]:outline-2 sm:has-[textarea:focus-visible]:outline-offset-2 sm:has-[textarea:focus-visible]:outline-accent">
        {answering === null ? null : (
          <p className="flex items-center gap-6 text-13 text-warn sm:px-14 sm:pt-8">
            <CornerDownLeft aria-hidden="true" size={13} className="shrink-0" />
            <span>
              Your own answer to the question of <PersonName name={answering} />
            </span>
          </p>
        )}
        <div className="flex items-end gap-8 sm:flex-col sm:items-stretch sm:gap-0">
          <MessageComposer
            slug={slug}
            aria-label="Reply"
            placeholder={placeholder}
            pending={pending}
            aria-invalid={invalid === null ? undefined : true}
            ref={(element) => {
              box.current = element;
              boxRef(element);
            }}
            value={value}
            onValueChange={onValueChange}
            onBlur={onBlur}
            className="min-w-0 flex-1 rounded-l px-14 py-10 sm:flex-none sm:rounded-none sm:bg-transparent sm:pt-8 sm:pb-4 sm:inset-ring-0 sm:focus-visible:outline-0"
          />
          <div className="contents sm:flex sm:items-center sm:gap-8 sm:py-8 sm:pr-8 sm:pl-10">
            <IconButton
              aria-label="Mention an agent or a person"
              className="hidden sm:inline-flex"
              disabled={pending}
              onClick={() => {
                const next = insertMention(
                  value,
                  box.current?.selectionStart ?? value.length,
                );
                caret.current = next.caret;
                onValueChange(next.text);
              }}
            >
              <AtSign aria-hidden="true" size={16} />
            </IconButton>
            <span className="hidden text-13 text-text-2 sm:inline">
              <Key>Enter</Key> to send · <Key>Shift</Key> + <Key>Enter</Key> for
              a new line
            </span>
            <span aria-hidden="true" className="hidden flex-1 sm:block" />
            <button
              type="submit"
              disabled={pending}
              aria-busy={pending ? true : undefined}
              className={cn(
                buttonClasses("primary", "md"),
                "size-44 shrink-0 rounded-full p-0 sm:size-auto sm:rounded-m sm:px-14 sm:py-7",
                focusRing,
              )}
            >
              {pending ? (
                <Loader2
                  aria-hidden="true"
                  size={15}
                  className="animate-spin motion-reduce:animate-none"
                />
              ) : (
                <Send aria-hidden="true" size={15} />
              )}
              <span className="sr-only sm:not-sr-only">
                {pending ? "Sending…" : "Send"}
              </span>
            </button>
          </div>
        </div>
      </div>
      {invalid === null ? null : (
        <p className="text-13 font-medium text-bad">{invalid}</p>
      )}
      {failure === null ? null : (
        <div className="flex flex-wrap items-center gap-x-8 gap-y-2">
          <p
            role="alert"
            className="flex items-center gap-6 text-13 font-medium text-bad"
          >
            <CircleAlert aria-hidden="true" size={13} className="shrink-0" />
            Not sent: {failure}
          </p>
          <button
            type="submit"
            disabled={pending}
            className={cn(
              "rounded-s text-13 font-semibold text-text underline underline-offset-3",
              focusRing,
            )}
          >
            Send again
          </button>
        </div>
      )}
    </form>
  );
}
