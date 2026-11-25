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
