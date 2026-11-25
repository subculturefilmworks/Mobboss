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
  return found
    .filter(
      (source, i) =>
        !found.slice(0, i).some((other) => sameSource(other.path, source.path)),
    )
    .map((source) => ({
      path: source.path,
      volume: source.volume,
      checked: remembered[pathKey(source.path)] ?? true,
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
