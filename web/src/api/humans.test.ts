import { afterEach, describe, expect, it, vi } from "vitest";
import { errorReply, stubFetch } from "../test/stubFetch";
import { isApiError } from "./errors";
import { humansQuery, listHumans } from "./humans";
import { keys } from "./keys";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("listHumans", () => {
  it("unwraps the names", async () => {
    stubFetch({ "GET /api/v1/humans": { body: { humans: ["ana", "tanim"] } } });
    await expect(listHumans()).resolves.toEqual(["ana", "tanim"]);
  });

  it("rejects with the daemon's code", async () => {
    stubFetch({ "GET /api/v1/humans": errorReply(401, "auth_required") });
    await expect(listHumans()).rejects.toSatisfy((e: unknown) =>
      isApiError(e, "auth_required"),
    );
  });

  it("is read again on focus once a minute old", () => {
    const options = humansQuery();
    expect(options.queryKey).toEqual(keys.humans());
    expect(options.staleTime).toBe(60_000);
    expect(options.refetchOnWindowFocus).toBe(true);
    expect(options.refetchInterval).toBeUndefined();
  });
});
