import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { OperationResults, type ResultRow } from "./OperationResults";
const hash = "abcdef0123456789".repeat(4);
const row = (verified: string, extra: Partial<ResultRow> = {}): ResultRow => ({
  op: "copy",
  source: "/fictional/Example.mxf",
  dest: "/fictional-backup/Example.mxf",
  checksum: hash,
  algorithm: "BLAKE3",
  status: "done",
  verified,
  ...extra,
});
const render = (rows: ResultRow[]) =>
  renderToStaticMarkup(
    <OperationResults
      result={{
        completed: 2,
        skipped: 1,
        cancelled: false,
        rows,
      }}
    />,
  );
describe("operation results", () => {
  it("shows verified copies, renames and matching existing files with checksum tooltips", () => {
    const html = render([
      row("yes"),
      row("not needed (same-drive rename)", { checksum: "", algorithm: "" }),
      row("matched existing", { status: "duplicate" }),
    ]);
    expect(html).toContain("2 completed · 1 verified · 1 skipped");
    expect(html).toContain("1 moved on same drive (no copy needed)");
    expect(html).toContain("✓ Verified");
    expect(html).toContain("Renamed");
    expect(html).toContain("Already there ✓");
    expect(html).toContain(`title="BLAKE3: ${hash}"`);
    expect(html).toContain(">abcdef012345</code>");
    expect(html).toContain("Example.mxf</span>");
    expect(html).toContain('<details open=""');
  });
  it("closes the file list by default above 50 rows", () => {
    expect(render(Array.from({ length: 50 }, () => row("yes")))).toContain(
      '<details open=""',
    );
    expect(render(Array.from({ length: 51 }, () => row("yes")))).not.toContain(
      "<details open=",
    );
  });
  it("shows verification failure with the original-kept warning and no verified badge", () => {
    const html = render([
      row("", {
        status: "skipped: Copy failed verification — original kept",
        checksum: "",
      }),
    ]);
    expect(html).toContain("✗ Copy failed verification — original kept");
    expect(html).not.toContain("✓ Verified");
    expect(html).toContain("0 verified");
  });
});
