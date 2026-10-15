import { Fragment } from "react";
import { PersonName } from "./PersonName";
import type { Segments } from "./segments";

/** A line that names people, each actor drawn as `@name`; `personClassName` sizes the names. */
export function ActorText({
  segments,
  personClassName,
}: {
  segments: Segments;
  personClassName?: string;
}) {
  return (
    <>
      {segments.map((part, index) => (
        <Fragment key={index}>
          {typeof part === "string" ? (
            part
          ) : (
            <PersonName name={part.actor} className={personClassName} />
          )}
        </Fragment>
      ))}
    </>
  );
}
