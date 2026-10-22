import { forwardRef, useEffect, useLayoutEffect, useRef } from "react";
import { cn } from "@/lib/cn";
import { composerHeight, composerKey, lineHeightOf, parsePx } from "./composer";
import { MentionTextarea, type MentionTextareaProps } from "./MentionTextarea";

export type MessageComposerProps = Omit<MentionTextareaProps, "rows"> & {
  /** True while a send is pending: the box is read-only and Enter sends nothing. */
  pending: boolean;
};

function fit(element: HTMLTextAreaElement) {
  const style = window.getComputedStyle(element);
  element.style.height = "auto";
  const { height, scrolls } = composerHeight({
    scrollHeight: element.scrollHeight,
    lineHeight: lineHeightOf(style.lineHeight, style.fontSize),
    paddingY: parsePx(style.paddingTop, 0) + parsePx(style.paddingBottom, 0),
    borderY:
      parsePx(style.borderTopWidth, 0) + parsePx(style.borderBottomWidth, 0),
  });
  element.style.height = `${String(height)}px`;
  element.style.overflowY = scrolls ? "auto" : "hidden";
}

/**
 * A one-message box: it grows with its text up to six lines and then scrolls, Enter submits its form as the send button does and Shift+Enter starts a new line. An Enter that picks a `#` or `@` suggestion never sends.
 */
export const MessageComposer = forwardRef<
  HTMLTextAreaElement,
  MessageComposerProps
>(function MessageComposer(
  {
    pending,
    onKeyDown,
    onKeyUp,
    onCompositionStart,
    readOnly,
    className,
    value,
    ...rest
  },
  ref,
) {
  const inner = useRef<HTMLTextAreaElement | null>(null);
  // Safari fires `compositionend` before the keydown of the Enter that ends it, so the flag lasts until that key comes up.
  const composing = useRef(false);
  useLayoutEffect(() => {
    if (inner.current) fit(inner.current);
  }, [value]);
  // A narrower box wraps the same text onto more lines.
  useEffect(() => {
    const element = inner.current;
    if (!element || typeof ResizeObserver === "undefined") return undefined;
    let width = element.clientWidth;
    const observer = new ResizeObserver(() => {
      if (element.clientWidth !== width) {
        width = element.clientWidth;
        fit(element);
      }
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, []);
  return (
    <MentionTextarea
      {...rest}
      ref={(element) => {
        inner.current = element;
        if (typeof ref === "function") ref(element);
        else if (ref) ref.current = element;
      }}
      rows={1}
      value={value}
      readOnly={pending || readOnly}
      className={cn("resize-none py-9", className)}
      onKeyDown={(event) => {
        const action = composerKey({
          key: event.key,
          shiftKey: event.shiftKey,
          isComposing: event.nativeEvent.isComposing || composing.current,
          pending: pending || readOnly === true,
          text: value,
        });
        if (action !== "pass") event.preventDefault();
        if (action === "send") event.currentTarget.form?.requestSubmit();
        onKeyDown?.(event);
      }}
      onKeyUp={(event) => {
        composing.current = event.nativeEvent.isComposing;
        onKeyUp?.(event);
      }}
      onCompositionStart={(event) => {
        composing.current = true;
        onCompositionStart?.(event);
      }}
    />
  );
});
