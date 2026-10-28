import { ArrowDown, ArrowUp, Plus, Trash2, X } from "lucide-react";
import { Button, IconButton } from "@/components/button/Button";
import { controlClasses } from "@/components/form/Field";
import { cn } from "@/lib/cn";
import { focusRing } from "@/lib/focus";
import { MentionTextarea } from "@/mentions/MentionTextarea";
import {
  addCard,
  addEdge,
  freeEdge,
  moveCard,
  removeCard,
  removeEdge,
  updateCard,
  type DraftForm,
} from "./draft";
import { MAX_CARDS } from "./errors";
import { edgesSummary } from "./present";

interface DraftEditorProps {
  slug: string;
  form: DraftForm;
  /** The version the save writes. */
  next: number;
  /** The save is in flight: the cards and edges stay locked. */
  locked: boolean;
  onChange: (form: DraftForm) => void;
}

const SELECT = cn(
  controlClasses,
  focusRing,
  "w-auto py-5 text-13 tabular-nums",
);

export function DraftEditor({
  slug,
  form,
  next,
  locked,
  onChange,
}: DraftEditorProps) {
  const free = freeEdge(form);
  return (
    <>
      <section aria-label="Draft" className="flex flex-col gap-10">
        <div className="flex items-center gap-10">
          <h2 className="text-16 leading-tight font-semibold text-text">
            Draft
          </h2>
          <p className="text-13 text-text-2">
            Editing · saves as version {next}
          </p>
        </div>
        <ol className="flex flex-col gap-10">
          {form.cards.map((card, index) => (
            <li
              key={card.key}
              className="flex flex-col gap-8 rounded-m bg-surface px-14 py-12"
            >
              <div className="flex items-center gap-10">
                <span className="rounded-s bg-fill px-7 py-2 text-13 font-semibold text-text-2 tabular-nums">
                  {card.key}
                </span>
                <input
                  aria-label={`Title of ${card.key}`}
                  value={card.title}
                  readOnly={locked}
                  onChange={(event) => {
                    onChange(
                      updateCard(form, card.key, { title: event.target.value }),
                    );
                  }}
                  className={cn(controlClasses, focusRing, "font-semibold")}
                />
                <IconButton
                  aria-label={`Move ${card.key} up`}
                  disabled={locked || index === 0}
                  onClick={() => {
                    onChange(moveCard(form, card.key, -1));
                  }}
                >
                  <ArrowUp aria-hidden="true" size={15} />
                </IconButton>
                <IconButton
                  aria-label={`Move ${card.key} down`}
                  disabled={locked || index === form.cards.length - 1}
                  onClick={() => {
                    onChange(moveCard(form, card.key, 1));
                  }}
                >
                  <ArrowDown aria-hidden="true" size={15} />
                </IconButton>
                <IconButton
                  aria-label={`Remove ${card.key}`}
                  disabled={locked}
                  className="text-bad"
                  onClick={() => {
                    onChange(removeCard(form, card.key));
                  }}
                >
                  <Trash2 aria-hidden="true" size={15} />
                </IconButton>
              </div>
              <MentionTextarea
                slug={slug}
                aria-label={`Description of ${card.key}`}
                rows={5}
                value={card.description}
                readOnly={locked}
                onValueChange={(description) => {
                  onChange(updateCard(form, card.key, { description }));
                }}
                className="text-14"
              />
              <p className="text-13 text-text-2">
                Markdown in the task template shape: Scope, then Acceptance.
              </p>
            </li>
          ))}
        </ol>
        <div>
          <Button
            icon={<Plus aria-hidden="true" size={15} />}
            disabled={locked || form.cards.length >= MAX_CARDS}
            onClick={() => {
              onChange(addCard(form));
            }}
          >
            Add card · {form.cards.length} of {MAX_CARDS}
          </Button>
        </div>
      </section>
      <section aria-label="Dependencies" className="flex flex-col gap-10">
        <div className="flex items-center gap-10">
          <h2 className="text-16 leading-tight font-semibold text-text">
            Dependencies
          </h2>
          <p className="text-13 text-text-2">
            {edgesSummary(form.edges.length)} · each row reads “dependent needs
            prerequisite”
          </p>
        </div>
        <ul className="flex flex-col gap-8">
          {form.edges.map((edge, index) => (
            <li key={index} className="flex items-center gap-8">
              <select
                aria-label={`Edge ${String(index + 1)} dependent`}
                value={edge.dependent}
                disabled={locked}
                className={SELECT}
                onChange={(event) => {
                  const edges = form.edges.map((e, i) =>
                    i === index ? { ...e, dependent: event.target.value } : e,
                  );
                  onChange({ ...form, edges });
                }}
              >
                {form.cards.map((c) => (
                  <option key={c.key} value={c.key}>
                    {c.key}
                  </option>
                ))}
              </select>
              <span className="text-13 text-text-2">needs</span>
              <select
                aria-label={`Edge ${String(index + 1)} prerequisite`}
                value={edge.prerequisite}
                disabled={locked}
                className={SELECT}
                onChange={(event) => {
                  const edges = form.edges.map((e, i) =>
                    i === index
                      ? { ...e, prerequisite: event.target.value }
                      : e,
                  );
                  onChange({ ...form, edges });
                }}
              >
                {form.cards.map((c) => (
                  <option key={c.key} value={c.key}>
                    {c.key}
                  </option>
                ))}
              </select>
              <IconButton
                aria-label={`Remove edge ${String(index + 1)}`}
                disabled={locked}
                className="ml-auto"
                onClick={() => {
                  onChange(removeEdge(form, index));
                }}
              >
                <X aria-hidden="true" size={15} />
              </IconButton>
            </li>
          ))}
        </ul>
        <div>
          <Button
            icon={<Plus aria-hidden="true" size={15} />}
            disabled={locked || free === null}
            onClick={() => {
              if (free) onChange(addEdge(form, free));
            }}
          >
            Add edge
          </Button>
        </div>
      </section>
    </>
  );
}
