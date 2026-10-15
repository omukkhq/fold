// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { PresenceDot } from "./PresenceDot";

afterEach(cleanup);

describe("PresenceDot", () => {
  it.each([
    ["active", "Active"],
    ["idle", "Idle"],
    ["offline", "Offline"],
  ] as const)("names the %s state in words", (state, name) => {
    render(<PresenceDot state={state} />);
    expect(screen.getByRole("img", { name })).toBeTruthy();
  });
});
