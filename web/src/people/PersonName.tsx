import { cn } from "@/lib/cn";
import { bareName, isHumanName } from "./names";

/**
 * A person as `@name` text: a human in the link-text colour, an agent or worker in the secondary colour, always Barlow 500.
 * It takes the size of the text around it; where it stands for a value, the caller sets one type step larger through `className`. The font, weight and colour come after `className`, so a caller's mono or muted classes never change them.
 */
export function PersonName({
  name,
  className,
}: {
  /** `human:<name>` or an agent or worker name. */
  name: string;
  className?: string;
}) {
  const human = isHumanName(name);
  return (
    <span
      data-person={human ? "human" : "agent"}
      className={cn(
        className,
        "font-ui font-medium",
        human ? "text-primary-text" : "text-secondary-text",
      )}
    >
      @{bareName(name)}
    </span>
  );
}

/** People in a row, `@a, @b`; `nameClassName` sizes each name. */
export function PersonList({
  names,
  className,
  nameClassName,
}: {
  names: readonly string[];
  className?: string;
  nameClassName?: string;
}) {
  return (
    <span className={className}>
      {names.map((name, index) => (
        <span key={name}>
          {index > 0 ? ", " : ""}
          <PersonName name={name} className={nameClassName} />
        </span>
      ))}
    </span>
  );
}
