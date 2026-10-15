import { describe, expect, it } from "vitest";
import { card, requestDraft } from "@/test/requests";
import {
  addCard,
  addEdge,
  checkDraft,
  draftBody,
  formFrom,
  freeEdge,
  moveCard,
  nextKey,
  refusalText,
  removeCard,
  removeEdge,
  updateCard,
  type DraftForm,
} from "./draft";

const form = (
  cards = [card("c1"), card("c2"), card("c3")],
  edges: DraftForm["edges"] = [],
) => ({
  cards,
  edges,
});

describe("editing a draft", () => {
  it("copies a version, so edits never touch the stored one", () => {
    const draft = requestDraft(2);
    const copy = formFrom(draft);
    copy.cards[0] = { ...card("c9") };
    expect(draft.cards[0]?.key).toBe("c1");
    expect(copy.edges).toEqual(draft.edges);
  });

  it("adds a card under the next free key, up to 20", () => {
    expect(nextKey([card("c1"), card("c3")])).toBe("c4");
    expect(nextKey([card("c2")])).toBe("c3");
    const added = addCard(form([card("c1")]));
    expect(added.cards.map((c) => c.key)).toEqual(["c1", "c2"]);
    const full = form(
      Array.from({ length: 20 }, (_, i) => card(`c${String(i + 1)}`)),
    );
    expect(addCard(full)).toBe(full);
  });

  it("removes a card with its edges", () => {
    const f = form(undefined, [
      { dependent: "c2", prerequisite: "c1" },
      { dependent: "c3", prerequisite: "c2" },
    ]);
    expect(removeCard(f, "c1")).toEqual({
      cards: [card("c2"), card("c3")],
      edges: [{ dependent: "c3", prerequisite: "c2" }],
    });
  });

  it("changes a title or description", () => {
    const f = updateCard(form(), "c2", { title: "New" });
    expect(f.cards[1]?.title).toBe("New");
    expect(f.cards[0]?.title).toBe("Card c1");
  });

  it("moves a card inside the list only", () => {
    const keys = (f: DraftForm) => f.cards.map((c) => c.key);
    expect(keys(moveCard(form(), "c2", -1))).toEqual(["c2", "c1", "c3"]);
    expect(keys(moveCard(form(), "c2", 1))).toEqual(["c1", "c3", "c2"]);
    const f = form();
    expect(moveCard(f, "c1", -1)).toBe(f);
    expect(moveCard(f, "c3", 1)).toBe(f);
    expect(moveCard(f, "c9", 1)).toBe(f);
  });

  it("adds and removes edges, offering the first pair not taken", () => {
    const f = form([card("c1"), card("c2")]);
    expect(freeEdge(f)).toEqual({ dependent: "c1", prerequisite: "c2" });
    const one = addEdge(f, { dependent: "c1", prerequisite: "c2" });
    expect(freeEdge(one)).toEqual({ dependent: "c2", prerequisite: "c1" });
    const both = addEdge(one, { dependent: "c2", prerequisite: "c1" });
    expect(freeEdge(both)).toBeNull();
    expect(removeEdge(both, 0).edges).toEqual([
      { dependent: "c2", prerequisite: "c1" },
    ]);
  });
});

describe("checkDraft", () => {
  const refusal = (f: DraftForm) => {
    const r = checkDraft(f);
    return r && [r.reason, r.details];
  };

  it("accepts a valid draft", () => {
    expect(
      checkDraft(form(undefined, [{ dependent: "c2", prerequisite: "c1" }])),
    ).toBeNull();
  });

  it("refuses in the daemon's order with its reasons", () => {
    expect(refusal(form([]))).toEqual(["empty", {}]);
    expect(
      refusal(
        form(Array.from({ length: 21 }, (_, i) => card(`c${String(i + 1)}`))),
      ),
    ).toEqual(["too_many_cards", { cards: 21 }]);
    expect(refusal(form([card("c 1")]))).toEqual([
      "invalid_key",
      { position: 1 },
    ]);
    expect(refusal(form([card("c1"), card("c1")]))).toEqual([
      "duplicate_key",
      { key: "c1" },
    ]);
    expect(refusal(form([card("c1", "  ")]))).toEqual([
      "empty_title",
      { key: "c1" },
    ]);
    expect(refusal(form([card("c1", "a\nb")]))).toEqual([
      "title_not_one_line",
      { key: "c1" },
    ]);
    expect(refusal(form([card("c1", "x".repeat(201))]))).toEqual([
      "title_too_long",
      { key: "c1" },
    ]);
    expect(
      refusal(form([{ key: "c1", title: "t", description: " " }])),
    ).toEqual(["empty_description", { key: "c1" }]);
    expect(
      refusal(
        form([{ key: "c1", title: "t", description: "x".repeat(20_001) }]),
      ),
    ).toEqual(["description_too_long", { key: "c1" }]);
  });

  it("refuses edges that name no card, repeat, loop or close a cycle", () => {
    expect(
      refusal(form(undefined, [{ dependent: "c9", prerequisite: "c1" }])),
    ).toEqual(["unknown_key", { position: 1 }]);
    const e = { dependent: "c2", prerequisite: "c1" };
    expect(refusal(form(undefined, [e, { ...e }]))).toEqual([
      "duplicate_edge",
      { edge: e },
    ]);
    expect(
      refusal(form(undefined, [{ dependent: "c1", prerequisite: "c1" }])),
    ).toEqual(["self_edge", { key: "c1" }]);
    expect(
      refusal(
        form(undefined, [
          { dependent: "c2", prerequisite: "c1" },
          { dependent: "c3", prerequisite: "c2" },
          { dependent: "c1", prerequisite: "c3" },
        ]),
      ),
    ).toEqual(["cycle", { path: ["c1", "c3", "c2", "c1"] }]);
  });

  it("explains a refusal in a sentence", () => {
    const r = checkDraft(form([card("c1", "")]));
    expect(r && refusalText(r)).toBe("Card c1 needs a title.");
  });
});

describe("draftBody", () => {
  it("trims titles and leaves a blank note out", () => {
    const f = form([card("c1", "  Query ")]);
    expect(draftBody(f, "  ")).toEqual({
      cards: [
        { key: "c1", title: "Query", description: "**Scope** the work of c1" },
      ],
      edges: [],
    });
    expect(draftBody(f, " split ").note).toBe("split");
  });
});
