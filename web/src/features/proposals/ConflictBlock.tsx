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
      className="flex flex-col gap-10 rounded-m bg-danger-soft px-12 py-10"
    >
      <h3 className="text-bar leading-snug font-semibold text-danger">
        Conflict report
      </h3>
      <dl className="flex flex-col gap-2 font-mono text-meta text-text-muted">
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
            <p className="font-mono text-code font-semibold break-all text-text">
              {file.path}
            </p>
            <p
              data-kind={file.kind}
              className="text-code leading-snug text-text"
            >
              {conflictKindText(file.kind)}
            </p>
            <dl className="flex flex-col gap-2 font-mono text-caption text-text-muted">
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
                className="flex flex-col gap-4 overflow-x-auto rounded-s bg-surface px-8 py-6 font-mono text-caption"
              >
                {hunkSides(hunk, ref).map(({ label, lines }) => (
                  <div key={label} className="flex flex-col">
                    <span className="text-text-faint">{label}</span>
                    {lines.map((text, n) => (
                      <code key={n} className="whitespace-pre text-text">
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
      <p className="text-code leading-snug text-text-muted">
        <ActorText
          segments={nextStepText(proposal.summary.author, report.version)}
          slot="waitingLine"
        />
      </p>
    </section>
  );
}
