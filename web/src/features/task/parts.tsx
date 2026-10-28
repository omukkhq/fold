import type { LucideIcon } from "lucide-react";
import type { ComponentProps, ReactNode, SyntheticEvent } from "react";
import { ApiError } from "@/api/errors";
import { HrefLink } from "@/app/HrefLink";
import {
  buttonClasses,
  controlLabel,
  isInert,
  type ButtonVariant,
  type PendingProps,
} from "@/components/button/buttonStyles";
import { ErrorBanner } from "@/components/states/ErrorBanner";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { describeError } from "./errors";

/** react-hook-form's submit handler returns a promise that a form event cannot take; validation errors land in the form state. */
export function submit(
  handler: (event?: SyntheticEvent) => Promise<void>,
): (event: SyntheticEvent) => void {
  return (event) => {
    void handler(event);
  };
}

/** A main-column section title. */
export function SectionHead({
  title,
  children,
}: {
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-center gap-10">
      <h3 className="font-title text-18 leading-tight font-semibold text-text">
        {title}
      </h3>
      {children}
    </div>
  );
}

/** One of the side panel's three groups, People, Work and Links: a surface block on the panel's ground. */
export function SideCard({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className="flex flex-col gap-14 rounded-m bg-surface px-16 pt-14 pb-16"
    >
      <h3 className="font-title text-16 leading-tight font-semibold text-text">
        {title}
      </h3>
      {children}
    </section>
  );
}

/** One row of a side group: the field label on the left, the value at 14px on the right, and an optional control after the value, which wraps under it when the row is narrow. */
export function SideGroup({
  title,
  action,
  stacked = false,
  children,
}: {
  title: string;
  /** A control at the end of the row. */
  action?: ReactNode;
  /** The label sits above a value that needs the group's whole width, such as a chart. */
  stacked?: boolean;
  children: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className={cn(
        "grid items-baseline gap-x-12 gap-y-6",
        stacked
          ? "grid-cols-1"
          : "grid-cols-[var(--width-side-label)_minmax(0,1fr)]",
      )}
    >
      <h4 className="font-title text-13 leading-tight font-semibold text-text-2">
        {title}
      </h4>
      <div className="flex min-w-0 flex-wrap items-start gap-x-12 gap-y-4">
        <div className="flex min-w-0 grow basis-auto flex-col gap-4 text-14 text-text">
          {children}
        </div>
        {action}
      </div>
    </section>
  );
}

/** The group's empty items in one line, so they take no row each; nothing when no item is empty. */
export function NothingYet({ text }: { text: string | null }) {
  if (text === null) {
    return null;
  }
  return (
    <p className="text-14 leading-normal text-text-2">
      <span className="font-medium text-text">Nothing yet:</span> {text}
    </p>
  );
}

/** A secondary line under a side value: a time, a claim, a base. */
export function SideNote({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return <p className={cn("text-13 text-text-2", className)}>{children}</p>;
}

/** A link drawn as a button, for actions that open another screen. */
export function LinkButton({
  href,
  variant = "quiet",
  icon: Icon,
  children,
}: {
  href: string;
  variant?: ButtonVariant;
  icon: LucideIcon;
  children: string;
}) {
  return (
    <HrefLink
      href={href}
      className={cn(buttonClasses(variant, "md"), focusRing)}
    >
      <Icon
        aria-hidden="true"
        size={16}
        className={variant === "quiet" ? "text-text-2" : undefined}
      />
      {children}
    </HrefLink>
  );
}

/** A small action drawn as accent text, such as `Change` and `Add label`. */
export function SmallAction({
  icon: Icon,
  children,
  className,
  pending,
  pendingLabel,
  disabled,
  ...rest
}: Omit<ComponentProps<"button">, "children"> &
  PendingProps & {
    icon: LucideIcon;
    children: string;
  }) {
  return (
    <button
      type="button"
      {...rest}
      disabled={isInert(disabled, pending)}
      aria-busy={pending ? true : undefined}
      className={cn(
        "inline-flex shrink-0 items-center gap-6 rounded-s font-title text-14 font-semibold whitespace-nowrap text-accent hover:underline disabled:cursor-not-allowed disabled:opacity-60",
        focusRing,
        className,
      )}
    >
      <Icon aria-hidden="true" size={14} />
      {controlLabel(children, pending, pendingLabel)}
    </button>
  );
}

/** A refused write: a mapped refusal names its cause, any other shows the daemon's message. A refusal that belongs to a form field is shown by the field. */
export function WriteError({ error }: { error: Error | null }) {
  if (error === null) {
    return null;
  }
  const view = describeError(error);
  if (view?.field) {
    return null;
  }
  if (view) {
    return (
      <p role="alert" className="text-13 font-medium text-bad">
        {view.text}
      </p>
    );
  }
  return (
    <ErrorBanner
      className="rounded-m border-b-0 px-12"
      title="The change was refused."
      message={
        error instanceof ApiError && error.message !== ""
          ? error.message
          : "The daemon did not answer."
      }
    />
  );
}
