import { describe, it, expect } from "vitest";
import {
  bytes,
  duration,
  filter,
  row,
  projects,
  csv,
  summaryHtml,
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
  it("uses binary units and descriptor edit-rate duration, including audio samples", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1024)).toBe("1 KiB");
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
