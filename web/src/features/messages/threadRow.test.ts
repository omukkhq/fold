import { describe, expect, it } from "vitest";
import { segmentsText } from "@/people/segments";
import { message, recipient, thread } from "@/test/messages";
import { rowLine, rowPreview, rowSecond } from "./threadRow";

const ask = message("m-2", {
  sender: "web-worker-39",
  recipients: [recipient("human:tanim")],
  asks_human: true,
  body: "Use null?",
});
const first = message("m-1", {
  sender: "main",
  recipients: [recipient("backend-lead"), recipient("web-lead")],
  body: "Company TIN is required.",
});

const line = (...args: Parameters<typeof rowLine>) => {
  const result = rowLine(...args);
  return { text: segmentsText(result.segments), sender: result.sender };
};

describe("rowLine", () => {
  it("names the agent whose ask waits for you", () => {
    const waiting = thread("m-1", {
      awaiting_human: true,
      messages: [first, ask],
    });
    expect(line(waiting, "tanim")).toEqual({
      text: "@web-worker-39 asks you",
      sender: "web-worker-39",
    });
  });

  it("words the first message's route when nothing asks you", () => {
    expect(line(thread("m-1", { messages: [first, ask] }), "tanim")).toEqual({
      text: "@main to @backend-lead, @web-lead",
      sender: "main",
    });
    expect(
      line(
        thread("m-1", { awaiting_human: true, messages: [first, ask] }),
        "rafeed",
      ).text,
    ).toBe("@main to @backend-lead, @web-lead");
  });

  it("calls your own first message yours and leaves out an empty route", () => {
    const own = message("m-3", {
      sender: "human:tanim",
      recipients: [recipient("frontend")],
    });
    expect(line(thread("m-3", { messages: [own] }), "tanim").text).toBe(
      "You to @frontend",
    );
    expect(
      line(
        thread("m-4", { messages: [message("m-4", { sender: "fold" })] }),
        null,
      ).text,
    ).toBe("@fold");
    expect(line(thread("m-5"), null)).toEqual({ text: "", sender: null });
  });
});

describe("rowPreview", () => {
  it("leaves out the sender the line already names", () => {
    expect(segmentsText(rowPreview(ask, "rafeed", "web-worker-39"))).toBe(
      "Use null?",
    );
  });
  it("calls your own message yours and names any other sender", () => {
    const own = message("m-6", { sender: "human:tanim", body: "Yes." });
    expect(segmentsText(rowPreview(own, "tanim", "main"))).toBe("You: Yes.");
    expect(segmentsText(rowPreview(ask, "tanim", "main"))).toBe(
      "@web-worker-39: Use null?",
    );
  });
});

describe("rowSecond", () => {
  const second = (...args: Parameters<typeof rowSecond>) => {
    const result = rowSecond(...args);
    return { text: segmentsText(result.segments), asks: result.asks };
  };
  const reply = message("m-7", { sender: "human:tanim", body: "Use null." });

  it("says who asks you while the thread waits for you", () => {
    expect(
      second(
        thread("m-1", { awaiting_human: true, messages: [first, ask] }),
        "tanim",
      ),
    ).toEqual({ text: "@web-worker-39 asks you", asks: true });
  });

  it("shows the route of a thread whose one message is its title", () => {
    expect(second(thread("m-1", { messages: [first] }), "tanim")).toEqual({
      text: "@main to @backend-lead, @web-lead",
      asks: false,
    });
    expect(second(thread("m-5"), null)).toEqual({ text: "", asks: false });
  });

  it("shows the newest message with its sender once the thread has a reply", () => {
    expect(
      second(thread("m-1", { messages: [first, ask, reply] }), "tanim"),
    ).toEqual({ text: "You: Use null.", asks: false });
    expect(second(thread("m-1", { messages: [first, ask] }), "tanim")).toEqual({
      text: "@web-worker-39: Use null?",
      asks: false,
    });
  });
});
