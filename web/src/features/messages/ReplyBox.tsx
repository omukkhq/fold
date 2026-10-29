import {
  AtSign,
  CircleAlert,
  CornerDownLeft,
  Loader2,
  Send,
} from "lucide-react";
import {
  useLayoutEffect,
  useRef,
  type ComponentProps,
  type ReactNode,
} from "react";
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
  /** The choice of what the reply is about, above the text. */
  about?: ReactNode;
  invalid: string | null;
  failure: string | null;
  onSubmit: ComponentProps<"form">["onSubmit"];
}

/**
 * The thread's reply box: one filled box with the text on top and a row with the mention button and Send below; at phone width the text and a round Send button share one line.
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
  about = null,
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
      className="flex shrink-0 flex-col gap-6 px-12 pt-6 pb-14 lg:px-28 lg:pt-8 lg:pb-20"
    >
      <div className="flex w-full max-w-thread-column flex-wrap items-end gap-x-8 gap-y-2 rounded-l bg-surface py-6 pr-6 pl-14 has-[textarea:focus-visible]:outline-2 has-[textarea:focus-visible]:outline-offset-2 has-[textarea:focus-visible]:outline-accent sm:flex-col sm:flex-nowrap sm:items-stretch sm:gap-4 sm:py-10 sm:pr-10 sm:pl-16">
        {about === null ? null : (
          <div className="flex basis-full items-center gap-6 pt-4 text-13 text-text-2 sm:basis-auto sm:pt-0">
            {about}
          </div>
        )}
        {answering === null ? null : (
          <p className="flex basis-full items-center gap-6 pt-4 text-13 text-text-2 sm:basis-auto sm:pt-0">
            <CornerDownLeft aria-hidden="true" size={14} className="shrink-0" />
            <span>
              Your own answer to the question of <PersonName name={answering} />
            </span>
          </p>
        )}
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
          className="min-w-0 flex-1 rounded-none bg-transparent px-0 py-8 leading-normal focus-visible:outline-0 sm:flex-none sm:py-4"
        />
        <div className="contents sm:flex sm:items-center sm:gap-8">
          <IconButton
            aria-label="Mention an agent or a person"
            variant="ghost"
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
          <span aria-hidden="true" className="hidden flex-1 sm:block" />
          <button
            type="submit"
            disabled={pending}
            aria-busy={pending ? true : undefined}
            className={cn(
              buttonClasses("primary", "md"),
              "size-40 shrink-0 rounded-full p-0 sm:size-auto sm:rounded-s sm:px-14 sm:py-7",
              focusRing,
            )}
          >
            {pending ? (
              <Loader2
                aria-hidden="true"
                size={16}
                className="animate-spin motion-reduce:animate-none"
              />
            ) : (
              <Send aria-hidden="true" size={16} />
            )}
            <span className="sr-only sm:not-sr-only">
              {pending ? "Sending…" : "Send"}
            </span>
          </button>
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
            <CircleAlert aria-hidden="true" size={14} className="shrink-0" />
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
