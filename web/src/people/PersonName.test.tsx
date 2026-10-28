// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ActorText } from "./ActorText";
import { HumanReviewBadge } from "./HumanReviewBadge";
import { PersonList, PersonName } from "./PersonName";
import { actorSegment, seg } from "./segments";

afterEach(cleanup);

describe("PersonName", () => {
  it("draws a human as @name in the link-text colour, with no avatar and no prefix", () => {
    const { container } = render(<PersonName name="human:tanim" />);
    const name = screen.getByText("@tanim");
    expect(name.getAttribute("data-person")).toBe("human");
    expect(name.className).toContain("text-accent");
    expect(name.className).toContain("font-medium");
    expect(container.querySelector('[role="img"]')).toBeNull();
  });

  it("marks the signed-in human as you, and only them", () => {
    const { container } = render(
      <PersonList
        names={["human:tanim", "human:rafeed", "tanim"]}
        you="tanim"
      />,
    );
    expect(
      [...container.querySelectorAll("[data-person]")].map(
        (name) => name.textContent,
      ),
    ).toEqual(["@tanim (you)", "@rafeed", "@tanim"]);
  });

  it("names the signed-in human by the word you inside a sentence", () => {
    const { container } = render(
      <p>
        <ActorText
          segments={seg`Read by ${actorSegment("human:tanim")} and ${actorSegment("human:rafeed")}`}
          you="tanim"
        />
      </p>,
    );
    expect(container.textContent).toBe("Read by you and @rafeed");
    expect(
      container.querySelector('[data-person="human"]')?.className,
    ).toContain("text-accent");
  });

  it("draws an agent as @name in the text colour, never gold", () => {
    render(<PersonName name="frontend/ui" />);
    const name = screen.getByText("@frontend/ui");
    expect(name.getAttribute("data-person")).toBe("agent");
    expect(name.className).toContain("text-text");
    expect(name.className).not.toContain("text-warn");
    expect(name.className).not.toContain("text-accent");
  });

  it("takes the caller's size but keeps the body face, the weight and its colour", () => {
    render(
      <PersonName
        name="backend/db"
        className="font-mono text-13 font-semibold text-text-2"
      />,
    );
    const name = screen.getByText("@backend/db");
    expect(name.className).toContain("text-13");
    expect(name.className).toContain("font-body");
    expect(name.className).toContain("font-medium");
    expect(name.className).toContain("text-text");
    expect(name.className).not.toContain("font-mono");
    expect(name.className).not.toContain("font-semibold");
    expect(name.className).not.toContain("text-text-2");
  });
});

describe("PersonName slots", () => {
  it("sizes a human and an agent by the slot's sizes", () => {
    const { container } = render(
      <>
        <PersonName name="human:ana" slot="requester" />
        <PersonName name="frontend" slot="requester" />
      </>,
    );
    const [human, agent] = container.querySelectorAll("[data-person]");
    expect(human?.className).toContain("text-15");
    expect(agent?.className).toContain("text-14");
    expect(agent?.className).not.toContain("text-15");
  });

  it("passes the slot through a list and through actor text", () => {
    const { container } = render(
      <>
        <PersonList names={["human:ana", "backend"]} slot="messageHead" />
        <ActorText
          segments={seg`by ${actorSegment("human:ana")}`}
          slot="gate"
        />
      </>,
    );
    const names = container.querySelectorAll("[data-person]");
    expect(names[0]?.className).toContain("text-14");
    expect(names[1]?.className).toContain("text-13");
    expect(names[2]?.className).toContain("text-15");
  });
});

describe("PersonList", () => {
  it("joins the names with commas, each in its own colour and the given size", () => {
    const { container } = render(
      <PersonList names={["frontend", "human:ana"]} nameClassName="text-14" />,
    );
    expect(container.textContent).toBe("@frontend, @ana");
    const names = container.querySelectorAll("[data-person]");
    expect(names).toHaveLength(2);
    expect(names[1]?.className).toContain("text-14");
  });
});

describe("ActorText", () => {
  it("draws the actors of a line as @names in the given size", () => {
    const { container } = render(
      <ActorText
        segments={seg`Claimed by ${actorSegment("frontend/ui")} · 13:40`}
        personClassName="text-14"
      />,
    );
    expect(container.textContent).toBe("Claimed by @frontend/ui · 13:40");
    expect(screen.getByText("@frontend/ui").className).toContain("text-14");
  });
});

describe("HumanReviewBadge", () => {
  it("names the reviewer as @name, or no one", () => {
    const { container, rerender } = render(
      <HumanReviewBadge reviewer="tanim" />,
    );
    expect(container.textContent).toContain("Human review");
    expect(container.querySelector('[data-person="human"]')?.textContent).toBe(
      "@tanim",
    );
    rerender(<HumanReviewBadge reviewer={null} />);
    expect(container.querySelector("[data-person]")).toBeNull();
  });
});
