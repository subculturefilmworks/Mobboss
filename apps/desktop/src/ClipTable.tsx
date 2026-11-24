import { Poster } from "./Previews";
import { Fragment, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  bytes,
  duration,
  filename,
  selectClip,
  timecode,
  type Clip,
} from "./model";

export default function ClipTable({
  clips,
  selected,
  setSelected,
  thumbnails = {},
  thumbnailErrors = {},
  grid = false,
}: {
  clips: Clip[];
  thumbnails?: Record<string, string>;
  thumbnailErrors?: Record<string, string>;
  grid?: boolean;
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
  const all =
    clips.length > 0 &&
    clips.every((c) => c.files.every((f) => selected.has(f.path)));
  if (grid)
    return (
      <section className="table-section">
        <div className="table-tools">
          {clips.length.toLocaleString()} matching clips
        </div>
        <div className="clip-grid">
          {clips.map((clip) => (
            <article
              tabIndex={0}
              key={clip.id}
              className={`clip-card ${clip.files.every((f) => selected.has(f.path)) ? "selected" : ""}`}
              onClick={(e) =>
                setSelected(selectClip(selected, clip, e.ctrlKey || e.metaKey))
              }
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setSelected(selectClip(selected, clip));
                }
              }}
            >
              <Poster
                path={thumbnails[clip.id]}
                name={clip.name}
                error={thumbnailErrors[clip.id]}
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
          <span>Clip / files</span>
          <span>Project</span>
          <span>Tracks</span>
          <span>Duration</span>
          <span>Start TC</span>
          <span>Size</span>
        </div>
        <div
          style={{
            height: virtual.getTotalSize(),
            position: "relative",
            minWidth: 950,
          }}
        >
          {virtual.getVirtualItems().map((item) => {
            const { clip, file } = entries[item.index],
              checked = file
                ? selected.has(file.path)
                : clip.files.every((f) => selected.has(f.path));
            return (
              <div
                key={file ? `${clip.id}:${file.path}` : clip.id}
                role="row"
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
                    : setSelected(
                        selectClip(
                          selected,
                          clip,
                          e.ctrlKey || e.metaKey || e.shiftKey,
                        ),
                      )
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
