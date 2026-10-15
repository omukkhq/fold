import { describe, expect, it } from "vitest";
import { apiErrorCode, ApiError } from "../../api/errors";
import { signInErrorMessage } from "./messages";
import { signInMode } from "./mode";

describe("signInMode", () => {
  it("asks for a token when the request is not authenticated", () => {
    expect(signInMode({ authenticated: false, human: null })).toBe("token");
    expect(signInMode({ authenticated: false, human: "tanim" })).toBe("token");
  });

  it("asks for a name when authenticated without one", () => {
    expect(signInMode({ authenticated: true, human: null })).toBe("name");
  });

  it("is ready with a name", () => {
    expect(signInMode({ authenticated: true, human: "tanim" })).toBe("ready");
  });
});

describe("signInErrorMessage", () => {
  it("words a refused token from its code, not its message", () => {
    for (const code of ["invalid_token", "auth_required"]) {
      const error = new ApiError(code, "secret-token-text");
      expect(apiErrorCode(error)).toBe(code);
      const message = signInErrorMessage(error);
      expect(message).toContain("token was not accepted");
      expect(message).not.toContain("secret-token-text");
    }
  });

  it("words the other cases", () => {
    expect(signInErrorMessage(new ApiError("invalid_name", ""))).toContain(
      "name",
    );
    expect(signInErrorMessage(new TypeError("fetch failed"))).toContain(
      "did not answer",
    );
    expect(
      signInErrorMessage(new ApiError("daemon_unavailable", "")),
    ).toContain("did not answer");
    expect(signInErrorMessage(new ApiError("internal_error", ""))).toBe(
      "Sign-in failed. Try again.",
    );
  });
});
