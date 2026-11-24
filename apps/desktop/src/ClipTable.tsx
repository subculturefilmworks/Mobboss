import { Poster } from "./Previews";
import { Fragment, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  clipSortFields,
  type ClipSort,
  type ClipSortKey,
  bytes,
  duration,
  filename,
  selectClip,
  timecode,
  type Clip,
} from "./model";

export default function ClipTable({
  clips,
  sort,
  setSort,
  onViewportChange,
  selected,
  setSelected,
  thumbnails = {},
  thumbnailErrors = {},
  grid = false,
  onPreview,
}: {
  clips: Clip[];
  sort: ClipSort;
  setSort: (sort: ClipSort) => void;
  onViewportChange: (ids: string[]) => void;
  thumbnails?: Record<string, string>;
  thumbnailErrors?: Record<string, string>;
  grid?: boolean;
  onPreview?: (clip: Clip) => void;
  selected: Set<string>;
  setSelected: (s: Set<string>) => void;
}) {
  const [expanded, setExpanded] = useState(new Set<string>());
  const scroll = useRef<HTMLDivElement>(null);
  const entries = clips.flatMap((clip) => [
    { clip, file: null },
    ...(expanded.has(clip.id)
      ? clip.files.map((file) => ({ clip, file }))
      : []),
  ]);
  const virtual = useVirtualizer({
    count: entries.length,
    getScrollElement: () => scroll.current,
    estimateSize: () => 56,
    overscan: 8,
  });
  const virtualItems = virtual.getVirtualItems();
  const layout = `${grid}:${virtualItems.map((v) => v.key).join(",")}:${[...expanded].join(",")}`;
  useEffect(() => {
    const root = scroll.current;
    if (!root) return;
    const nodes = [...root.querySelectorAll<HTMLElement>("[data-clip-id]")];
    const visible = new Set<Element>();
    let timer: ReturnType<typeof setTimeout>;
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting && entry.intersectionRect.height > 0)
            visible.add(entry.target);
          else visible.delete(entry.target);
        });
        clearTimeout(timer);
        timer = setTimeout(
          () =>
            onViewportChange([
              ...new Set(
                nodes
                  .filter((node) => visible.has(node))
                  .map((node) => node.dataset.clipId!),
              ),
            ]),
          50,
        );
      },
      { root, threshold: 0 },
    );
    nodes.forEach((node) => observer.observe(node));
    return () => {
      observer.disconnect();
      clearTimeout(timer);
    };
  }, [clips, layout, onViewportChange]);
  const sortingControls = (
    <div className="clip-sort-controls">
      <label>
        Sort by{" "}
        <select
          aria-label="Sort clips by"
          value={sort.key}
          onChange={(e) =>
            setSort({ key: e.target.value as ClipSortKey, desc: sort.desc })
          }
        >
          {clipSortFields.map(([key, label]) => (
            <option key={key} value={key}>
              {label}
            </option>
          ))}
        </select>
      </label>
      <button
        aria-label={sort.desc ? "Sort ascending" : "Sort descending"}
        onClick={() => setSort({ ...sort, desc: !sort.desc })}
      >
        {sort.desc ? "Descending ↓" : "Ascending ↑"}
      </button>
    </div>
  );
  const header = (key: ClipSortKey, label: string) => (
    <span
      role="columnheader"
      aria-sort={
        sort.key === key ? (sort.desc ? "descending" : "ascending") : "none"
      }
    >
      <button
        onClick={() =>
          setSort({ key, desc: sort.key === key ? !sort.desc : false })
        }
      >
        {label}
        {sort.key === key ? (sort.desc ? " ↓" : " ↑") : ""}
      </button>
    </span>
  );
  const all =
    clips.length > 0 &&
    clips.every((c) => c.files.every((f) => selected.has(f.path)));
  if (grid)
    return (
      <section className="table-section">
        <div className="table-tools">
          <span>{clips.length.toLocaleString()} matching clips</span>
          {sortingControls}
        </div>
        <div ref={scroll} className="clip-grid">
          {clips.map((clip) => (
            <article
              tabIndex={0}
              key={clip.id}
              data-clip-id={clip.id}
              className={`clip-card ${clip.files.every((f) => selected.has(f.path)) ? "selected" : ""}`}
              onClick={(e) => {
                setSelected(selectClip(selected, clip, e.ctrlKey || e.metaKey));
                if (!e.ctrlKey && !e.metaKey) onPreview?.(clip);
              }}
              onKeyDown={(e) => {
                if (
                  e.target === e.currentTarget &&
                  (e.key === "Enter" || e.key === " ")
                ) {
                  e.preventDefault();
                  if (e.key === "Enter") onPreview?.(clip);
                  else setSelected(selectClip(selected, clip));
                }
              }}
            >
              <Poster
                path={thumbnails[clip.id]}
                name={clip.name}
                error={thumbnailErrors[clip.id]}
                uncompressedTitle={clip.files.some(
                  (f) => f.metadata.codec === "DNxUncompressed",
                )}
              />
              <div>
                <label onClick={(e) => e.stopPropagation()}>
                  <input
                    type="checkbox"
                    aria-label={`Select ${clip.name}`}
                    checked={clip.files.every((f) => selected.has(f.path))}
                    onChange={() => setSelected(selectClip(selected, clip))}
                  />
                  <b title={clip.name}>{clip.name}</b>
                </label>
                <small>{clip.metadata.project || "Unknown"}</small>
                <small>
                  {clip.tracks} · {duration(clip.metadata) || "—"} ·{" "}
                  {bytes(clip.size)}
                </small>
                <small>{timecode(clip.metadata)}</small>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onPreview?.(clip);
                  }}
                >
                  ▶ Preview
                </button>
              </div>
            </article>
          ))}
        </div>
      </section>
    );
  return (
    <section className="table-section">
      <div className="table-tools">
        <span>
          {clips.length.toLocaleString()} matching clips · actions include every
          member file
        </span>
        {sortingControls}
      </div>
      <div
        ref={scroll}
        className="table-scroll"
        role="table"
        aria-label="Clip inventory"
      >
        <div className="clip-head" role="row">
          <input
            type="checkbox"
            aria-label="Select all filtered clips"
            checked={all}
            onChange={() => {
              const next = new Set(selected);
              clips.forEach((c) =>
                c.files.forEach((f) =>
                  all ? next.delete(f.path) : next.add(f.path),
                ),
              );
              setSelected(next);
            }}
          />
          <span>Thumbnail</span>
          {header("name", "Clip / files")}
          {header("project", "Project")}
          {header("tracks", "Tracks")}
          {header("duration", "Duration")}
          {header("timecode", "Start TC")}
          {header("size", "Size")}
        </div>
        <div
          style={{
            height: virtual.getTotalSize(),
            position: "relative",
            minWidth: 950,
          }}
        >
          {virtualItems.map((item) => {
            const { clip, file } = entries[item.index],
              checked = file
                ? selected.has(file.path)
                : clip.files.every((f) => selected.has(f.path));
            return (
              <div
                key={file ? `${clip.id}:${file.path}` : clip.id}
                role="row"
                data-clip-id={clip.id}
                tabIndex={0}
                onKeyDown={(e) => {
                  if (
                    e.target === e.currentTarget &&
                    e.key === "Enter" &&
                    !file
                  ) {
                    onPreview?.(clip);
                  }
                }}
                aria-selected={checked}
                className={`clip-row ${checked ? "selected" : ""} ${file ? "clip-member" : ""}`}
                style={{
                  position: "absolute",
                  height: 56,
                  width: "100%",
                  transform: `translateY(${item.start}px)`,
                }}
                onClick={(e) =>
                  file
                    ? setSelected(new Set([file.path]))
                    : (onPreview?.(clip),
                      setSelected(
                        selectClip(
                          selected,
                          clip,
                          e.ctrlKey || e.metaKey || e.shiftKey,
                        ),
                      ))
                }
              >
                <input
                  aria-label={`Select ${file ? filename(file.path) : clip.name}`}
                  type="checkbox"
                  checked={checked}
                  onClick={(e) => e.stopPropagation()}
                  onChange={() => {
                    if (file) {
                      const next = new Set(selected);
                      checked ? next.delete(file.path) : next.add(file.path);
                      setSelected(next);
                    } else setSelected(selectClip(selected, clip));
                  }}
                />
                <span>
                  {!file && (
                    <Poster
                      path={thumbnails[clip.id]}
                      name={clip.name}
                      error={thumbnailErrors[clip.id]}
                      uncompressedTitle={clip.files.some(
                        (f) => f.metadata.codec === "DNxUncompressed",
                      )}
                    />
                  )}
                </span>
                <span className="clip-name" title={file?.path || clip.name}>
                  {file ? (
                    <Fragment>↳ {filename(file.path)}</Fragment>
                  ) : (
                    <>
                      <button
                        aria-label={`Expand ${clip.name}`}
                        aria-expanded={expanded.has(clip.id)}
                        onClick={(e) => {
                          e.stopPropagation();
                          const next = new Set(expanded);
                          next.has(clip.id)
                            ? next.delete(clip.id)
                            : next.add(clip.id);
                          setExpanded(next);
                        }}
                      >
                        {expanded.has(clip.id) ? "▾" : "▸"}
                      </button>
                      {clip.name}
                      <small>{clip.files.length} files</small>
                    </>
                  )}
                </span>
                <span>
                  {(file?.metadata || clip.metadata).project || "Unknown"}
                </span>
                <span className="track">
                  {file?.metadata.track_kind || clip.tracks || "—"}
                </span>
                <span>{duration(file?.metadata || clip.metadata) || "—"}</span>
                <span>{timecode(file?.metadata || clip.metadata)}</span>
                <span title={`${file?.size ?? clip.size} bytes`}>
                  {bytes(file?.size ?? clip.size)}
                </span>
              </div>
            );
          })}
        </div>
        {!clips.length && (
          <div className="empty">
            <h2>No matching clips</h2>
            <p>Scan media or adjust your filters.</p>
          </div>
        )}
      </div>
    </section>
  );
}
