import { describe, expect, it } from "vitest";
import type { RequestCode } from "@/api/requests";
import { requestSummary } from "@/test/requests";
import {
  codeHref,
  codeRequestBody,
  codeText,
  compareMarks,
  makeRequestTitle,
  requestMarkActive,
  requestMarkLabel,
  requestMarks,
  revisionAnchor,
  revisionRequestMarks,
} from "./codeLocation";

const onMain: RequestCode = {
  path: "web/src/board.ts",
  start_line: 2,
  end_line: 3,
  excerpt: "b\nc",
  excerpt_cut: false,
  revision: "r17",
  proposal: null,
  version: null,
  side: null,
};

const onChange: RequestCode = {
  ...onMain,
  start_line: 4,
  end_line: 4,
  revision: null,
  proposal: "P-3",
  version: 2,
  side: "old",
};

const selection = {
  path: "web/src/board.ts",
  side: "new" as const,
  start_line: 2,
  end_line: 3,
};

describe("codeRequestBody", () => {
  it("names a revision's lines by the revision alone", () => {
    expect(
      codeRequestBody({ kind: "revision", revision: "r17" }, selection, "Hi"),
    ).toEqual({
      body: "Hi",
      path: "web/src/board.ts",
      start_line: 2,
      end_line: 3,
      revision: "r17",
    });
  });

  it("names a version's lines by the proposal, the version and the selection's side", () => {
    expect(
      codeRequestBody(
        { kind: "version", proposal: "P-3", version: 2 },
        { ...selection, side: "old" },
        "Hi",
      ),
    ).toEqual({
      body: "Hi",
      path: "web/src/board.ts",
      start_line: 2,
      end_line: 3,
      proposal: "P-3",
      version: 2,
      side: "old",
    });
  });
});

describe("where the lines are", () => {
  it("says the lines in words", () => {
    expect(codeText(onMain)).toBe("lines 2–3 of web/src/board.ts in r17");
    expect(codeText(onChange)).toBe(
      "line 4 of web/src/board.ts in P-3 v2, old side",
    );
  });

  it("links to the file at the revision, or to the file on the version's Changes tab", () => {
    expect(codeHref("demo", onMain)).toBe(
      "/p/demo/repository?revision=r17&path=web%2Fsrc%2Fboard.ts",
    );
    expect(codeHref("demo", onChange)).toBe(
      "/p/demo/proposals/P-3?tab=changes&version=2&path=web%2Fsrc%2Fboard.ts",
    );
    expect(codeHref("demo", { ...onChange, version: null })).toBe(
      "/p/demo/proposals/P-3?tab=changes&path=web%2Fsrc%2Fboard.ts",
    );
  });

  it("titles the box by the lines and their anchor", () => {
    expect(
      makeRequestTitle({ kind: "revision", revision: "r17" }, selection),
    ).toBe("Make a request about lines 2–3 · r17");
    expect(
      makeRequestTitle(
        { kind: "version", proposal: "P-3", version: 2 },
        {
          ...selection,
          start_line: 3,
        },
      ),
    ).toBe("Make a request about line 3 · new side · v2");
  });

  it("anchors only a revision side of a comparison", () => {
    expect(revisionAnchor({ kind: "revision", id: "r4" })).toEqual({
      kind: "revision",
      revision: "r4",
    });
    expect(revisionAnchor({ kind: "workspace", id: "ws-2" })).toBeNull();
  });
});

describe("marks", () => {
  const first = requestSummary("RQ-1", { code: onMain });
  const requests = [
    first,
    requestSummary("RQ-2", { code: onChange }),
    requestSummary("RQ-3", { code: { ...onChange, side: "new" } }),
    requestSummary("RQ-4", { code: { ...onChange, version: 1 } }),
    requestSummary("RQ-5"),
  ];

  it("marks the requests of a revision on the side the screen draws it", () => {
    expect(revisionRequestMarks(requests, "r17", "new")).toEqual([
      {
        id: "RQ-1",
        range: {
          path: "web/src/board.ts",
          side: "new",
          start_line: 2,
          end_line: 3,
        },
      },
    ]);
    expect(revisionRequestMarks(requests, "r9", "new")).toEqual([]);
    expect(
      requestMarks(requests, { kind: "revision", revision: "r17" }).map(
        (m) => m.id,
      ),
    ).toEqual(["RQ-1"]);
  });

  it("marks a version's requests on the sides whose lines are the version's own", () => {
    const anchor = { kind: "version" as const, proposal: "P-3", version: 2 };
    expect(requestMarks(requests, anchor)).toEqual([
      {
        id: "RQ-2",
        range: {
          path: "web/src/board.ts",
          side: "old",
          start_line: 4,
          end_line: 4,
        },
      },
      {
        id: "RQ-3",
        range: {
          path: "web/src/board.ts",
          side: "new",
          start_line: 4,
          end_line: 4,
        },
      },
    ]);
    expect(requestMarks(requests, anchor, ["new"]).map((m) => m.id)).toEqual([
      "RQ-3",
    ]);
  });

  it("marks each revision of a comparison on its own side", () => {
    const older = requestSummary("RQ-6", {
      code: { ...onMain, revision: "r16" },
    });
    const marks = compareMarks([...requests, older], {
      from: { kind: "revision", id: "r16" },
      to: { kind: "revision", id: "r17" },
    });
    expect(marks.map((m) => [m.id, m.range.side])).toEqual([
      ["RQ-6", "old"],
      ["RQ-1", "new"],
    ]);
    expect(
      compareMarks(requests, {
        from: { kind: "revision", id: "r17" },
        to: { kind: "workspace", id: "ws-1" },
      }).map((m) => [m.id, m.range.side]),
    ).toEqual([["RQ-1", "old"]]);
  });

  it("names a marker and quiets it once the request is final", () => {
    expect(requestMarkLabel(first)).toBe("Request RQ-1 about lines 2–3, open");
    const named = requestSummary("RQ-7", {
      code: onChange,
      title: "Rename b",
      state: "draft_ready",
    });
    expect(requestMarkLabel(named)).toBe(
      "Request RQ-7 about line 4: Rename b, draft ready",
    );
    expect(requestMarkLabel(requestSummary("RQ-8"))).toBe("Request RQ-8, open");
    expect(requestMarkActive(named)).toBe(true);
    expect(requestMarkActive({ ...named, state: "approved" })).toBe(false);
  });
});
