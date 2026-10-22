import { cn } from "@/lib/cn";
import { bareName, isHumanName, isYou } from "./names";
import { type PersonSlot, slotSize } from "./sizes";

/**
 * A person as `@name` text: a human in the link-text colour, an agent or worker in the secondary colour, always Barlow 500.
 * It takes the size of the text around it; where a name has its own size, the caller names the `slot` and the size follows the person's kind, a human and an agent each at the size the slot gives them. The font, weight and colour come after `className`, so a caller's mono or muted classes never change them.
 */
export function PersonName({
  name,
  className,
  slot,
  you = null,
  youWord = false,
}: {
  /** `human:<name>` or an agent or worker name. */
  name: string;
  className?: string;
  slot?: PersonSlot;
  /** The signed-in human's bare name: their own name gets `(you)` after it. */
  you?: string | null;
  /** The signed-in human reads as the word `you` instead of `@name (you)`, inside a sentence such as `Read by you`. */
  youWord?: boolean;
}) {
  const human = isHumanName(name);
  const own = isYou(name, you);
  return (
    <span
      data-person={human ? "human" : "agent"}
      className={cn(
        className,
        slot && slotSize(slot, human ? "human" : "agent"),
        "font-ui font-medium",
        human ? "text-primary-text" : "text-secondary-text",
      )}
    >
      {own && youWord ? "you" : `@${bareName(name)}`}
      {own && !youWord ? (
        <span className="font-regular text-text-muted"> (you)</span>
      ) : null}
    </span>
  );
}

/** People in a row, `@a, @b`; `nameClassName` or `slot` sizes each name. */
export function PersonList({
  names,
  className,
  nameClassName,
  slot,
  you,
  youWord = false,
}: {
  names: readonly string[];
  className?: string;
  nameClassName?: string;
  slot?: PersonSlot;
  you?: string | null;
  youWord?: boolean;
}) {
  return (
    <span className={className}>
      {names.map((name, index) => (
        <span key={name}>
          {index > 0 ? ", " : ""}
          <PersonName
            name={name}
            className={nameClassName}
            slot={slot}
            you={you}
            youWord={youWord}
          />
        </span>
      ))}
    </span>
  );
}
