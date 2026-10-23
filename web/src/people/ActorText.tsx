import { Fragment } from "react";
import { PersonName } from "./PersonName";
import type { Segments } from "./segments";
import type { PersonSlot } from "./sizes";

/** A line that names people, each actor drawn as `@name`; `personClassName` or `slot` sizes the names. */
export function ActorText({
  segments,
  personClassName,
  slot,
  you = null,
}: {
  segments: Segments;
  personClassName?: string;
  slot?: PersonSlot;
  /** The signed-in human's bare name: their own name reads as the word `you`. */
  you?: string | null;
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
              you={you}
              youWord
            />
          )}
        </Fragment>
      ))}
    </>
  );
}
