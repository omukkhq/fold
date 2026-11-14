import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/errors";
import type { ProjectAccount } from "@/api/workers";
import {
  SYSTEM_DEFAULT,
  accountFailure,
  accountHint,
  accountLabel,
  accountOptions,
  shownValue,
} from "./accountModel";

function view(fields: Partial<ProjectAccount> = {}): ProjectAccount {
  return {
    account: SYSTEM_DEFAULT,
    accounts: [SYSTEM_DEFAULT, "work-a", "work-b"],
    ...fields,
  };
}

function refused(code: string): ApiError {
  return new ApiError(code, "");
}

describe("accountModel", () => {
  it("offers system-default first, then each account, and keeps a missing one shown", () => {
    expect(accountOptions(view())).toEqual([
      {
        value: SYSTEM_DEFAULT,
        label: "system-default (Claude Code's own login)",
      },
      { value: "work-a", label: "work-a" },
      { value: "work-b", label: "work-b" },
    ]);
    expect(accountOptions(view({ account: "gone" })).at(-1)).toEqual({
      value: "gone",
      label: "gone (not on this host)",
    });
    expect(accountOptions(view({ account: "work-a" }))).toHaveLength(3);
    expect(accountLabel("work-a")).toBe("work-a");
  });

  it("shows the choice under way, else the project's account", () => {
    expect(shownValue(view(), undefined)).toBe(SYSTEM_DEFAULT);
    expect(shownValue(view({ account: "work-a" }), undefined)).toBe("work-a");
    expect(shownValue(view({ account: "work-a" }), "work-b")).toBe("work-b");
  });

  it("says what a change does, how to add an account and what a missing one means", () => {
    expect(accountHint(view(), true)).toBe("Saving…");
    expect(accountHint(view(), false)).toContain(
      "continues in its own session",
    );
    expect(accountHint(view({ accounts: [SYSTEM_DEFAULT] }), false)).toContain(
      "fld account add <name>",
    );
    expect(accountHint(view({ account: "gone" }), false)).toContain(
      "fld account add gone",
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
