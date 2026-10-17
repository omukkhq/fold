import type { LucideIcon } from "lucide-react";
import type { ComponentProps, ReactNode, SyntheticEvent } from "react";
import type { TaskStatus } from "@/api/board";
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
import { statusLook } from "./text";

/** react-hook-form's submit handler returns a promise that a form event cannot take; validation errors land in the form state. */
export function submit(
  handler: (event?: SyntheticEvent) => Promise<void>,
): (event: SyntheticEvent) => void {
  return (event) => {
    void handler(event);
  };
}

/** A main-column section title, 16 / 600. */
export function SectionHead({
  title,
  children,
}: {
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="flex items-center gap-10">
      <h3 className="text-section leading-snug font-semibold text-text">
        {title}
      </h3>
      {children}
    </div>
  );
}

/** A side-column group with its tracked overline title. */
export function SideGroup({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section aria-label={title} className="flex flex-col gap-8">
      <h3 className="text-overline leading-snug font-semibold text-primary-text uppercase">
        {title}
      </h3>
      {children}
    </section>
  );
}

export function KeyValue({ name, value }: { name: string; value: string }) {
  return (
    <div className="flex items-center gap-8">
      <span className="w-skeleton-s shrink-0 text-bar text-text-muted">
        {name}
      </span>
      <span className="min-w-0 truncate font-mono text-meta text-text">
        {value}
      </span>
    </div>
  );
}

export function StatusMark({ status }: { status: TaskStatus }) {
  const look = statusLook(status);
  return (
    <span className="inline-flex shrink-0 items-center gap-5 text-meta text-text-muted">
      <span
        aria-hidden="true"
        className={cn("size-7 rounded-full", look.dot)}
      />
      {look.text}
    </span>
  );
}

/** A link drawn as a button, for actions that open another screen. */
export function LinkButton({
  href,
  variant = "secondary",
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
        size={15}
        className={variant === "secondary" ? "text-text-muted" : undefined}
      />
      {children}
    </HrefLink>
  );
}

/** A small ghost action, the `+ Label` and `Edit`. */
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
        "inline-flex items-center gap-6 rounded-m px-6 py-3 text-meta font-medium text-text-muted hover:bg-sunken",
        focusRing,
        className,
      )}
    >
      <Icon aria-hidden="true" size={13} />
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
      <p role="alert" className="text-meta font-medium text-danger">
        {view.text}
      </p>
    );
  }
  return (
    <ErrorBanner
      className="rounded-m border-b-0 px-12 inset-ring inset-ring-border"
      title="The change was refused."
      message={
        error instanceof ApiError && error.message !== ""
          ? error.message
          : "The daemon did not answer."
      }
    />
  );
}
