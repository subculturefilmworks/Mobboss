import { describe, expect, it } from "vitest";
import {
  availableSources,
  sameSource,
  mergeSources,
  sourceLabel,
  checkedSources,
} from "./sources";
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

describe("persisted source rows", () => {
  it("keeps checks, marks missing rows offline, and adds new folders on search", () => {
    const old = [
      {
        path: found[0].path,
        checked: false,
        offline: true,
        volume: "Media-01",
      },
      {
        path: "/Volumes/Absent/MXF/1",
        checked: true,
        offline: false,
        volume: "Absent",
      },
    ];
    const merged = mergeSources(old, found);
    expect(merged.map((r) => [r.path, r.checked, r.offline])).toEqual([
      [found[0].path, false, false],
      [old[1].path, true, true],
      [found[1].path, true, false],
    ]);
    expect(mergeSources(merged, found)).toEqual(merged);
  });
  it("scans only checked rows in both lists, retaining offline selections", () => {
    const row = (path: string, checked: boolean) => ({
      path,
      checked,
      offline: true,
      volume: "",
    });
    expect(
      checkedSources({
        mounted_sources: [
          row("E:/Avid MediaFiles/MXF/1", true),
          row("E:/Avid MediaFiles/MXF/2", false),
        ],
        custom_folders: [
          row("M:/Project/Backup/MXF/1", false),
          row("M:/Project/Media", true),
        ],
      }),
    ).toEqual(["E:/Avid MediaFiles/MXF/1", "M:/Project/Media"]);
  });
});

it("shows the Mac volume name even for the system root, and preserves Windows paths", () => {
  const row = {
    path: "/Avid MediaFiles/MXF/1",
    volume: "System-Island",
    checked: true,
    offline: false,
  };
  expect(sourceLabel(row, "MacIntel")).toBe(
    "System-Island · /Avid MediaFiles/MXF/1",
  );
  expect(
    sourceLabel({ ...row, path: "E:/Avid MediaFiles/MXF/1" }, "Win32"),
  ).toBe("E:/Avid MediaFiles/MXF/1");
});
