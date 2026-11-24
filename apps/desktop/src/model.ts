export interface Rational {
  numerator: number;
  denominator: number;
}
export interface Metadata {
  material_package_uid?: string | null;
  file_package_uid?: string | null;
  clip_name?: string | null;
  project?: string | null;
  track_kind?: string | null;
  codec?: string | null;
  codec_ul?: string | null;
  edit_rate?: Rational | null;
  sample_rate?: Rational | null;
  duration?: number | null;
  width?: number | null;
  height?: number | null;
  bit_depth?: number | null;
  created?: string | null;
  modified?: string | null;
  kind?: string | null;
}
export interface MediaFile {
  path: string;
  folder: string;
  size: number;
  modified_ns: string;
  metadata: Metadata;
}
export interface Diff {
  missing: string[];
  unlisted: string[];
  changed: string[];
}
export interface Reconciliation {
  root: string;
  media_files: number;
  database_files: {
    path: string;
    size: number;
    modified_ns: string;
    description: string;
    pmr_missing: number | null;
    pmr_extra: number | null;
  }[];
  other_files: number;
  other_names: string[];
  folders: number;
  total_bytes: number;
}
export interface Report {
  reconciliation?: Reconciliation;
  files: number;
  elapsed_seconds: number;
  warnings: string[];
  diff: Diff;
}
export interface Progress {
  phase: "discovery" | "inventory" | "enrichment" | "complete";
  done: number;
  total: number;
  current_folder: string;
  files: MediaFile[];
}
export interface Settings {
  index: string;
  roots: string[];
  concurrency: number;
}
export interface Volume {
  name: string;
  path: string;
  available: number;
  total: number;
}
export const filename = (path: string) => path.split(/[\\/]/).pop() || path;
export function bytes(
  size: number,
  platform = globalThis.navigator?.platform || "",
): string {
  if (!size) return "0 B";
  const base = /win/i.test(platform) ? 1024 : 1000;
  const unit = Math.min(
    4,
    Math.floor(Math.log(Math.max(1, size)) / Math.log(base)),
  );
  return `${(size / base ** unit).toLocaleString(undefined, { maximumFractionDigits: unit ? 2 : 0 })} ${["B", "KB", "MB", "GB", "TB"][unit]}`;
}
export const rate = (r?: Rational | null) =>
  r && r.denominator > 0 ? `${r.numerator}/${r.denominator}` : "";
export function duration(m: Metadata): string {
  if (
    m.duration == null ||
    !m.edit_rate ||
    m.edit_rate.numerator <= 0 ||
    m.edit_rate.denominator <= 0
  )
    return "";
  const ms = Math.round(
    ((m.duration * m.edit_rate.denominator) / m.edit_rate.numerator) * 1000,
  );
  const sec = Math.floor(ms / 1000);
  return `${Math.floor(sec / 3600)
    .toString()
    .padStart(
      2,
      "0",
    )}:${Math.floor(sec / 60) % 60 < 10 ? "0" : ""}${Math.floor(sec / 60) % 60}:${(sec % 60).toString().padStart(2, "0")}.${(ms % 1000).toString().padStart(3, "0")}`;
}
export const fields = [
  ["filename", "Filename"],
  ["clip", "Clip name"],
  ["project", "Project"],
  ["size", "Size"],
  ["created", "Created"],
  ["modified", "Modified"],
  ["track", "Track"],
  ["codec", "Codec"],
  ["resolution", "Resolution"],
  ["rate", "Frame/edit rate"],
  ["sample", "Sample rate"],
  ["depth", "Bit depth"],
  ["duration", "Duration"],
  ["kind", "Kind"],
  ["folder", "Folder / volume"],
  ["path", "Path"],
  ["material", "Material UID"],
  ["uid", "File UID"],
  ["ul", "Raw codec UL"],
  ["diff", "Diff"],
] as const;
export type Field = (typeof fields)[number][0];
export interface Row {
  file: MediaFile;
  values: Record<Field, string | number>;
  search: string;
}
export function row(file: MediaFile, diff = ""): Row {
  const m = file.metadata;
  const values = {
    filename: filename(file.path),
    clip: m.clip_name || "",
    project: m.project || "Unknown",
    size: file.size,
    created: m.created || "",
    modified:
      m.modified ||
      (file.modified_ns
        ? new Date(Number(BigInt(file.modified_ns) / 1000000n)).toISOString()
        : ""),
    track: m.track_kind || "",
    codec: m.codec || "",
    resolution: m.width && m.height ? `${m.width} × ${m.height}` : "",
    rate: rate(m.edit_rate),
    sample: rate(m.sample_rate),
    depth: m.bit_depth || "",
    duration: duration(m),
    kind: m.kind || "",
    folder: file.folder,
    path: file.path,
    material: m.material_package_uid || "",
    uid: m.file_package_uid || "",
    ul: m.codec_ul || "",
    diff,
  };
  return {
    file,
    values,
    search: [...Object.values(values), bytes(file.size)]
      .join("\n")
      .toLocaleLowerCase(),
  };
}
export interface Filters {
  search: string;
  projects: string[];
  track: string;
  codec: string;
  folder: string;
  diff: string;
}
export function filter(rows: Row[], f: Filters) {
  const query = f.search.trim().toLocaleLowerCase();
  return rows.filter(
    (r) =>
      (!query || r.search.includes(query)) &&
      (!f.projects.length || f.projects.includes(String(r.values.project))) &&
      (!f.track || r.values.track === f.track) &&
      (!f.codec || r.values.codec === f.codec) &&
      (!f.folder ||
        r.file.folder === f.folder ||
        r.file.path.startsWith(
          f.folder.replace(/[\\/]$/, "") +
            (r.file.path.includes("\\") ? "\\" : "/"),
        )) &&
      (!f.diff ||
        (f.diff === "all" ? !!r.values.diff : r.values.diff === f.diff)),
  );
}
export function projects(files: MediaFile[]) {
  const summary = new Map<
    string,
    { name: string; count: number; size: number }
  >();
  for (const file of files) {
    const name = file.metadata.project || "Unknown";
    const p = summary.get(name) || { name, count: 0, size: 0 };
    p.count++;
    p.size += file.size;
    summary.set(name, p);
  }
  return [...summary.values()].sort((a, b) => a.name.localeCompare(b.name));
}
export const totalSize = (rows: MediaFile[]) =>
  rows.reduce((n, r) => n + r.size, 0);
// Spreadsheet formula escaping is applied before RFC 4180 quoting.
export function csv(records: (string | number)[][]) {
  return (
    records
      .map((record) =>
        record
          .map((value) => {
            let s = String(value);
            if (typeof value === "string" && /^[=+\-@\t\r]/.test(s))
              s = "'" + s;
            return `"${s.replaceAll('"', '""')}"`;
          })
          .join(","),
      )
      .join("\r\n") + "\r\n"
  );
}
export function tableCsv(rows: Row[]) {
  return csv([
    fields.map(([, label]) => label),
    ...rows.map((r) => fields.map(([key]) => r.values[key])),
  ]);
}
export function summaryCsv(files: MediaFile[]) {
  return csv([
    ["Project", "Files", "Bytes"],
    ...projects(files).map((p) => [p.name, p.count, p.size]),
  ]);
}
const escapeHtml = (s: string) =>
  s.replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
export function summaryHtml(files: MediaFile[]) {
  return `<!doctype html><html lang="en"><meta charset="utf-8"><title>Mob Boss project summary</title><style>body{font:16px system-ui;margin:3rem}table{border-collapse:collapse}td,th{padding:12px;border-bottom:1px solid #ccc;text-align:left}</style><h1>Mob Boss · Projects</h1><table><thead><tr><th>Project</th><th>Files</th><th>Size</th></tr></thead><tbody>${projects(
    files,
  )
    .map(
      (p) =>
        `<tr><td>${escapeHtml(p.name)}</td><td>${p.count}</td><td>${bytes(p.size)} (${p.size} bytes)</td></tr>`,
    )
    .join("")}</tbody></table></html>`;
}
export function diffRows(
  files: MediaFile[],
  reports: Report[],
  archived: MediaFile[] = [],
): Row[] {
  const statuses = new Map<string, string>();
  for (const r of reports) {
    for (const p of r.diff.unlisted) statuses.set(p, "unlisted");
    for (const p of r.diff.changed) statuses.set(p, "changed");
    for (const p of r.diff.missing) statuses.set(p, "missing");
  }
  const present = new Set(files.map((f) => f.path));
  const history = new Map(archived.map((f) => [f.path, f]));
  return [
    ...files.map((f) => row(f, statuses.get(f.path))),
    ...[...statuses]
      .filter(([p, s]) => s === "missing" && !present.has(p))
      .map(([path, status]) =>
        row(
          history.get(path) || {
            path,
            folder: path.replace(/[\\/][^\\/]+$/, ""),
            size: 0,
            modified_ns: "",
            metadata: {},
          },
          status,
        ),
      ),
  ];
}

/** Unknown source layouts must never imply the system volume as a destination. */
export function requiresDestination(files: MediaFile[]): boolean {
  return files.some(
    (f) =>
      !f.path
        .split(/[\\/]/)
        .some((part) => part.toLowerCase() === "avid mediafiles"),
  );
}
