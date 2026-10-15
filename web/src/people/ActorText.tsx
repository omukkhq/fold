import { Fragment } from "react";
import { PersonName } from "./PersonName";
import type { Segments } from "./segments";
import type { PersonSlot } from "./sizes";

/** A line that names people, each actor drawn as `@name`; `personClassName` or `slot` sizes the names. */
export function ActorText({
  segments,
  personClassName,
  slot,
}: {
  segments: Segments;
  personClassName?: string;
  slot?: PersonSlot;
}) {
  return (
    <>
      {segments.map((part, index) => (
        <Fragment key={index}>
          {typeof part === "string" ? (
            part
          ) : (
            <PersonName
              name={part.actor}
              className={personClassName}
              slot={slot}
            />
          )}
        </Fragment>
      ))}
    </>
  );
}
