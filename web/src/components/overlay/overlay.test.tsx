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

describe("Dialog layout", () => {
  const longText = "A long review line. ".repeat(400);

  it("names itself from its title and description", () => {
    render(
      <Dialog open title="Changes requested on v1" description="By backend">
        <p>{longText}</p>
      </Dialog>,
    );
    const dialog = screen.getByRole("dialog", {
      name: "Changes requested on v1",
    });
    expect(dialog.getAttribute("aria-describedby")).toBe(
      screen.getByText("By backend").id,
    );
  });

  it("keeps the title and Close outside the one scroll container that holds the body", () => {
    render(
      <Dialog open title="Changes requested on v1">
        <p>{longText}</p>
        <a href="/x">T-12</a>
      </Dialog>,
    );
    const dialog = screen.getByRole("dialog");
    const scrollers = [...dialog.querySelectorAll(".overflow-y-auto")];
    expect(scrollers).toHaveLength(1);
    const body = scrollers[0];
    expect(body?.contains(screen.getByText(longText.trim()))).toBe(true);
    expect(body?.contains(screen.getByRole("link", { name: "T-12" }))).toBe(
      true,
    );
    expect(body?.contains(screen.getByText("Changes requested on v1"))).toBe(
      false,
    );
    expect(body?.contains(screen.getByRole("button", { name: "Close" }))).toBe(
      false,
    );
    expect(body?.classList.contains("min-h-0")).toBe(true);
    expect(dialog.classList.contains("max-h-full")).toBe(true);
    expect(dialog.classList.contains("overflow-hidden")).toBe(true);
  });

  it("keeps the footer in view under the scrolling body, and a form in the body submits from it", async () => {
    const onSubmit = vi.fn((event: { preventDefault: () => void }) => {
      event.preventDefault();
    });
    render(
      <Dialog
        open
        title="Send feedback"
        footer={
          <Button type="submit" form="feedback">
            Send
          </Button>
        }
      >
        <form id="feedback" onSubmit={onSubmit}>
          <p>{longText}</p>
        </form>
      </Dialog>,
    );
    const dialog = screen.getByRole("dialog");
    const body = dialog.querySelector(".overflow-y-auto");
    const send = screen.getByRole("button", { name: "Send" });
    expect(body?.contains(send)).toBe(false);
    expect(send.closest(".shrink-0")?.parentElement).toBe(dialog);
    await userEvent.click(send);
    expect(onSubmit).toHaveBeenCalledOnce();
  });

  it("pads the scrolling body on every side by at least a focus ring's reach", () => {
    const pad = (classes: string[], names: string[]) =>
      Math.max(
        0,
        ...classes.map((c) => {
          const [name, size] = c.split("-");
          return names.includes(name ?? "") ? Number(size) : 0;
        }),
      );
    for (const footer of [undefined, <Button key="f">Done</Button>]) {
      const { unmount } = render(
        <Dialog open title="Archive T-2?" footer={footer}>
          <Button>Archive task</Button>
        </Dialog>,
      );
      const inner = screen
        .getByRole("dialog")
        .querySelector(".overflow-y-auto")?.firstElementChild;
      const classes = [...(inner?.classList ?? [])];
      for (const names of [
        ["p", "py", "pt"],
        ["p", "py", "pb"],
        ["p", "px"],
      ])
        expect(pad(classes, names), names.join()).toBeGreaterThanOrEqual(4);
      unmount();
    }
  });

  it("draws no body band without a body, and hides a body whose parts render nothing", () => {
    function Nothing() {
      return null;
    }
    const { unmount } = render(
      <Dialog open title="Drop stash@{0}?" footer={<Button>Drop</Button>}>
        {null}
      </Dialog>,
    );
    expect(
      screen.getByRole("dialog").querySelector(".overflow-y-auto"),
    ).toBeNull();
    unmount();
    render(
      <Dialog open title="Drop stash@{0}?" footer={<Button>Drop</Button>}>
        <Nothing />
      </Dialog>,
    );
    const inner = screen
      .getByRole("dialog")
      .querySelector(".overflow-y-auto")?.firstElementChild;
    expect(inner?.childElementCount).toBe(0);
    expect(inner?.classList.contains("empty:hidden")).toBe(true);
  });

  it("sits inside its scrim on the dialog layer, which pads it inside the window", () => {
    render(<Dialog open title="Rename" />);
    const scrim = screen.getByRole("dialog").parentElement;
    expect(scrim?.classList.contains("fixed")).toBe(true);
    expect(scrim?.classList.contains("inset-0")).toBe(true);
    expect(scrim?.classList.contains("z-dialog")).toBe(true);
    expect(scrim?.classList.contains("p-16")).toBe(true);
    expect(scrim?.classList.contains("max-md:p-0")).toBe(false);
  });

  it("takes the reading width when wide, and the whole screen of a phone with the action row at its bottom", () => {
    render(
      <Dialog
        open
        wide
        title="Changes requested on v1"
        footer={<button type="button">Send</button>}
      >
        <p>Body</p>
      </Dialog>,
    );
    const dialog = screen.getByRole("dialog");
    expect(dialog.classList.contains("w-dialog")).toBe(true);
    expect(dialog.classList.contains("max-md:h-full")).toBe(true);
    expect(dialog.parentElement?.classList.contains("max-md:p-0")).toBe(true);
    const body = screen.getByText("Body").parentElement?.parentElement;
    expect(body?.classList.contains("max-md:flex-1")).toBe(true);
    expect(dialog.lastElementChild?.textContent).toBe("Send");
  });

  it("sizes a narrow dialog to its content on a phone", () => {
    render(
      <Dialog open title="Rename">
        <p>Body</p>
      </Dialog>,
    );
    const body = screen.getByText("Body").parentElement?.parentElement;
    expect(body?.classList.contains("max-md:flex-1")).toBe(false);
  });
});

describe("overlay layers", () => {
  it("draws popovers and menus on the pop layer", async () => {
    render(
      <>
        <Popover trigger={<Button>Filter</Button>}>
          <p>Only mine</p>
        </Popover>
        <Menu
          trigger={<Button>Actions</Button>}
          items={[{ label: "Rename", onSelect: () => {} }]}
        />
      </>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Filter" }));
    const panel = screen.getByText("Only mine").parentElement;
    expect(panel?.classList.contains("z-pop")).toBe(true);
    await userEvent.keyboard("{Escape}");
    screen.getByRole("button", { name: "Actions" }).focus();
    await userEvent.keyboard("{Enter}");
    expect(screen.getByRole("menu").classList.contains("z-pop")).toBe(true);
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

  it("stops at the room Radix measures in the window and scrolls inside", async () => {
    const items = Array.from({ length: 300 }, (_, n) => ({
      label: `worker-${String(n)}`,
      onSelect: () => {},
    }));
    render(<Menu trigger={<Button>Agents</Button>} items={items} />);
    const trigger = screen.getByRole("button", { name: "Agents" });
    trigger.focus();
    await userEvent.keyboard("{Enter}");
    const menu = screen.getByRole("menu");
    expect(menu.className).toContain(
      "max-h-(--radix-dropdown-menu-content-available-height)",
    );
    expect(menu.className).toContain(
      "max-w-(--radix-dropdown-menu-content-available-width)",
    );
    expect(menu.classList.contains("overflow-y-auto")).toBe(true);
    // A width of its own content would beat the window's width, so the menu takes none.
    expect(menu.classList.contains("min-w-max")).toBe(false);
    // The keyboard reaches the last of hundreds of items; the browser scrolls the focused item into view.
    await userEvent.keyboard("{End}");
    expect(document.activeElement?.textContent).toBe("worker-299");
  });

  it("is at least as wide as its trigger when asked, and still grows to its rows within the window", async () => {
    render(
      <Menu
        trigger={<Button>Switch account</Button>}
        minTriggerWidth
        items={[{ label: "work-b", onSelect: () => {} }]}
      />,
    );
    screen.getByRole("button", { name: "Switch account" }).focus();
    await userEvent.keyboard("{Enter}");
    const menu = screen.getByRole("menu");
    expect(menu.className).toContain(
      "min-w-(--radix-dropdown-menu-trigger-width)",
    );
    expect(menu.className).toContain("w-max");
    expect(menu.className).toContain(
      "max-w-(--radix-dropdown-menu-content-available-width)",
    );
  });

  it("keeps a menu of tick boxes open while the reader ticks several", async () => {
    const ticked = new Set<string>(["ui"]);
    const onTick = vi.fn((name: string) => {
      ticked.add(name);
    });
    render(
      <Menu
        trigger={<Button>Labels</Button>}
        items={["data", "ui"].map((name) => ({
          label: name,
          hint: name === "ui" ? "web" : undefined,
          checked: ticked.has(name),
          onSelect: () => {
            onTick(name);
          },
        }))}
      />,
    );
    screen.getByRole("button", { name: "Labels" }).focus();
    await userEvent.keyboard("{Enter}");
    const ui = screen.getByRole("menuitemcheckbox", { name: "ui web" });
    expect(ui.getAttribute("aria-checked")).toBe("true");
    const data = screen.getByRole("menuitemcheckbox", { name: "data" });
    expect(data.getAttribute("aria-checked")).toBe("false");
    await userEvent.click(data);
    expect(onTick).toHaveBeenCalledWith("data");
    expect(screen.getByRole("menu")).toBeTruthy();
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
  it("describes its trigger on keyboard focus, on the tooltip layer", async () => {
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
    expect(screen.getByRole("tooltip").closest(".z-tooltip")).not.toBeNull();
  });
});
