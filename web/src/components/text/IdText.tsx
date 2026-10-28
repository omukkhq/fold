import { Fragment } from "react";
import { idRuns } from "@/format/ids";

/** A sentence whose public ids never break over two lines; the words between them wrap as usual. */
export function IdText({ text }: { text: string }) {
  return (
    <>
      {idRuns(text).map((run, index) => (
        <Fragment key={index}>
          {run.id ? (
            <span className="whitespace-nowrap tabular-nums">{run.text}</span>
          ) : (
            run.text
          )}
        </Fragment>
      ))}
    </>
  );
}
