import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import { message, thread } from "@/test/messages";
import { isMissingThread, openThread } from "./openThread";

const a = thread("m-1", { messages: [message("m-1")] });
const b = thread("m-2", { messages: [message("m-2"), message("m-5")] });

describe("openThread", () => {
  it("opens the first visible thread when the address names none", () => {
    expect(openThread([a, b], [a, b], undefined)).toEqual({
      kind: "listed",
      thread: a,
    });
    expect(openThread([], [], undefined)).toEqual({
      kind: "listed",
      thread: null,
    });
  });

  it("opens a listed thread, also by the id of one of its messages", () => {
    expect(openThread([a, b], [a, b], "m-2").kind).toBe("listed");
    expect(openThread([a, b], [a, b], "m-5")).toEqual({
      kind: "listed",
      thread: b,
    });
  });

  it("falls back to the first visible thread when the filters hide the requested one", () => {
    expect(openThread([a, b], [a], "m-2")).toEqual({
      kind: "listed",
      thread: a,
    });
  });

  it("reads a requested thread the listing does not hold on its own", () => {
    expect(openThread([a, b], [a, b], "m-900")).toEqual({
      kind: "unlisted",
      id: "m-900",
    });
  });
});

describe("isMissingThread", () => {
  it("reads an unknown and a malformed message id as missing, and nothing else", () => {
    expect(isMissingThread(new ApiError("message_not_found", ""))).toBe(true);
    expect(isMissingThread(new ApiError("invalid_id", ""))).toBe(true);
    expect(isMissingThread(new ApiError("project_unavailable", ""))).toBe(
      false,
    );
    expect(isMissingThread(null)).toBe(false);
  });
});
