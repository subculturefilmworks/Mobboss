import { expect, test } from "vitest";
import { formatVersion } from "./version";
test("formats the complete release identity", () => {
  expect(
    formatVersion({
      version: "v0.1.0-rc13",
      date: "2026-11-25 04:30 UTC",
      commit: "02561d7",
    }),
  ).toBe("v0.1.0-rc13 · 2026-11-25 04:30 UTC · 02561d7");
});
