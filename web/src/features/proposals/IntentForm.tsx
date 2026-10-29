import { Plus, Trash2 } from "lucide-react";
import { Button, IconButton } from "@/components/button/Button";
import { TextField, Textarea } from "@/components/form/controls";
import {
  SECTIONS,
  addPart,
  changePart,
  changeSection,
  removePart,
  type IntentDraft,
} from "./intentDraft";
import { MAX_PARTS } from "./intent";

const TITLE = "font-title text-14 leading-tight font-semibold text-text-2";

export interface IntentFormProps {
  /** The version the edit makes, such as `v3`. */
  next: string;
  draft: IntentDraft;
  onChange: (draft: IntentDraft) => void;
  hint: string;
  /** Why Save cannot send the intent; shown once the human changed it. */
  problem: string | null;
  readOnly: boolean;
}

/** The intent of the version an edit makes: the summary, the parts with their files one path per line, and the lists that apply, one entry per line. */
export function IntentForm({
  next,
  draft,
  onChange,
  hint,
  problem,
  readOnly,
}: IntentFormProps) {
  return (
    <section
      aria-label={`Intent of ${next}`}
      className="flex min-w-0 flex-col gap-12 rounded-m bg-sunken p-14"
    >
      <h3 className="text-16 leading-tight font-semibold text-text">
        Intent of {next}
      </h3>
      <p className="max-w-measure text-13 leading-normal text-text-2">{hint}</p>
      <Textarea
        label="Summary"
        hint="How the change works in the code, in 2 to 4 sentences."
        rows={3}
        readOnly={readOnly}
        value={draft.summary}
        onChange={(event) => {
          onChange({ ...draft, summary: event.target.value });
        }}
        className="resize-y"
      />
      <h4 className={TITLE}>Parts</h4>
      <ol className="flex flex-col gap-12">
        {draft.parts.map((part, index) => {
          const which = `Part ${String(index + 1)}`;
          return (
            // A part's name changes while the human types, so its place keeps the fields apart.
            <li
              key={index}
              aria-label={which}
              className="flex flex-col gap-8 rounded-s bg-surface p-12"
            >
              <div className="flex items-end gap-8">
                <div className="min-w-0 flex-1">
                  <TextField
                    label={`${which} name`}
                    readOnly={readOnly}
                    value={part.name}
                    onChange={(event) => {
                      onChange(
                        changePart(draft, index, { name: event.target.value }),
                      );
                    }}
                  />
                </div>
                {draft.parts.length > 1 ? (
                  <IconButton
                    aria-label={`Remove ${which.toLowerCase()}`}
                    disabled={readOnly}
                    onClick={() => {
                      onChange(removePart(draft, index));
                    }}
                  >
                    <Trash2 aria-hidden="true" size={16} />
                  </IconButton>
                ) : null}
              </div>
              <Textarea
                label={`What ${which.toLowerCase()} does`}
                rows={2}
                readOnly={readOnly}
                value={part.does}
                onChange={(event) => {
                  onChange(
                    changePart(draft, index, { does: event.target.value }),
                  );
                }}
                className="resize-y"
              />
              <Textarea
                label={`Files of ${which.toLowerCase()}`}
                hint="One path per line."
                rows={2}
                spellCheck={false}
                readOnly={readOnly}
                value={part.files}
                onChange={(event) => {
                  onChange(
                    changePart(draft, index, { files: event.target.value }),
                  );
                }}
                className="resize-y font-mono text-13"
              />
            </li>
          );
        })}
      </ol>
      {draft.parts.length < MAX_PARTS ? (
        <Button
          size="sm"
          className="self-start"
          disabled={readOnly}
          icon={<Plus aria-hidden="true" size={14} />}
          onClick={() => {
            onChange(addPart(draft));
          }}
        >
          Add a part
        </Button>
      ) : null}
      <h4 className={TITLE}>Only when they apply</h4>
      <div className="grid grid-cols-1 gap-10 md:grid-cols-2">
        {SECTIONS.map(({ key, label }) => (
          <Textarea
            key={key}
            label={label}
            hint="One entry per line."
            rows={2}
            readOnly={readOnly}
            value={draft.sections[key]}
            onChange={(event) => {
              onChange(changeSection(draft, key, event.target.value));
            }}
            className="resize-y"
          />
        ))}
      </div>
      {problem === null ? null : (
        <p role="alert" className="text-13 font-medium text-bad">
          {problem}
        </p>
      )}
    </section>
  );
}
