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
  start_timecode?: number | null;
  timecode_fps?: number | null;
  timecode_drop?: boolean | null;
  track_id?: number | null;
  timecode_rate?: Rational | null;
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
  preview_cache_bytes: number;
}
export interface Volume {
  name: string;
  path: string;
  available: number;
  total: number;
}
export const filename = (path: string) => path.split(/[\\/]/).pop() || path;
export function groupedInteger(
  value: number,
  locales?: string | string[],
): string {
  try {
    const formatter = new Intl.NumberFormat(locales, {
      maximumFractionDigits: 0,
    });
    if (formatter.formatToParts(10000).some((part) => part.type === "group"))
      return formatter.format(value);
  } catch {
    // Some headless C-locale webviews lack working Intl grouping.
  }
  const fallback = value.toLocaleString("en-US", {
    maximumFractionDigits: 0,
    useGrouping: true,
  });
  if (Math.abs(value) < 1000 || fallback.includes(",")) return fallback;
  return Math.round(value)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}
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
export function summaryHtml(
  files: MediaFile[],
  thumbnails?: Record<string, string>,
) {
  const clipTable = thumbnails
    ? `<h2>Clips</h2><table><thead><tr><th>Poster</th><th>Clip</th><th>Project</th><th>Tracks</th><th>Duration</th><th>Start TC</th></tr></thead><tbody>${groupClips(
        files,
      )
        .map((c) => {
          const src = thumbnails[c.id];
          const poster =
            src && /^data:image\/jpeg;base64,[A-Za-z0-9+/=]+$/.test(src)
              ? `<img width="160" alt="Clip poster" src="${src}">`
              : "Unavailable";
          return `<tr><td>${poster}</td><td>${escapeHtml(c.name)}</td><td>${escapeHtml(c.metadata.project || "Unknown")}</td><td>${escapeHtml(c.tracks)}</td><td>${duration(c.metadata)}</td><td>${timecode(c.metadata)}</td></tr>`;
        })
        .join("")}</tbody></table>`
    : "";

  return `<!doctype html><html lang="en"><meta charset="utf-8"><title>Mob Boss project summary</title><style>body{font:16px system-ui;margin:3rem}table{border-collapse:collapse}td,th{padding:12px;border-bottom:1px solid #ccc;text-align:left}img{display:block;max-width:160px}tr{break-inside:avoid}@media print{body{margin:0;font-size:11px}button{display:none}thead{display:table-header-group}td,th{padding:6px}}</style><button onclick="window.print()">Print / Save as PDF</button><h1>Mob Boss · Projects</h1><table><thead><tr><th>Project</th><th>Files</th><th>Size</th></tr></thead><tbody>${projects(
    files,
  )
    .map(
      (p) =>
        `<tr><td>${escapeHtml(p.name)}</td><td>${p.count}</td><td>${bytes(p.size)} (${p.size} bytes)</td></tr>`,
    )
    .join("")}</tbody></table>${clipTable}</html>`;
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

export interface Clip {
  id: string;
  name: string;
  files: MediaFile[];
  metadata: Metadata;
  size: number;
  tracks: string;
}
export const clipSortFields = [
  ["name", "Name"],
  ["project", "Project"],
  ["created", "Created"],
  ["modified", "Modified"],
  ["duration", "Duration"],
  ["size", "Size"],
  ["timecode", "Start TC"],
  ["tracks", "Tracks"],
] as const;
export type ClipSortKey = (typeof clipSortFields)[number][0];
export interface ClipSort {
  key: ClipSortKey;
  desc: boolean;
}
export function sortClips(clips: Clip[], sort: ClipSort): Clip[] {
  const key = clipSortFields.some(([key]) => key === sort?.key)
    ? sort.key
    : "name";
  const value = (clip: Clip): string | number | undefined => {
    const m = clip.metadata;
    const rate = (r?: Rational | null) =>
      r && r.numerator > 0 && r.denominator > 0
        ? r.numerator / r.denominator
        : undefined;
    switch (key) {
      case "name":
        return clip.name;
      case "project":
        return m.project || undefined;
      case "tracks":
        return clip.tracks || undefined;
      case "size":
        return clip.size;
      case "created":
        return m.created ? Date.parse(m.created) : undefined;
      case "modified":
        return m.modified
          ? Date.parse(m.modified)
          : Math.max(...clip.files.map((f) => Number(f.modified_ns) / 1e6));
      case "duration": {
        const fps = rate(m.edit_rate);
        return m.duration != null && fps ? m.duration / fps : undefined;
      }
      case "timecode": {
        const fps =
          rate(m.timecode_rate) || m.timecode_fps || rate(m.edit_rate);
        return m.start_timecode != null && fps
          ? m.start_timecode / fps
          : undefined;
      }
    }
  };
  const missing = (v: unknown) =>
    v == null || (typeof v === "number" && !Number.isFinite(v));
  return [...clips].sort((a, b) => {
    const x = value(a),
      y = value(b);
    if (missing(x) !== missing(y)) return missing(x) ? 1 : -1;
    const result = missing(x)
      ? 0
      : typeof x === "number" && typeof y === "number"
        ? x - y
        : String(x).localeCompare(String(y), undefined, {
            numeric: true,
            sensitivity: "base",
          });
    return (
      (sort?.desc ? -result : result) ||
      a.name.localeCompare(b.name, undefined, {
        numeric: true,
        sensitivity: "base",
      }) ||
      a.id.localeCompare(b.id)
    );
  });
}
export function groupClips(files: MediaFile[]): Clip[] {
  const groups = new Map<string, MediaFile[]>();
  for (const file of files) {
    const id = file.metadata.material_package_uid
      ? `uid:${file.metadata.material_package_uid}`
      : `file:${file.path}`;
    groups.set(id, [...(groups.get(id) || []), file]);
  }
  return [...groups].map(([id, members]) => {
    members.sort(
      (a, b) =>
        (a.metadata.track_id ?? 0) - (b.metadata.track_id ?? 0) ||
        (a.path < b.path ? -1 : a.path > b.path ? 1 : 0),
    );
    const first =
      members.find((f) => f.metadata.track_kind === "video") || members[0];
    const tracks = (
      [
        ["video", "V"],
        ["audio", "A"],
        ["data", "D"],
      ] as const
    )
      .flatMap(([kind, prefix]) => {
        const n = new Set(
          members
            .filter((f) => f.metadata.track_kind === kind)
            .map((f) => f.metadata.file_package_uid || f.path),
        ).size;
        return n ? [n === 1 ? `${prefix}1` : `${prefix}1–${prefix}${n}`] : [];
      })
      .join(" + ");
    return {
      id,
      name: first.metadata.clip_name || filename(first.path),
      files: members,
      metadata: first.metadata,
      size: totalSize(members),
      tracks,
    };
  });
}
export function selectClip(
  selected: Set<string>,
  clip: Clip,
  additive = true,
): Set<string> {
  const next = additive ? new Set(selected) : new Set<string>();
  const remove = additive && clip.files.every((f) => selected.has(f.path));
  clip.files.forEach((f) => (remove ? next.delete(f.path) : next.add(f.path)));
  return next;
}
export function timecode(m: Metadata, seconds = 0): string {
  if (m.start_timecode == null || !m.timecode_fps) return "—";
  const fps = m.timecode_fps;
  const r = m.timecode_rate || (m.track_kind !== "audio" ? m.edit_rate : null);
  const rate = r && r.denominator > 0 ? r.numerator / r.denominator : fps;
  let frames = Math.max(
    0,
    m.start_timecode + Math.floor(seconds * rate + 0.0001),
  );
  if (m.timecode_drop && (fps === 30 || fps === 60)) {
    const drop = fps === 60 ? 4 : 2,
      ten = fps * 600 - drop * 9;
    const d = Math.floor(frames / ten),
      rem = frames % ten;
    frames +=
      drop * 9 * d +
      (rem >= drop ? drop * Math.floor((rem - drop) / (fps * 60 - drop)) : 0);
  }
  const parts = [
    Math.floor(frames / (fps * 3600)) % 24,
    Math.floor(frames / (fps * 60)) % 60,
    Math.floor(frames / fps) % 60,
    frames % fps,
  ].map((n) => String(n).padStart(2, "0"));
  return `${parts.slice(0, 3).join(":")}${m.timecode_drop ? ";" : ":"}${parts[3]}`;
}
