import { forwardRef } from "react";
import { Field, type FieldProps } from "@/components/form/Field";
import { MentionTextarea, type MentionTextareaProps } from "./MentionTextarea";

/** A labelled mention textarea: the label, hint and error of the other form fields around the suggesting control. */
export const MentionField = forwardRef<
  HTMLTextAreaElement,
  Omit<MentionTextareaProps, "id"> & FieldProps
>(function MentionField({ label, hint, error, ...rest }, ref) {
  return (
    <Field label={label} hint={hint} error={error}>
      {(control) => <MentionTextarea {...rest} {...control} ref={ref} />}
    </Field>
  );
});
