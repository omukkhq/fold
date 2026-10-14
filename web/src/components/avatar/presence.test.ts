import { describe, expect, it } from "vitest";
import { PRESENCE_STATES, presenceSpec } from "./presence";

describe("presenceSpec", () => {
  it.each(PRESENCE_STATES)(
    "names %s and colours it from its token",
    (state) => {
      const spec = presenceSpec(state);
      expect(spec.dot).toBe(`bg-presence-${state}`);
      expect(spec.label.toLowerCase()).toBe(state);
    },
  );
});
