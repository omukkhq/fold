// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Button } from "../button/Button";
import { Dialog } from "./Dialog";
import { Menu } from "./Menu";
import { Popover } from "./Popover";
import { Tabs } from "./Tabs";
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

describe("Tabs", () => {
  const items = [
    { value: "a", label: "Board", content: <p>Board panel</p> },
    { value: "b", label: "Activity", content: <p>Activity panel</p> },
  ];

  it("selects the first tab and moves with the arrow keys", async () => {
    render(<Tabs aria-label="Views" items={items} />);
    const board = screen.getByRole("tab", { name: "Board" });
    expect(board.getAttribute("aria-selected")).toBe("true");
    board.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(
      screen
        .getByRole("tab", { name: "Activity" })
        .getAttribute("aria-selected"),
    ).toBe("true");
    expect(screen.getByRole("tabpanel").textContent).toBe("Activity panel");
  });

  it("can be controlled", async () => {
    const onValueChange = vi.fn();
    render(
      <Tabs
        aria-label="Views"
        items={items}
        value="b"
        onValueChange={onValueChange}
      />,
    );
    expect(screen.getByRole("tabpanel").textContent).toBe("Activity panel");
    await userEvent.click(screen.getByRole("tab", { name: "Board" }));
    expect(onValueChange).toHaveBeenCalledWith("a");
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
