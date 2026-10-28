import { CODE_BOX, textFlow } from "@/components/text/textFlow";
import { cn } from "@/lib/cn";
import { ActorText } from "@/people/ActorText";
import type { ConflictReport, ProposalDetail } from "@/api/proposals";
import {
  conflictKindText,
  fileSides,
  hunkSides,
  nextStepText,
  reportTrees,
} from "./conflict";
import { versionRef } from "./present";

/** What the last integration of the proposal ran into: each conflicting file with its sides and hunks, and who acts next. */
export function ConflictBlock({
  report,
  proposal,
}: {
  report: ConflictReport;
  proposal: ProposalDetail;
}) {
  const ref = versionRef(proposal.summary.id, report.version);
  return (
    <section
      aria-label="Conflict report"
      className="flex flex-col gap-10 rounded-m bg-bad-soft px-12 py-10"
    >
      <h3 className="text-14 leading-tight font-semibold text-bad">
        Conflict report
      </h3>
      <dl className="flex flex-col gap-2 font-mono text-13 text-text-2">
        {reportTrees(report, ref).map((side) => (
          <div key={side.label} className="flex gap-6">
            <dt className="shrink-0">{side.label}</dt>
            <dd className="min-w-0 truncate">{side.entry}</dd>
          </div>
        ))}
      </dl>
      <ul className="flex flex-col gap-12">
        {report.files.map((file) => (
          <li key={file.path} className="flex flex-col gap-4">
            <p className="font-mono text-13 font-semibold break-all text-text">
              {file.path}
            </p>
            <p
              data-kind={file.kind}
              className="text-13 leading-tight text-text"
            >
              {conflictKindText(file.kind)}
            </p>
            <dl className="flex flex-col gap-2 font-mono text-13 text-text-2">
              {fileSides(file, ref, report).map((side) => (
                <div key={side.label} className="flex gap-6">
                  <dt className="shrink-0">{side.label}</dt>
                  <dd className="min-w-0 truncate">{side.entry}</dd>
                </div>
              ))}
            </dl>
            {file.hunks.map((hunk, index) => (
              <div
                key={index}
                className={cn(
                  CODE_BOX,
                  "flex flex-col gap-4 rounded-s bg-surface px-8 py-6 font-mono text-13",
                )}
              >
                {hunkSides(hunk, ref).map(({ label, lines }) => (
                  <div key={label} className="flex flex-col">
                    <span className="text-text-2">{label}</span>
                    {lines.map((text, n) => (
                      <code
                        key={n}
                        className={cn("text-text", textFlow("code"))}
                      >
                        {text}
                      </code>
                    ))}
                  </div>
                ))}
              </div>
            ))}
          </li>
        ))}
      </ul>
      <p className="text-13 leading-tight text-text-2">
        <ActorText
          segments={nextStepText(proposal.summary.author, report.version)}
          slot="waitingLine"
        />
      </p>
    </section>
  );
}
