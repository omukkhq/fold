import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { getSession, signIn, signInWithLink } from "./session";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("getSession", () => {
  it("reads the session status", async () => {
    stubFetch({
      "GET /api/v1/session": { body: { human: "tanim", authenticated: true } },
    });
    await expect(getSession()).resolves.toEqual({
      human: "tanim",
      authenticated: true,
    });
  });
});

describe("signInWithLink", () => {
  it("posts the link's secret and nothing else", async () => {
    const calls = stubFetch({ "POST /api/v1/session": { status: 204 } });
    await signInWithLink("secret");
    expect(calls).toHaveLength(1);
    expect(calls[0]?.path).toBe("/api/v1/session");
    expect(calls[0]?.body).toEqual({ link: "secret" });
  });

  it("throws the daemon's code for a used link", async () => {
    stubFetch({
      "POST /api/v1/session": errorReply(401, "invalid_sign_in_link"),
    });
    await expect(signInWithLink("used")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "invalid_sign_in_link"),
    );
  });
});

describe("signIn", () => {
  it("posts the token and nothing else", async () => {
    const calls = stubFetch({ "POST /api/v1/session": { status: 204 } });
    await signIn("Qm9sZF9leGFtcGxlX3Rva2VuX2Zvcl90ZXN0cw");
    expect(calls[0]?.body).toEqual({
      token: "Qm9sZF9leGFtcGxlX3Rva2VuX2Zvcl90ZXN0cw",
    });
  });

  it("throws the daemon's code for a refused token", async () => {
    stubFetch({
      "POST /api/v1/session": errorReply(401, "invalid_token"),
    });
    await expect(signIn("bad")).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "invalid_token"),
    );
  });

  it("throws a plain error when the body is not the error envelope", async () => {
    stubFetch({ "POST /api/v1/session": { status: 502, body: "bad gateway" } });
    await expect(signIn("x")).rejects.toThrow("the daemon answered 502");
  });
});
