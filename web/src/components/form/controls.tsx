import * as RadixCheckbox from "@radix-ui/react-checkbox";
import { Check, ChevronDown } from "lucide-react";
import { forwardRef, type ComponentProps } from "react";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { checkboxClasses, isChecked } from "./checkbox";
import { Field, controlClasses, type FieldProps } from "./Field";

export type TextFieldProps = Omit<ComponentProps<"input">, "id" | "ref"> &
  FieldProps;

export const TextField = forwardRef<HTMLInputElement, TextFieldProps>(
  function TextField({ label, hint, error, className, ...rest }, ref) {
    return (
      <Field label={label} hint={hint} error={error}>
        {(control) => (
          <input
            {...rest}
            {...control}
            ref={ref}
            className={cn(controlClasses, focusRing, className)}
          />
        )}
      </Field>
    );
  },
);

export type TextareaProps = Omit<ComponentProps<"textarea">, "id" | "ref"> &
  FieldProps;

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(
  function Textarea({ label, hint, error, className, rows = 4, ...rest }, ref) {
    return (
      <Field label={label} hint={hint} error={error}>
        {(control) => (
          <textarea
            {...rest}
            {...control}
            ref={ref}
            rows={rows}
            className={cn(controlClasses, focusRing, className)}
          />
        )}
      </Field>
    );
  },
);

/** The browser's own arrow ignores the padding and sits at the edge, so the select draws none and leaves room for its chevron, which sits as far in as the text does. */
const SELECT_ARROW_ROOM = "appearance-none pr-40";

export type SelectProps = Omit<ComponentProps<"select">, "id" | "ref"> &
  FieldProps & {
    /** Keeps the label for assistive technology only, for a select whose first option says what it does. */
    labelHidden?: boolean;
  };

export const Select = forwardRef<HTMLSelectElement, SelectProps>(
  function Select(
    { label, hint, error, labelHidden, className, children, ...rest },
    ref,
  ) {
    return (
      <Field label={label} hint={hint} error={error} labelHidden={labelHidden}>
        {(control) => (
          <div className="relative">
            <select
              {...rest}
              {...control}
              ref={ref}
              className={cn(
                controlClasses,
                focusRing,
                SELECT_ARROW_ROOM,
                className,
              )}
            >
              {children}
            </select>
            <ChevronDown
              aria-hidden="true"
              size={16}
              className="pointer-events-none absolute top-1/2 right-12 -translate-y-1/2 text-text-2"
            />
          </div>
        )}
      </Field>
    );
  },
);

export type CheckboxProps = Omit<
  ComponentProps<typeof RadixCheckbox.Root>,
  "id" | "ref" | "checked" | "defaultChecked" | "onCheckedChange" | "children"
> &
  FieldProps & {
    checked?: boolean;
    defaultChecked?: boolean;
    onCheckedChange?: (checked: boolean) => void;
  };

/**
 * A Radix checkbox is a button, so react-hook-form's `register` cannot read it; bind it with `Controller`,
 * passing `field.value` as `checked` and `field.onChange` as `onCheckedChange`.
 */
export const Checkbox = forwardRef<HTMLButtonElement, CheckboxProps>(
  function Checkbox(
    { label, hint, error, className, onCheckedChange, ...rest },
    ref,
  ) {
    return (
      <Field label={label} hint={hint} error={error} inline>
        {(control) => (
          <RadixCheckbox.Root
            {...rest}
            {...control}
            ref={ref}
            onCheckedChange={(state) => onCheckedChange?.(isChecked(state))}
            className={cn(checkboxClasses, focusRing, className)}
          >
            <RadixCheckbox.Indicator className="inline-flex">
              <Check aria-hidden="true" size={12} />
            </RadixCheckbox.Indicator>
          </RadixCheckbox.Root>
        )}
      </Field>
    );
  },
);
