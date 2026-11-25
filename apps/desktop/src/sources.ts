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
const key = (p: string) =>
  p
    .replaceAll("\\", "/")
    .replace(/^\/\/\?\/UNC\//i, "//")
    .replace(/^\/\/\?\//, "")
    .replace(/\/$/, "")
    .toLowerCase();
export const sameSource = (a: string, b: string) => key(a) === key(b);
const covered = (p: string, root: string) =>
  key(p) === key(root) || key(p).startsWith(key(root) + "/");
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
export function mergeSources(
  rows: SourceRow[],
  found: FoundSource[],
): SourceRow[] {
  return [
    ...rows.map((row) => ({
      ...row,
      offline: !found.some((s) => sameSource(s.path, row.path)),
    })),
    ...found
      .filter(
        (s, i) =>
          !rows.some((row) => sameSource(row.path, s.path)) &&
          !found.slice(0, i).some((other) => sameSource(other.path, s.path)),
      )
      .map((s) => ({
        path: s.path,
        checked: true,
        offline: false,
        volume: s.volume,
      })),
  ];
}
export const checkedSources = (settings: {
  mounted_sources: SourceRow[];
  custom_folders: SourceRow[];
}) =>
  [...settings.mounted_sources, ...settings.custom_folders]
    .filter((row) => row.checked)
    .map((row) => row.path);

export function sourceLabel(
  row: SourceRow,
  platform = globalThis.navigator?.platform || "",
) {
  return /mac/i.test(platform) && row.volume
    ? `${row.volume} · ${row.path}`
    : row.path;
}
