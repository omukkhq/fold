import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { ProjectAccount } from "@/api/workers";
import {
  DEFAULT_ACCOUNT,
  accountFailure,
  accountHint,
  accountOptions,
  shownValue,
} from "./accountModel";

function view(fields: Partial<ProjectAccount> = {}): ProjectAccount {
  return {
    account: DEFAULT_ACCOUNT,
    accounts: [DEFAULT_ACCOUNT, "work-a", "work-b"],
    ...fields,
  };
}

function refused(code: string): ApiError {
  return new ApiError(code, "");
}

describe("accountModel", () => {
  it("offers default first, then each account, by name only, and keeps a missing one shown", () => {
    expect(accountOptions(view())).toEqual([
      {
        value: DEFAULT_ACCOUNT,
        label: "default",
      },
      { value: "work-a", label: "work-a" },
      { value: "work-b", label: "work-b" },
    ]);
    expect(accountOptions(view({ account: "gone" })).at(-1)).toEqual({
      value: "gone",
      label: "gone (not on this host)",
    });
    expect(accountOptions(view({ account: "work-a" }))).toHaveLength(3);
  });

  it("shows the choice under way, else the project's account", () => {
    expect(shownValue(view(), undefined)).toBe(DEFAULT_ACCOUNT);
    expect(shownValue(view({ account: "work-a" }), undefined)).toBe("work-a");
    expect(shownValue(view({ account: "work-a" }), "work-b")).toBe("work-b");
  });

  it("says what a change does, how to add an account and what a missing one means", () => {
    expect(accountHint(view(), true)).toBe("Saving…");
    expect(accountHint(view(), false)).toContain(
      "continues in its own session",
    );
    expect(accountHint(view({ accounts: [DEFAULT_ACCOUNT] }), false)).toContain(
      "fld account add <name>",
    );
    expect(accountHint(view({ account: "gone" }), false)).toContain(
      "fld account add gone",
    );
    // A host without Claude Code's own login lists no default, and a project that still names it needs that login.
    expect(
      accountHint(
        view({ accounts: ["work-a"], account: DEFAULT_ACCOUNT }),
        false,
      ),
    ).toContain("claude /login");
    expect(accountHint(view({ accounts: [] }), false)).toContain(
      "claude /login",
    );
  });

  it("words each refusal by its code", () => {
    for (const [code, words] of [
      ["account_not_found", "no such account"],
      ["invalid_account_name", "account's name"],
      ["invalid_config", ".fold/config.toml"],
      ["accounts_io", "accounts folder"],
      ["human_required", "signed-in person"],
    ] as const) {
      const failure = accountFailure(refused(code));
      expect(failure.code).toBe(code);
      expect(failure.text).toContain(words);
    }
    expect(accountFailure(refused("human_name_required")).text).toBe(
      "Tell fold your name first, then try once more.",
    );
    expect(accountFailure(new Error("offline")).code).toBeNull();
  });
});
