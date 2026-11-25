import { describe, expect, it } from "vitest";
import { availableSources, sameSource } from "./sources";
const found = [
  {
    path: "/Volumes/Media-01/Avid MediaFiles",
    volume: "Media-01",
    available: 40,
    total: 100,
  },
  {
    path: "/Volumes/NEXIS-Workspace/OMFI MediaFiles",
    volume: "NEXIS-Workspace",
    available: 0,
    total: 0,
  },
];
describe("available Avid media folders", () => {
  it("only lists candidates, preserving paths, capacity, and configured sources", () => {
    const roots = ["/Volumes/Manual/Footage"];
    expect(availableSources(found, roots)).toEqual(found);
    expect(roots).toEqual(["/Volumes/Manual/Footage"]);
  });
  it("omits folders already added or covered by a manual parent", () => {
    expect(availableSources(found, [found[0].path])).toEqual([found[1]]);
    expect(availableSources(found, ["/Volumes/Media-01"])).toEqual([found[1]]);
    expect(availableSources(found, ["/Volumes/Media-0"])).toEqual(found);
  });
  it("omits duplicate mount aliases and offers removed sources again", () => {
    expect(availableSources([...found, found[0]], [])).toEqual(found);
    expect(
      availableSources(
        found,
        found.map((s) => s.path),
      ),
    ).toEqual([]);
    expect(availableSources(found, [])).toEqual(found);
  });
  it("keeps an offline source configured without listing it as available", () => {
    const roots = [found[0].path];
    expect(availableSources([], roots)).toEqual([]);
    expect(roots).toEqual([found[0].path]);
  });
  it("compares Windows drive and UNC paths consistently", () => {
    expect(sameSource("X:\\Avid MediaFiles", "x:/avid mediafiles/")).toBe(true);
    expect(
      sameSource("\\\\?\\X:\\Avid MediaFiles", "X:\\Avid MediaFiles"),
    ).toBe(true);
    expect(
      sameSource(
        "\\\\?\\UNC\\Media-01\\Share\\Avid MediaFiles",
        "\\\\Media-01\\Share\\Avid MediaFiles",
      ),
    ).toBe(true);
  });
});
