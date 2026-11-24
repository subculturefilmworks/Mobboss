import { describe, it, expect, vi } from "vitest";
import {
  bytes,
  groupedInteger,
  groupClips,
  isAudioOnly,
  sortClips,
  requiresDestination,
  duration,
  filter,
  row,
  projects,
  csv,
  summaryHtml,
  summaryCsv,
  tableCsv,
  diffRows,
  type MediaFile,
  type Filters,
} from "./model";
const file = (path: string, project: string, size = 1024): MediaFile => ({
  path,
  folder: "/media",
  size,
  modified_ns: "1700000000000000000",
  metadata: {
    project,
    codec: "DNxHR SQ",
    track_kind: "video",
    clip_name: "Clip",
  },
});
const defaults: Filters = {
  search: "",
  projects: [],
  track: "",
  codec: "",
  folder: "",
  diff: "",
};
describe("formatting and exports", () => {
  it("marks only clips containing audio and no picture as audio-only", () => {
    const a = file("/media/audio.mxf", "Sample");
    a.metadata.track_kind = "audio";
    a.metadata.material_package_uid = "joined";
    const v = file("/media/video.mxf", "Sample");
    v.metadata.material_package_uid = "joined";
    expect(isAudioOnly(groupClips([a])[0])).toBe(true);
    expect(isAudioOnly(groupClips([v])[0])).toBe(false);
    expect(isAudioOnly(groupClips([a, v])[0])).toBe(false);
    const unknown = file("/media/unknown.mxf", "Sample");
    unknown.metadata.track_kind = null;
    expect(isAudioOnly(groupClips([unknown])[0])).toBe(false);
  });
  it("uses platform units and descriptor edit-rate duration, including audio samples", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1000, "MacIntel")).toBe("1 KB");
    expect(bytes(1024, "Win32")).toBe("1 KB");
    const finderBytes = 315636807021;
    expect(bytes(finderBytes, "MacIntel")).toBe("315.64 GB");
    expect(bytes(finderBytes, "Win32")).toBe("293.96 GB");
    expect(2449 + 8 + 5 + 4).toBe(2466);
    expect((293.94 * 1024 ** 3) / 1000 ** 3).toBeCloseTo(315.62, 2);
    expect(
      duration({
        duration: 198,
        edit_rate: { numerator: 24000, denominator: 1001 },
      }),
    ).toBe("00:00:08.258");
    expect(
      duration({
        duration: 690690,
        edit_rate: { numerator: 48000, denominator: 1 },
      }),
    ).toBe("00:00:14.389");
    expect(
      duration({ duration: 1, edit_rate: { numerator: 0, denominator: 1 } }),
    ).toBe("");
  });
  it("retains exact byte counts in HTML and CSV exports", () => {
    const f = file("/media/a.mxf", "Project", 315636807021);
    expect(summaryHtml([f])).toContain("315636807021 bytes");
    expect(summaryCsv([f])).toContain("315636807021");
    expect(tableCsv([row(f)])).toContain("315636807021");
  });
  it("quotes CSV, preserves newlines, and neutralizes spreadsheet formulas", () => {
    expect(csv([['a,"b\nc', "=SUM(A1)", 4]])).toBe(
      '"a,""b\nc","\'=SUM(A1)","4"\r\n',
    );
    expect(
      summaryHtml([file("/a", "<script>alert(1)</script>")]),
    ).not.toContain("<script>");
  });
});
describe("table logic", () => {
  const files = [
    file("/media/a.mxf", "First"),
    file("/media/b.mxf", "Second", 2048),
  ];
  it("searches all columns, including hidden raw identifiers, and intersects filters", () => {
    files[0].metadata.file_package_uid = "ABCDEF1234";
    const rows = files.map((f) => row(f));
    expect(
      filter(rows, { ...defaults, search: "abcdef" }).map((r) => r.file.path),
    ).toEqual(["/media/a.mxf"]);
    expect(
      filter(rows, {
        ...defaults,
        projects: ["Second"],
        codec: "DNxHR SQ",
        track: "video",
        folder: "/media",
      }),
    ).toHaveLength(1);
    expect(filter(rows, { ...defaults, folder: "/med" })).toHaveLength(0);
    expect(
      filter(rows, { ...defaults, projects: ["First"], track: "audio" }),
    ).toHaveLength(0);
  });
  it("aggregates project sizes and represents missing paths after a scan", () => {
    expect(projects(files).map((p) => p.size)).toEqual([1024, 2048]);
    const rows = diffRows(files, [
      {
        files: 2,
        elapsed_seconds: 1,
        warnings: [],
        diff: {
          missing: ["/media/gone.mxf"],
          changed: ["/media/a.mxf"],
          unlisted: ["/media/b.mxf"],
        },
      },
    ]);
    expect(filter(rows, { ...defaults, diff: "all" })).toHaveLength(3);
    expect(filter(rows, { ...defaults, diff: "missing" })[0].file.path).toBe(
      "/media/gone.mxf",
    );
  });
  it("filters 50,000 rows with stable file identities", () => {
    const rows = Array.from({ length: 50000 }, (_, i) =>
      row(file(`/media/${i}.mxf`, i % 2 ? "A" : "B")),
    );
    expect(filter(rows, { ...defaults, projects: ["A"] })).toHaveLength(25000);
    expect(new Set(rows.map((r) => r.file.path)).size).toBe(50000);
  });
});

it("retains indexed project and size for a missing diff row", () => {
  const gone = file("/media/gone.mxf", "Archive", 4096);
  const rows = diffRows(
    [],
    [
      {
        files: 0,
        elapsed_seconds: 1,
        warnings: [],
        diff: { missing: [gone.path], changed: [], unlisted: [] },
      },
    ],
    [gone],
  );
  expect(rows[0].values.project).toBe("Archive");
  expect(rows[0].values.size).toBe(4096);
});

describe("operation destination requirements", () => {
  it("requires an explicit root for unknown layouts on Unix, Windows, or mixed selections", () => {
    const unix = file("/Volumes/Drive/Avid MediaFiles/MXF/1/a.mxf", "Film");
    const windows = file("D:\\Avid MediaFiles\\MXF\\1\\a.mxf", "Film");
    const arbitrary = file("/System/Volumes/Data/editorial/a.mxf", "Film");
    expect(requiresDestination([unix, windows])).toBe(false);
    expect(requiresDestination([arbitrary])).toBe(true);
    expect(requiresDestination([unix, arbitrary])).toBe(true);
    expect(requiresDestination([file("D:\\editorial\\a.mxf", "Film")])).toBe(
      true,
    );
  });
});

it("groups only UIDs and selects the complete clip even when members were filtered out", async () => {
  const { groupClips, selectClip, timecode } = await import("./model");
  const v = file("/media/v", "Film"),
    a = file("/media/a", "Film"),
    unknown = file("/media/u", "Film");
  v.metadata.material_package_uid = a.metadata.material_package_uid = "same";
  a.metadata.track_kind = "audio";
  const clips = groupClips([v, a, unknown]);
  expect(clips).toHaveLength(2);
  const clip = clips.find((c) => c.files.length === 2)!;
  expect(clip.tracks).toBe("V1 + A1");
  expect([...selectClip(new Set(), clip)]).toEqual([a.path, v.path]);
  expect(selectClip(new Set([a.path]), clip).size).toBe(2);
  expect(selectClip(new Set([a.path, v.path]), clip).size).toBe(0);
  expect(timecode({ start_timecode: 86400, timecode_fps: 24 })).toBe(
    "01:00:00:00",
  );
  expect(
    timecode({ start_timecode: 1800, timecode_fps: 30, timecode_drop: true }),
  ).toBe("00:01:00;02");
  expect(timecode({})).toBe("—");
});

it("HTML thumbnails are opt-in, one per clip, constrained to JPEG data, and printable", async () => {
  const { groupClips } = await import("./model");
  const a = file("/media/a", "<Film>"),
    b = file("/media/b", "<Film>");
  a.metadata.material_package_uid = b.metadata.material_package_uid = "one";
  const id = groupClips([a, b])[0].id;
  expect(summaryHtml([a, b])).not.toContain("<img");
  const html = summaryHtml([a, b], { [id]: "data:image/jpeg;base64,/9j/AA==" });
  expect(html.match(/<img/g)).toHaveLength(1);
  expect(html).toContain("Print / Save as PDF");
  expect(html).toContain("@media print");
  expect(html).toContain("&lt;Film&gt;");
  expect(summaryHtml([a], { [id]: "file:///private/image.jpg" })).not.toContain(
    "<img",
  );
});

describe("reconciliation number grouping", () => {
  it("preserves the requested locale's grouping", () => {
    expect(groupedInteger(23200945232, "en-US")).toBe("23,200,945,232");
    expect(groupedInteger(23200945232, "de-DE")).toBe("23.200.945.232");
    expect(groupedInteger(23200945232, "en-IN")).toBe("23,20,09,45,232");
  });
  it("falls back when the runtime has no grouping, including a C-locale webview", () => {
    const parts = vi
      .spyOn(Intl.NumberFormat.prototype, "formatToParts")
      .mockReturnValue([{ type: "integer", value: "10000" }]);
    try {
      expect(groupedInteger(23200945232)).toBe("23,200,945,232");
      const legacy = vi
        .spyOn(Number.prototype, "toLocaleString")
        .mockImplementation(function (this: number) {
          return String(this.valueOf());
        });
      try {
        expect(groupedInteger(23200945232)).toBe("23,200,945,232");
        expect(groupedInteger(0)).toBe("0");
      } finally {
        legacy.mockRestore();
      }
    } finally {
      parts.mockRestore();
    }
  });
});

describe("clip sorting", () => {
  it("sorts names naturally in both directions without changing members", () => {
    const files = [file("/media/2", "A"), file("/media/10", "B")];
    files[0].metadata.clip_name = "Clip 2";
    files[1].metadata.clip_name = "Clip 10";
    const clips = groupClips(files);
    expect(
      sortClips(clips, { key: "name", desc: false }).map((c) => c.name),
    ).toEqual(["Clip 2", "Clip 10"]);
    expect(
      sortClips(clips, { key: "name", desc: true }).map((c) => c.name),
    ).toEqual(["Clip 10", "Clip 2"]);
    expect(clips[0].files).toEqual([files[0]]);
  });
  it("compares duration and start TC by their actual rates, keeping unknowns last", () => {
    const files = [
      file("/media/a", "A"),
      file("/media/b", "B"),
      file("/media/c", "C"),
    ];
    Object.assign(files[0].metadata, {
      duration: 50,
      edit_rate: { numerator: 25, denominator: 1 },
      start_timecode: 50,
      timecode_fps: 25,
    });
    Object.assign(files[1].metadata, {
      duration: 60,
      edit_rate: { numerator: 60, denominator: 1 },
      start_timecode: 60,
      timecode_fps: 60,
    });
    const clips = groupClips(files);
    for (const key of ["duration", "timecode"] as const) {
      expect(sortClips(clips, { key, desc: false }).map((c) => c.id)).toEqual([
        "file:/media/b",
        "file:/media/a",
        "file:/media/c",
      ]);
      expect(sortClips(clips, { key, desc: true }).map((c) => c.id)).toEqual([
        "file:/media/a",
        "file:/media/b",
        "file:/media/c",
      ]);
    }
  });
  it("sorts project, size and dates and breaks ties deterministically", () => {
    const files = [file("/media/b", "B", 20), file("/media/a", "A", 10)];
    Object.assign(files[0].metadata, {
      created: "2025-01-01",
      modified: "2026-01-01",
    });
    Object.assign(files[1].metadata, {
      created: "2024-01-01",
      modified: "2025-01-01",
    });
    const clips = groupClips(files);
    for (const key of [
      "project",
      "size",
      "created",
      "modified",
      "name",
    ] as const)
      expect(sortClips(clips, { key, desc: false }).map((c) => c.id)).toEqual([
        "file:/media/a",
        "file:/media/b",
      ]);
  });
});
