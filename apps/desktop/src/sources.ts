export interface FoundSource {
  path: string;
  volume: string;
  available: number;
  total: number;
}
export interface Discovery {
  found: FoundSource[];
  denied: { name: string; path: string }[];
  online: string[];
}
// Match the backend path comparison, including Windows extended/UNC spelling.
export const pathKey = (p: string) =>
  p
    .replaceAll("\\", "/")
    .replace(/^\/\/\?\/UNC\//i, "//")
    .replace(/^\/\/\?\//, "")
    .replace(/\/$/, "")
    .toLowerCase();
export const sameSource = (a: string, b: string) => pathKey(a) === pathKey(b);
export const covered = (p: string, root: string) =>
  pathKey(p) === pathKey(root) || pathKey(p).startsWith(pathKey(root) + "/");
/** Listing is read-only. Covered sources and duplicate mount aliases are omitted. */
export function availableSources(
  found: FoundSource[],
  roots: string[],
): FoundSource[] {
  return found.filter(
    (source, i) =>
      !roots.some((root) => covered(source.path, root)) &&
      !found.slice(0, i).some((other) => sameSource(other.path, source.path)),
  );
}

export interface SourceRow {
  path: string;
  checked: boolean;
  offline: boolean;
  volume: string;
}
export function rememberMounted(
  rows: SourceRow[],
  checks: Record<string, boolean> = {},
) {
  return {
    ...checks,
    ...Object.fromEntries(rows.map((row) => [pathKey(row.path), row.checked])),
  };
}
export function mergeSources(
  rows: SourceRow[],
  found: FoundSource[],
  checks: Record<string, boolean> = {},
): SourceRow[] {
  const remembered = rememberMounted(rows, checks);
  const driveChecks = new Map<string, boolean>();
  for (const [path, checked] of Object.entries(remembered)) {
    const key = driveKey(path);
    driveChecks.set(key, (driveChecks.get(key) ?? true) && checked);
  }
  // Current rows describe the mounted drive; older absent paths keep their own ticks.
  for (const group of groupSources(rows))
    driveChecks.set(
      group.key,
      group.rows.every((row) => row.checked),
    );
  return found
    .filter(
      (source, i) =>
        !found.slice(0, i).some((other) => sameSource(other.path, source.path)),
    )
    .map((source) => ({
      path: source.path,
      volume: source.volume,
      checked:
        remembered[pathKey(source.path)] ??
        driveChecks.get(driveKey(source.path)) ??
        true,
      offline: false,
    }));
}
export function projectCheckState(names: string[], selected: string[]) {
  const count = names.filter((name) => selected.includes(name)).length;
  return {
    checked: names.length > 0 && count === names.length,
    indeterminate: count > 0 && count < names.length,
  };
}
/** Measure in pixels in the UI; tests can supply a deterministic character measure. */
export function middleEllipsis(
  label: string,
  width: number,
  measure: (text: string) => number,
): string {
  if (measure(label) <= width) return label;
  const tail =
    label.match(/[\\/]MXF[\\/][^\\/]+$/i)?.[0] ||
    label.match(/[\\/][^\\/]+$/)?.[0] ||
    label;
  const head = label.slice(0, label.length - tail.length);
  // Keep the drive/volume at the beginning and the complete MXF folder at the end.
  let length = head.length;
  while (length > 0 && measure(head.slice(0, length) + "…" + tail) > width)
    length--;
  return head.slice(0, length) + "…" + tail;
}
export const checkedSources = (settings: {
  mounted_sources: SourceRow[];
  custom_folders: SourceRow[];
}) =>
  [...settings.mounted_sources, ...settings.custom_folders]
    .filter((row) => row.checked && !row.offline)
    .map((row) => row.path)
    .filter(
      (path, i, paths) =>
        !paths.some(
          (other, j) =>
            j !== i &&
            covered(path, other) &&
            (!sameSource(path, other) || j < i),
        ),
    );

export function sourceLabel(
  row: SourceRow,
  platform = globalThis.navigator?.platform || "",
) {
  return /mac/i.test(platform) && row.volume
    ? `${row.volume} · ${row.path}`
    : row.path;
}

/** A union of selected roots, retaining one indexed record per normalized path. */
export function scopeFiles<T extends { path: string }>(
  files: T[],
  roots: string[],
): T[] {
  const seen = new Set<string>();
  return files.filter((file) => {
    const key = pathKey(file.path);
    if (seen.has(key) || !roots.some((root) => covered(file.path, root)))
      return false;
    seen.add(key);
    return true;
  });
}

/** Mount identity comes from the path, never the display name (names can collide). */
export function driveKey(path: string): string {
  const key = pathKey(path);
  if (/^[a-z]:/.test(key)) return key.slice(0, 2);
  if (key.startsWith("//")) return key.split("/").slice(0, 4).join("/");
  const root = key.match(/^(.*)\/(?:avid mediafiles|omfi mediafiles)(?:\/|$)/);
  return root ? root[1] || "/" : key.split("/").slice(0, 3).join("/");
}
export function groupSources(rows: SourceRow[]) {
  const groups = new Map<
    string,
    { key: string; name: string; rows: SourceRow[] }
  >();
  for (const row of rows) {
    const key = driveKey(row.path);
    const group = groups.get(key) || { key, name: row.volume || key, rows: [] };
    group.rows.push(row);
    groups.set(key, group);
  }
  return [...groups.values()];
}
export function driveCheckState(rows: SourceRow[]) {
  return {
    checked: rows.length > 0 && rows.every((row) => row.checked),
    indeterminate:
      rows.some((row) => row.checked) && rows.some((row) => !row.checked),
  };
}
export function toggleDrive(rows: SourceRow[], key: string, checked: boolean) {
  return rows.map((row) =>
    driveKey(row.path) === key ? { ...row, checked } : row,
  );
}
export function shortSourceLabel(path: string, mounted: boolean) {
  const normalized = path.replaceAll("\\", "/");
  return (
    (mounted ? normalized.split(/\/Avid MediaFiles\//i)[1] : undefined) ||
    normalized.split("/").filter(Boolean).slice(-3).join("/")
  );
}
export const DEFAULT_RAIL_WIDTH = 320;
export const MIN_RAIL_WIDTH = 240;
export const RAIL_WIDTH_KEY = "mobboss.rail-width";
export function clampRailWidth(width: number, windowWidth: number) {
  return Math.max(
    MIN_RAIL_WIDTH,
    Math.min(
      Number.isFinite(width) ? width : DEFAULT_RAIL_WIDTH,
      Math.max(MIN_RAIL_WIDTH, windowWidth * 0.45),
    ),
  );
}
export function readRailWidth(
  storage: Pick<Storage, "getItem">,
  windowWidth: number,
) {
  try {
    const saved = storage.getItem(RAIL_WIDTH_KEY);
    return clampRailWidth(
      saved === null ? DEFAULT_RAIL_WIDTH : Number(saved),
      windowWidth,
    );
  } catch {
    return clampRailWidth(DEFAULT_RAIL_WIDTH, windowWidth);
  }
}
export function saveRailWidth(
  storage: Pick<Storage, "setItem">,
  width: number,
  windowWidth: number,
) {
  const clamped = clampRailWidth(width, windowWidth);
  try {
    storage.setItem(RAIL_WIDTH_KEY, String(clamped));
  } catch {
    /* Session resizing still works. */
  }
  return clamped;
}
