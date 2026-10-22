import { describe, expect, it } from "vitest";
import { linkSecret, signInView, withoutFragment } from "./link";

const SECRET = "Ab-_09".repeat(7);

describe("linkSecret", () => {
  it("reads the secret from the fragment, with or without its #", () => {
    expect(linkSecret(`#link=${SECRET}`)).toBe(SECRET);
    expect(linkSecret(`link=${SECRET}`)).toBe(SECRET);
    expect(linkSecret(`other=1&link=${SECRET}`)).toBe(SECRET);
  });

  it.each([
    "",
    "#",
    "#link=",
    "#token=abc",
    "#link=a b",
    "#link=a%2Fb",
    "#link=a.b",
  ])("finds no secret in %j", (hash) => {
    expect(linkSecret(hash)).toBeNull();
  });
});

describe("withoutFragment", () => {
  it("drops the fragment and keeps the query", () => {
    expect(
      withoutFragment(
        `http://127.0.0.1:7420/sign-in?redirect=%2Fp#link=${SECRET}`,
      ),
    ).toBe("http://127.0.0.1:7420/sign-in?redirect=%2Fp");
    expect(withoutFragment("/sign-in")).toBe("/sign-in");
  });
});

describe("signInView", () => {
  it("opens the link until the person chooses a token", () => {
    expect(signInView(SECRET, false)).toEqual({ kind: "link", secret: SECRET });
    expect(signInView(SECRET, true)).toEqual({ kind: "token" });
    expect(signInView(null, false)).toEqual({ kind: "token" });
  });
});
