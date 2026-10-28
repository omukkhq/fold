// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Button } from "../button/Button";
import { Dialog } from "./Dialog";
import { Menu } from "./Menu";
import { Popover } from "./Popover";
import { Tooltip } from "./Tooltip";

beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
});

afterEach(cleanup);

describe("Dialog", () => {
  it("moves focus in, keeps it inside and restores it to the trigger on Escape", async () => {
    render(
      <>
        <Dialog
          trigger={<Button>Open</Button>}
          title="Rename"
          description="Pick a new name"
        >
          <input aria-label="Name" />
        </Dialog>
        <button type="button">Outside</button>
      </>,
    );
    const trigger = screen.getByRole("button", { name: "Open" });
    await userEvent.click(trigger);
    const dialog = screen.getByRole("dialog", { name: "Rename" });
    expect(dialog.contains(document.activeElement)).toBe(true);
    for (let i = 0; i < 4; i += 1) {
      await userEvent.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("closes from the Close button", async () => {
    const onOpenChange = vi.fn();
    render(<Dialog open onOpenChange={onOpenChange} title="Rename" />);
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe("Menu", () => {
  it("opens with the keyboard, runs an item and restores focus to the trigger", async () => {
    const onSelect = vi.fn();
    render(
      <Menu
        trigger={<Button>Actions</Button>}
        items={[
          { label: "Rename", onSelect },
          { label: "Delete", onSelect: () => {}, danger: true },
        ]}
      />,
    );
    const trigger = screen.getByRole("button", { name: "Actions" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    expect(screen.getByRole("menu")).toBeTruthy();
    expect(screen.getAllByRole("menuitem")).toHaveLength(2);
    await userEvent.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});

describe("Popover", () => {
  it("opens, moves focus in and restores it on Escape", async () => {
    render(
      <Popover trigger={<Button>Filter</Button>}>
        <button type="button">Inside</button>
      </Popover>,
    );
    const trigger = screen.getByRole("button", { name: "Filter" });
    await userEvent.click(trigger);
    expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(
      true,
    );
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});

describe("Tooltip", () => {
  it("describes its trigger on keyboard focus", async () => {
    render(
      <Tooltip content="Copy the id">
        <Button>Copy</Button>
      </Tooltip>,
    );
    await userEvent.tab();
    expect((await screen.findAllByText("Copy the id")).length).toBeGreaterThan(
      0,
    );
    expect(screen.getByRole("tooltip").textContent).toBe("Copy the id");
  });
});
