import { expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import ScanDetails from "./ScanDetails";
import { bytes, type Report } from "./model";

const report: Report = {
  files: 2,
  elapsed_seconds: 1,
  warnings: [],
  diff: { missing: [], unlisted: [], changed: [] },
  reconciliation: {
    root: "/Volumes/Story-Island/Avid MediaFiles/MXF/EDIT-BAY-1.1",
    media_files: 2,
    folders: 1,
    total_bytes: 1234567890,
    other_files: 1,
    other_names: ["notes.txt"],
    database_files: [
      {
        path: "/Volumes/Story-Island/Avid MediaFiles/MXF/EDIT-BAY-1.1/msmFMID.pmr",
        size: 1024,
        modified_ns: "0",
        description: "Index",
        pmr_missing: 0,
        pmr_extra: 0,
      },
    ],
  },
};
const render = (reports: Report[]) =>
  renderToStaticMarkup(<ScanDetails reports={reports} />);

it("counts reconciled folders and starts collapsed, with singular and plural summaries", () => {
  expect(render([])).toBe("");
  expect(render([{ ...report, reconciliation: undefined }])).toBe("");
  expect(render([report])).toContain("Scan details · 1 folder<");
  expect(
    render([report, report, { ...report, reconciliation: undefined }]),
  ).toContain("Scan details · 2 folders<");
  expect(render([report])).not.toContain(" open=");
});

it("preserves folder diagnostics, full path and exact size tooltips", () => {
  const html = render([report]);
  expect(html).toContain(`title="${report.reconciliation!.root}"`);
  expect(html).toContain(bytes(1234567890));
  expect(html).toContain(`title="${(1234567890).toLocaleString()} bytes"`);
  expect(html).toContain("PMR vs MXF: agree");
  expect(html).toContain("Other files (1; showing up to 200)");
  expect(html).toContain("notes.txt");
  const mismatch = structuredClone(report);
  mismatch.reconciliation!.database_files[0].pmr_missing = 2;
  mismatch.reconciliation!.database_files[0].pmr_extra = 3;
  expect(render([mismatch])).toContain("2 missing from disk / 3 extra on disk");
});
