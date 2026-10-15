import { describe, expect, it } from "vitest";
import { message, thread } from "@/test/messages";
import { MAX_TITLE, threadTitle } from "./title";

const withBody = (body: string) =>
  thread("m-1", { messages: [message("m-1", { body })] });

describe("threadTitle", () => {
  it("names the thread by the first line of its first message", () => {
    expect(threadTitle(withBody("Which chart library?\nI need to know."))).toBe(
      "Which chart library?",
    );
  });

  it("skips leading blank lines", () => {
    expect(threadTitle(withBody("\n  \n  Hello  \n"))).toBe("Hello");
  });

  it("cuts a long line", () => {
    const title = threadTitle(withBody("x".repeat(MAX_TITLE + 20)));
    expect(title).toHaveLength(MAX_TITLE);
    expect(title.endsWith("…")).toBe(true);
  });

  it("falls back to the thread id when there is no text", () => {
    expect(threadTitle(withBody(""))).toBe("Thread m-1");
    expect(threadTitle(thread("m-2"))).toBe("Thread m-2");
  });
});
