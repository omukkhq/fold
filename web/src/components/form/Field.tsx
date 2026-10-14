import { useId, type ReactNode } from "react";
import { describedBy, fieldIds } from "./describedBy";

export interface FieldProps {
  label: string;
  hint?: string;
  error?: string;
}

export interface ControlProps {
  id: string;
  "aria-describedby": string | undefined;
  "aria-invalid": true | undefined;
}

/** The label, hint and error around one control; the control gets the ids that link them. */
export function Field({
  label,
  hint,
  error,
  inline,
  children,
}: FieldProps & {
  /** Puts the control before its label, as a checkbox needs. */
  inline?: boolean;
  children: (control: ControlProps) => ReactNode;
}) {
  const ids = fieldIds(useId());
  const labelNode = (
    <label
      htmlFor={ids.input}
      className={
        inline
          ? "text-body leading-snug text-text"
          : "text-bar leading-snug font-semibold text-text"
      }
    >
      {label}
    </label>
  );
  const control = children({
    id: ids.input,
    "aria-describedby": describedBy(ids, {
      hint: Boolean(hint),
      error: Boolean(error),
    }),
    "aria-invalid": error ? true : undefined,
  });
  return (
    <div className="flex flex-col gap-6">
      {inline ? (
        <div className="flex items-center gap-10">
          {control}
          {labelNode}
        </div>
      ) : (
        <>
          {labelNode}
          {control}
        </>
      )}
      {hint ? (
        <p id={ids.hint} className="text-meta text-text-muted">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={ids.error} className="text-meta font-medium text-danger">
          {error}
        </p>
      ) : null}
    </div>
  );
}

export const controlClasses =
  "w-full rounded-m bg-surface px-12 py-10 text-body leading-snug text-text inset-ring inset-ring-border placeholder:text-text-faint aria-[invalid=true]:inset-ring-danger disabled:cursor-not-allowed disabled:opacity-60";
