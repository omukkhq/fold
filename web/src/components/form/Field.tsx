import { createContext, use, useId, type ReactNode } from "react";
import { describedBy, fieldIds } from "./describedBy";

export interface FieldProps {
  label: string;
  hint?: string;
  error?: string;
}

/** The hint under a control: a 13px note by default; a form whose hints are whole sentences gives them 14px within the measure. */
export const FieldHintClass = createContext("text-13 text-text-2");

/** The hint style of a form whose hints are sentences that explain a choice. */
export const SENTENCE_HINT = "max-w-measure text-14 leading-normal text-text-2";

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
  const hintClass = use(FieldHintClass);
  const labelNode = (
    <label
      htmlFor={ids.input}
      className={
        inline
          ? "text-15 leading-tight text-text"
          : "font-title text-13 leading-tight font-semibold text-text-2"
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
        <p id={ids.hint} className={hintClass}>
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={ids.error} className="text-13 font-medium text-bad">
          {error}
        </p>
      ) : null}
    </div>
  );
}

export const controlClasses =
  "w-full rounded-s bg-fill px-12 py-9 text-15 leading-tight text-text placeholder:text-text-2 aria-[invalid=true]:inset-ring aria-[invalid=true]:inset-ring-bad disabled:cursor-not-allowed disabled:opacity-60";
