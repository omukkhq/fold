import { Avatar } from "@/components/avatar/Avatar";
import { cn } from "@/lib/cn";
import { bareName, isHumanName } from "./names";

/**
 * An actor's name. A human always shows in Barlow in the link-text colour with no mark, so its classes come after `className`; an agent takes `className` whole and keeps its worker chip unless `chip` is false.
 */
export function PersonName({
  name,
  className,
  avatarClassName,
  chip = true,
}: {
  /** `human:<name>` or an agent or worker name. */
  name: string;
  className?: string;
  avatarClassName?: string;
  chip?: boolean;
}) {
  if (isHumanName(name)) {
    return (
      <span
        data-person="human"
        className={cn(className, "font-ui text-primary-text")}
      >
        {bareName(name)}
      </span>
    );
  }
  if (!chip) {
    return (
      <span data-person="agent" className={className}>
        {name}
      </span>
    );
  }
  return (
    <span
      data-person="agent"
      className={cn("inline-flex items-center gap-6", className)}
    >
      <Avatar name={name} className={avatarClassName} />
      {name}
    </span>
  );
}

/** Names in a row, comma separated, each with its own colour. */
export function PersonList({
  names,
  className,
}: {
  names: readonly string[];
  className?: string;
}) {
  return (
    <span className={className}>
      {names.map((name, index) => (
        <span key={name}>
          {index > 0 ? ", " : ""}
          <PersonName name={name} chip={false} />
        </span>
      ))}
    </span>
  );
}
