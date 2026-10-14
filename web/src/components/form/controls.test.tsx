// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { createRef } from "react";
import userEvent from "@testing-library/user-event";
import { Controller, useForm } from "react-hook-form";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Checkbox, Select, TextField, Textarea } from "./controls";

beforeAll(() => {
  // Radix sizes the checkbox's hidden form input with a ResizeObserver, which jsdom lacks.
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

afterEach(cleanup);

describe("TextField", () => {
  it("is named by its label and forwards its ref", () => {
    const ref = createRef<HTMLInputElement>();
    render(<TextField label="Title" ref={ref} />);
    const input = screen.getByRole("textbox", { name: "Title" });
    expect(ref.current).toBe(input);
    expect(input.getAttribute("aria-describedby")).toBeNull();
    expect(input.getAttribute("aria-invalid")).toBeNull();
  });

  it("links the hint and the error with aria-describedby", () => {
    render(<TextField label="Title" hint="Short and clear" error="Required" />);
    const input = screen.getByRole("textbox", { name: "Title" });
    const ids = (input.getAttribute("aria-describedby") ?? "").split(" ");
    expect(ids).toHaveLength(2);
    expect(document.getElementById(ids[0] ?? "")?.textContent).toBe(
      "Short and clear",
    );
    expect(document.getElementById(ids[1] ?? "")?.textContent).toBe("Required");
    expect(input.getAttribute("aria-invalid")).toBe("true");
  });

  it("describes with the hint alone when there is no error", () => {
    render(<TextField label="Title" hint="Short and clear" />);
    const input = screen.getByRole("textbox", { name: "Title" });
    expect(
      document.getElementById(input.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("Short and clear");
    expect(input.getAttribute("aria-invalid")).toBeNull();
  });

  it("gives two fields different ids", () => {
    render(
      <>
        <TextField label="A" />
        <TextField label="B" />
      </>,
    );
    expect(screen.getByLabelText("A").id).not.toBe(
      screen.getByLabelText("B").id,
    );
  });
});

describe("Textarea", () => {
  it("is a named textbox that forwards its ref and links its error", () => {
    const ref = createRef<HTMLTextAreaElement>();
    render(<Textarea label="Body" error="Too long" ref={ref} />);
    const area = screen.getByRole("textbox", { name: "Body" });
    expect(ref.current).toBe(area);
    expect(
      document.getElementById(area.getAttribute("aria-describedby") ?? "")
        ?.textContent,
    ).toBe("Too long");
  });
});

describe("Select", () => {
  it("is a named combobox that forwards its ref", () => {
    const ref = createRef<HTMLSelectElement>();
    render(
      <Select label="Agent" hint="Who works on it" ref={ref}>
        <option value="a">a</option>
      </Select>,
    );
    const select = screen.getByRole("combobox", { name: "Agent" });
    expect(ref.current).toBe(select);
    expect(select.getAttribute("aria-describedby")).not.toBeNull();
  });
});

describe("Checkbox", () => {
  it("is a named checkbox that forwards its ref", () => {
    const ref = createRef<HTMLButtonElement>();
    render(<Checkbox label="Blocked only" error="Pick one" ref={ref} />);
    const box = screen.getByRole("checkbox", { name: "Blocked only" });
    expect(ref.current).toBe(box);
    expect(box.getAttribute("aria-invalid")).toBe("true");
    expect(box.getAttribute("aria-checked")).toBe("false");
  });

  it("toggles from Space and from a click on its label, reporting a boolean", async () => {
    const onCheckedChange = vi.fn();
    render(<Checkbox label="Blocked only" onCheckedChange={onCheckedChange} />);
    const box = screen.getByRole("checkbox", { name: "Blocked only" });
    await userEvent.tab();
    expect(document.activeElement).toBe(box);
    await userEvent.keyboard(" ");
    expect(box.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(screen.getByText("Blocked only"));
    expect(box.getAttribute("aria-checked")).toBe("false");
    expect(onCheckedChange.mock.calls).toEqual([[true], [false]]);
  });

  it("cannot be toggled while disabled", async () => {
    render(<Checkbox label="Blocked only" disabled />);
    const box = screen.getByRole("checkbox", { name: "Blocked only" });
    await userEvent.click(box);
    expect(box.getAttribute("aria-checked")).toBe("false");
  });
});

interface Values {
  title: string;
  blocked: boolean;
}

function TaskForm({ onSubmit }: { onSubmit: (values: Values) => void }) {
  const { register, control, handleSubmit, reset } = useForm<Values>({
    defaultValues: { title: "Login form", blocked: true },
  });
  return (
    <form onSubmit={(event) => void handleSubmit(onSubmit)(event)}>
      <TextField label="Title" {...register("title")} />
      <Controller
        name="blocked"
        control={control}
        render={({ field }) => (
          <Checkbox
            label="Blocked only"
            ref={field.ref}
            name={field.name}
            checked={field.value}
            onCheckedChange={field.onChange}
            onBlur={field.onBlur}
          />
        )}
      />
      <button type="submit">Save</button>
      <button
        type="button"
        onClick={() => {
          reset();
        }}
      >
        Reset
      </button>
    </form>
  );
}

describe("react-hook-form", () => {
  it("registers a text field and binds a checkbox through Controller", async () => {
    const onSubmit = vi.fn();
    render(<TaskForm onSubmit={onSubmit} />);
    const box = screen.getByRole("checkbox", { name: "Blocked only" });
    expect(box.getAttribute("aria-checked")).toBe("true");

    await userEvent.type(screen.getByRole("textbox", { name: "Title" }), "!");
    await userEvent.click(box);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).toHaveBeenLastCalledWith(
      { title: "Login form!", blocked: false },
      expect.anything(),
    );

    await userEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(box.getAttribute("aria-checked")).toBe("true");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).toHaveBeenLastCalledWith(
      { title: "Login form", blocked: true },
      expect.anything(),
    );
  });
});
