import { describe, expect, it } from "vitest";
import { segmentsText } from "@/people/segments";
import { message, recipient } from "@/test/messages";
import { initial, messageSide, recipientsPhrase } from "./messageLayout";

describe("messageSide", () => {
  it("puts your own message on the right and marks other senders by kind", () => {
    expect(messageSide("human:tanim", "tanim")).toBe("own");
    expect(messageSide("human:rafeed", "tanim")).toBe("human");
    expect(messageSide("human:tanim", null)).toBe("human");
    expect(messageSide("web-worker-39", "tanim")).toBe("agent");
  });
});

describe("initial", () => {
  it("takes the first letter of the bare name", () => {
    expect(initial("human:rafeed")).toBe("R");
    expect(initial("main")).toBe("M");
  });
});

describe("recipientsPhrase", () => {
  it("names you first, then the others in order", () => {
    const sent = message("m-1", {
      recipients: [
        recipient("human:rafeed"),
        recipient("human:tanim"),
        recipient("web-lead"),
      ],
    });
    expect(segmentsText(recipientsPhrase(sent, "tanim"))).toBe(
      "to @tanim, @rafeed and @web-lead",
    );
    expect(segmentsText(recipientsPhrase(sent, null))).toBe(
      "to @rafeed, @tanim and @web-lead",
    );
  });
  it("is empty for a message without recipients", () => {
    expect(recipientsPhrase(message("m-2"), "tanim")).toEqual([]);
  });
});
