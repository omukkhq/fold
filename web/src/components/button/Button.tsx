import { Loader2 } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import {
  buttonClasses,
  buttonIconClass,
  iconButtonClasses,
  controlLabel,
  iconPixels,
  isInert,
  type PendingProps,
  type ButtonSize,
  type ButtonVariant,
} from "./buttonStyles";

interface StyleProps {
  variant?: ButtonVariant;
  size?: ButtonSize;
}

type CommonProps = StyleProps & PendingProps;

function Spinner({ size }: { size: ButtonSize }) {
  return (
    <Loader2
      aria-hidden="true"
      size={iconPixels(size)}
      className="animate-spin motion-reduce:animate-none"
    />
  );
}

type NativeProps = Omit<ComponentProps<"button">, "children">;

export type ButtonProps = NativeProps &
  CommonProps & {
    children: ReactNode;
    /** An icon shown before the label; the spinner replaces it while pending. */
    icon?: ReactNode;
  };

export function Button({
  variant = "quiet",
  size = "md",
  pending,
  pendingLabel,
  icon,
  disabled,
  className,
  children,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      {...rest}
      type={type}
      disabled={isInert(disabled, pending)}
      aria-busy={pending ? true : undefined}
      className={cn(buttonClasses(variant, size), focusRing, className)}
    >
      {pending || icon ? (
        <span className={buttonIconClass(variant)}>
          {pending ? <Spinner size={size} /> : icon}
        </span>
      ) : null}
      {controlLabel(children, pending, pendingLabel)}
    </button>
  );
}

export type IconButtonProps = NativeProps &
  CommonProps & {
    /** The accessible name; an icon alone names nothing. */
    "aria-label": string;
    children: ReactNode;
  };

export function IconButton({
  variant = "ghost",
  size = "md",
  pending,
  pendingLabel,
  disabled,
  className,
  children,
  type = "button",
  "aria-label": ariaLabel,
  ...rest
}: IconButtonProps) {
  return (
    <button
      {...rest}
      type={type}
      aria-label={controlLabel(ariaLabel, pending, pendingLabel)}
      disabled={isInert(disabled, pending)}
      aria-busy={pending ? true : undefined}
      className={cn(iconButtonClasses(variant, size), focusRing, className)}
    >
      {pending ? <Spinner size={size} /> : children}
    </button>
  );
}
