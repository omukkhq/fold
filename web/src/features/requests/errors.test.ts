import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { describeFailure, draftReasonText, reloadsRequest } from "./errors";

const err = (
  code: string,
  details: Record<string, unknown> = {},
  message = "",
) => new ApiError(code, message, details);

describe("draftReasonText", () => {
  it.each([
    ["empty", {}, "A draft needs at least one card."],
    ["too_many_cards", { cards: 21 }, "A draft holds at most 20 cards."],
    [
      "invalid_key",
      { position: 2 },
      "Card 2 has a key fold does not accept. Use 1 to 32 letters, digits, - or _.",
    ],
    ["duplicate_key", { key: "c1" }, "Two cards use the key c1."],
    ["empty_title", { key: "c1" }, "Card c1 needs a title."],
    ["title_not_one_line", { key: "c1" }, "The title of c1 must be one line."],
    [
      "title_too_long",
      { key: "c1" },
      "The title of c1 is longer than 200 characters.",
    ],
    ["empty_description", { key: "c2" }, "Card c2 needs a description."],
    [
      "description_too_long",
      { key: "c2" },
      "The description of c2 is longer than 20000 characters.",
    ],
    [
      "unknown_key",
      { position: 3 },
      "Edge 3 names a card that is not in the draft.",
    ],
    ["self_edge", { key: "c1" }, "c1 cannot need itself."],
    [
      "duplicate_edge",
      { edge: { dependent: "c2", prerequisite: "c1" } },
      "c2 already needs c1.",
    ],
    ["duplicate_edge", {}, "Two edges are the same."],
    [
      "cycle",
      { path: ["c1", "c2", "c1"] },
      "These edges make a cycle: c1 → c2 → c1. Remove one of them.",
    ],
    ["cycle", {}, "The edges make a cycle. Remove one of them."],
    ["new_reason", {}, "fold refused the draft as written."],
    ["empty_title", {}, "Card a card needs a title."],
  ])("explains %s", (reason, details, text) => {
    expect(draftReasonText(reason, details)).toBe(text);
  });
});

describe("describeFailure", () => {
  it("branches on the code and keeps it", () => {
    expect(describeFailure(err("requester_required"))).toEqual({
      text: "Only the requester can do this.",
      code: "requester_required",
    });
    expect(
      describeFailure(err("stale_version", { expected: 1, latest: 2 })).text,
    ).toBe("v2 is now the latest version. Read it, then try once more.");
    expect(
      describeFailure(
        err("invalid_draft", { reason: "empty_title", key: "c3" }),
      ).text,
    ).toBe("Card c3 needs a title.");
    expect(
      describeFailure(err("dependency_cycle", { path: ["c1", "c2", "c1"] }))
        .text,
    ).toMatch(/c1 → c2 → c1/);
    expect(describeFailure(err("request_too_large")).text).toMatch(
      /Remove a file/,
    );
  });

  it("explains the attachment and state reasons of invalid_request", () => {
    const text = (details: Record<string, unknown>) =>
      describeFailure(err("invalid_request", details)).text;
    expect(text({ reason: "attachment_too_large", position: 2 })).toBe(
      "File 2 is larger than 10 MiB.",
    );
    expect(text({ reason: "too_many_attachments", position: 11 })).toBe(
      "A request holds at most 10 files.",
    );
    expect(text({ reason: "invalid_attachment_name", position: 1 })).toMatch(
      /name/,
    );
    expect(text({ reason: "invalid_media_type", position: 1 })).toMatch(/type/);
    expect(text({ reason: "state" })).toMatch(/moved on/);
    expect(text({})).toBe("fold refused the request as written.");
  });

  it("shows the daemon's message for a code without a sentence", () => {
    expect(
      describeFailure(err("brand_new", {}, "the daemon says why")),
    ).toEqual({
      text: "the daemon says why",
      code: "brand_new",
    });
    expect(describeFailure(err("brand_new")).text).toBe(
      "The daemon refused the request.",
    );
    expect(describeFailure(new Error("offline"))).toEqual({
      text: "The request did not reach the daemon. Try again.",
      code: null,
    });
    expect(
      describeFailure({
        error: { code: "auth_required", message: "x", details: {} },
      }),
    ).toEqual({
      text: "Sign in again, then try once more.",
      code: "auth_required",
    });
  });
});

describe("reloadsRequest", () => {
  it("reads the request again after a refusal that says it moved on", () => {
    expect(reloadsRequest(err("stale_version"))).toBe(true);
    expect(reloadsRequest(err("request_not_open"))).toBe(true);
    expect(reloadsRequest(err("invalid_request", { reason: "state" }))).toBe(
      true,
    );
    expect(reloadsRequest(err("invalid_request"))).toBe(false);
    expect(reloadsRequest(err("requester_required"))).toBe(false);
    expect(reloadsRequest(new Error("x"))).toBe(false);
  });
});

describe("describeFailure for a request on code", () => {
  it.each([
    [
      "unknown_comment_path",
      "This file is not a text file of that revision or version, so it takes no request on its lines.",
    ],
    [
      "invalid_comment_range",
      "These lines are not in the file now. Select the lines again.",
    ],
    ["revision_not_found", "fold has no such revision."],
    ["proposal_not_found", "fold has no such proposal or version."],
    ["invalid_path", "fold does not accept this file path."],
  ])("names %s in words", (code, text) => {
    expect(describeFailure(err(code))).toEqual({ text, code });
  });

  it.each([
    [
      "missing",
      "That revision or version has no file at this path, so its lines take no request.",
    ],
    ["binary", "This file is binary, so its lines take no request."],
    [
      "too_large",
      "This file is too large for fold to read, so its lines take no request.",
    ],
    [
      "not_a_file",
      "This path is not a file in that revision or version, so it takes no request.",
    ],
    [
      "other",
      "This file is not a text file of that revision or version, so it takes no request on its lines.",
    ],
  ])("says why a path takes no request: %s", (reason, text) => {
    expect(describeFailure(err("unknown_comment_path", { reason })).text).toBe(
      text,
    );
  });
});
