import { bytes, filename, type Report } from "./model";
import { shortSourceLabel } from "./sources";
export default function ScanDetails({ reports }: { reports: Report[] }) {
  const count = reports.filter((report) => report.reconciliation).length;
  if (!count) return null;
  return (
    <details className="scan-details">
      <summary>
        Scan details · {count} {count === 1 ? "folder" : "folders"}
      </summary>
      <div className="scan-details-scroll">
        {reports.map((report, i) => {
          const r = report.reconciliation;
          return (
            r && (
              <details key={i} className="reconciliation">
                <summary title={r.root}>
                  {shortSourceLabel(r.root, false)}:{" "}
                  {r.media_files.toLocaleString()} media files ·{" "}
                  {r.database_files.length} Avid database files (.pmr/.mdb) ·{" "}
                  {r.other_files} other files · {r.folders} folders — total{" "}
                  <span title={`${r.total_bytes.toLocaleString()} bytes`}>
                    {bytes(r.total_bytes)}
                  </span>
                </summary>
                {r.database_files.map((db) => (
                  <p
                    key={db.path}
                    title={`${db.path} · ${db.description} · Modified ${db.modified_ns ? new Date(Number(db.modified_ns) / 1e6).toLocaleString() : "unknown"}`}
                  >
                    {filename(db.path)} ·{" "}
                    {db.path.toLowerCase().endsWith(".mdb")
                      ? "Avid media database"
                      : "Avid media index"}{" "}
                    ·{" "}
                    <span title={`${db.size.toLocaleString()} bytes`}>
                      {bytes(db.size)}
                    </span>
                    {db.pmr_missing !== null && (
                      <>
                        {" "}
                        · PMR vs MXF:{" "}
                        {db.pmr_missing === 0 && db.pmr_extra === 0
                          ? "agree"
                          : `${db.pmr_missing} missing from disk / ${db.pmr_extra} extra on disk`}
                      </>
                    )}
                  </p>
                ))}
                <details>
                  <summary>
                    Other files ({r.other_files}; showing up to 200)
                  </summary>
                  <pre>{r.other_names.join("\n")}</pre>
                </details>
              </details>
            )
          );
        })}
      </div>
    </details>
  );
}
