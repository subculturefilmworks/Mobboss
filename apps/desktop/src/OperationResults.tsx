export interface ResultRow {
  op: string;
  source: string;
  dest: string;
  checksum: string;
  status: string;
  verified: string;
  algorithm: string;
}
export function OperationResults({
  result,
}: {
  result: {
    completed: number;
    skipped: number;
    cancelled: boolean;
    rows: ResultRow[];
  };
}) {
  const rows = result.rows;
  const verified = rows.filter((r) => r.verified === "yes").length;
  const renamed = rows.filter(
    (r) => r.verified === "not needed (same-drive rename)",
  ).length;
  return (
    <>
      <p className="result-count">
        {result.completed} completed · {verified} verified · {result.skipped}{" "}
        skipped
        {result.cancelled ? " · Cancelled" : ""}
      </p>
      {renamed > 0 && <p>{renamed} moved on same drive (no copy needed)</p>}
      <details open={rows.length <= 50}>
        <summary>Per-file outcomes ({rows.length})</summary>
        <ul className="operation-results">
          {rows.map((row, i) => {
            const failed = row.status.startsWith("skipped:");
            const verificationFailed = row.status.includes(
              "Copy failed verification",
            );
            const badge = failed
              ? verificationFailed
                ? "✗ Copy failed verification — original kept"
                : `✗ ${row.status}`
              : row.verified === "yes"
                ? "✓ Verified"
                : row.verified === "matched existing"
                  ? "Already there ✓"
                  : row.verified === "not needed (same-drive rename)"
                    ? "Renamed"
                    : row.op === "undo" && row.status === "done"
                      ? "Undone"
                      : row.status;
            return (
              <li key={i}>
                <span title={row.source}>
                  {row.source.split(/[\\/]/).pop()}
                </span>
                {" · "}
                <strong
                  className={failed ? "operation-failed" : "operation-badge"}
                >
                  {badge}
                </strong>{" "}
                {row.checksum && (
                  <code title={`${row.algorithm}: ${row.checksum}`}>
                    {row.checksum.slice(0, 12)}
                  </code>
                )}
              </li>
            );
          })}
        </ul>
      </details>
    </>
  );
}
