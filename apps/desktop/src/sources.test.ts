import {
  groupSources,
  driveCheckState,
  toggleDrive,
  shortSourceLabel,
  pathKey,
  readRailWidth,
  saveRailWidth,
  clampRailWidth,
  DEFAULT_RAIL_WIDTH,
  MIN_RAIL_WIDTH,
  RAIL_WIDTH_KEY,
} from "./sources";
import { rememberMounted, middleEllipsis, projectCheckState } from "./sources";
import { describe, expect, it } from "vitest";
import {
  availableSources,
  sameSource,
  mergeSources,
  sourceLabel,
  checkedSources,
  scopeFiles,
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
  it("prunes absent rows and remembers ticks when their paths return", () => {
    const rows = found.map((source, i) => ({
      ...source,
      checked: i === 0,
      offline: false,
    }));
    const checks = rememberMounted(rows);
    expect(mergeSources(rows, [], checks)).toEqual([]);
    const savedChecks = JSON.parse(JSON.stringify(checks));
    const returned = mergeSources([], [...found, found[0]], savedChecks);
    expect(returned.map((row) => row.checked)).toEqual([true, false]);
    expect(returned.every((row) => !row.offline)).toBe(true);
    expect(mergeSources(returned, found, savedChecks)).toEqual(returned);
    const renamed = { ...found[1], path: found[1].path.toUpperCase() };
    expect(mergeSources([], [renamed], savedChecks)[0].checked).toBe(false);
    expect(
      mergeSources(
        [],
        [{ ...found[0], path: "/Volumes/New-Island/MXF/3" }],
        savedChecks,
      )[0].checked,
    ).toBe(true);
  });
  it("excludes offline checked rows without changing their ticks", () => {
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
    ).toEqual([]);
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

describe("single-line middle ellipsis", () => {
  const measure = (text: string) => text.length;
  it("leaves fitting paths intact", () => {
    expect(middleEllipsis("E:/MXF/1", 40, measure)).toBe("E:/MXF/1");
  });
  it("keeps the Windows drive and final MXF folder", () => {
    const path = "E:\\Avid MediaFiles\\MXF\\desktop.1";
    const label = middleEllipsis(path, 24, measure);
    expect(label.startsWith("E:\\")).toBe(true);
    expect(label.endsWith("\\MXF\\desktop.1")).toBe(true);
    expect(label).toContain("…");
    expect(measure(label)).toBeLessThanOrEqual(24);
  });
  it("keeps the Mac volume and custom folder ending", () => {
    const label = middleEllipsis(
      "Story-Island · /Volumes/Story-Island/Project Directory/Backup files/MXF/1",
      30,
      measure,
    );
    expect(label.startsWith("Story-Island")).toBe(true);
    expect(label.endsWith("/MXF/1")).toBe(true);
    expect(
      middleEllipsis("E:/Long Project Directory/Selected Media", 24, measure),
    ).toMatch(/^E:.*….*\/Selected Media$/);
  });
});

it("reports none, some, and all project ticks, ignoring stale project names", () => {
  expect(projectCheckState([], [])).toEqual({
    checked: false,
    indeterminate: false,
  });
  expect(projectCheckState(["A", "B"], [])).toEqual({
    checked: false,
    indeterminate: false,
  });
  expect(projectCheckState(["A", "B"], ["A", "Gone"])).toEqual({
    checked: false,
    indeterminate: true,
  });
  expect(projectCheckState(["A", "B"], ["B", "A"])).toEqual({
    checked: true,
    indeterminate: false,
  });
});

it("uses a deduped union with path boundaries and Windows aliases, and reticks without rescanning", () => {
  const files = [
    { path: "E:/Story/Media/a.mxf" },
    { path: "e:\\Story\\Media\\a.mxf" },
    { path: "E:/Story/Media2/b.mxf" },
  ];
  const rows = [
    { path: "E:/Story", checked: true, offline: false, volume: "" },
    { path: "E:/Story/Media", checked: true, offline: false, volume: "" },
  ];
  expect(checkedSources({ mounted_sources: rows, custom_folders: [] })).toEqual(
    ["E:/Story"],
  );
  expect(scopeFiles(files, ["E:/Story/Media"])).toEqual([files[0]]);
  expect(scopeFiles(files, [])).toEqual([]);
  expect(scopeFiles(files, ["E:/Story/Media"])).toEqual([files[0]]);
  expect(files).toHaveLength(3);
});

describe("drive groups and rail width", () => {
  const row = (path: string, checked = true, volume = "Story Island") => ({
    path,
    checked,
    volume,
    offline: false,
  });
  it("groups by mount identity across Windows aliases and keeps distinct equally named volumes apart", () => {
    const groups = groupSources([
      row("E:/Avid MediaFiles/MXF/1"),
      row("\\\\?\\E:\\Avid MediaFiles\\MXF\\desktop.1", false),
      row("F:/Avid MediaFiles/MXF/1"),
      row("//Paper/Share/Avid MediaFiles/MXF/1"),
      row("//Paper/Share/Avid MediaFiles/MXF/2"),
      row("/Volumes/Amber/Avid MediaFiles/MXF/1"),
      row("/Volumes/Amber/Avid MediaFiles/MXF/2"),
    ]);
    expect(groups.map((group) => group.rows.length)).toEqual([2, 1, 2, 2]);
    expect(driveCheckState(groups[0].rows)).toEqual({
      checked: false,
      indeterminate: true,
    });
    const checked = toggleDrive(
      groups.flatMap((group) => group.rows),
      "e:",
      true,
    );
    expect(driveCheckState(checked.slice(0, 2))).toEqual({
      checked: true,
      indeterminate: false,
    });
    expect(
      driveCheckState(toggleDrive(checked, "e:", false).slice(0, 2)),
    ).toEqual({ checked: false, indeterminate: false });
    expect(checked.slice(2)).toEqual(
      groups.slice(1).flatMap((group) => group.rows),
    );
  });
  it("inherits all, some, or none only for new paths and keeps explicit path memory", () => {
    const paths = ["E:/Avid MediaFiles/MXF/1", "E:/Avid MediaFiles/MXF/2"];
    const discovered = [...paths, "E:/Avid MediaFiles/MXF/3"].map((path) => ({
      path,
      volume: "Story Island",
      available: 0,
      total: 0,
    }));
    expect(
      mergeSources(
        paths.map((p) => row(p)),
        discovered,
      ).map((r) => r.checked),
    ).toEqual([true, true, true]);
    expect(
      mergeSources(
        paths.map((p, i) => row(p, i === 0)),
        discovered,
      ).map((r) => r.checked),
    ).toEqual([true, false, false]);
    expect(
      mergeSources(
        paths.map((p) => row(p, false)),
        discovered,
      ).map((r) => r.checked),
    ).toEqual([false, false, false]);
    expect(
      mergeSources(
        paths.map((p) => row(p)),
        discovered,
        { [pathKey(discovered[2].path)]: false },
      )[2].checked,
    ).toBe(false);
    expect(shortSourceLabel(discovered[2].path, true)).toBe("MXF/3");
    expect(
      shortSourceLabel("/Volumes/Story/Backup/Media/Selected", false),
    ).toBe("Backup/Media/Selected");
  });
  it("clamps widths, restores persisted values, resets, and handles unavailable storage", () => {
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => {
        values.set(key, value);
      },
    };
    expect(readRailWidth(storage, 1200)).toBe(DEFAULT_RAIL_WIDTH);
    expect(saveRailWidth(storage, 900, 1200)).toBe(540);
    expect(readRailWidth(storage, 1200)).toBe(540);
    expect(readRailWidth(storage, 800)).toBe(360);
    expect(saveRailWidth(storage, 1, 1200)).toBe(MIN_RAIL_WIDTH);
    expect(saveRailWidth(storage, DEFAULT_RAIL_WIDTH, 1200)).toBe(320);
    values.set(RAIL_WIDTH_KEY, "broken");
    expect(readRailWidth(storage, 1200)).toBe(320);
    expect(clampRailWidth(NaN, 1200)).toBe(320);
    expect(
      readRailWidth(
        {
          getItem: () => {
            throw Error();
          },
        },
        1200,
      ),
    ).toBe(320);
    expect(
      saveRailWidth(
        {
          setItem: () => {
            throw Error();
          },
        },
        400,
        1200,
      ),
    ).toBe(400);
  });
});
