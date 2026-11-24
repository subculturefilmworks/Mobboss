import { useMemo, useRef, useState } from "react";
import {
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
  type VisibilityState,
  type ColumnSizingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { bytes, fields, type Row } from "./model";
export function useStored<T>(key: string, fallback: T) {
  const [value, setValue] = useState<T>(() => {
    try {
      return JSON.parse(localStorage.getItem(key) || "null") ?? fallback;
    } catch {
      return fallback;
    }
  });
  return [
    value,
    (next: T | ((old: T) => T)) =>
      setValue((old) => {
        const v =
          typeof next === "function" ? (next as (old: T) => T)(old) : next;
        localStorage.setItem(key, JSON.stringify(v));
        return v;
      }),
  ] as const;
}
export default function MediaTable({
  rows,
  showDiff,
  selected,
  setSelected,
}: {
  rows: Row[];
  showDiff: boolean;
  selected: Set<string>;
  setSelected: (value: Set<string>) => void;
}) {
  const columns = useMemo<ColumnDef<Row>[]>(
    () =>
      (showDiff
        ? [
            ...fields.filter(([key]) => key === "diff"),
            ...fields.filter(([key]) => key !== "diff"),
          ]
        : fields
      ).map(([key, label]) => ({
        id: key,
        accessorFn: (r) => r.values[key],
        header: label,
        size:
          key === "path" || key === "material" || key === "uid"
            ? 300
            : key === "filename" || key === "clip"
              ? 220
              : 150,
        minSize: 75,
        cell: (info) =>
          key === "size"
            ? bytes(Number(info.getValue()))
            : String(info.getValue() || "—"),
      })),
    [showDiff],
  );
  const [sorting, setSorting] = useStored<SortingState>("mobboss.sort", [
    { id: "filename", desc: false },
  ]);
  const [columnVisibility, setColumnVisibility] = useStored<VisibilityState>(
    "mobboss.columns",
    {
      created: false,
      modified: false,
      material: false,
      uid: false,
      ul: false,
      folder: false,
      sample: false,
      depth: false,
      kind: false,
      diff: false,
    },
  );
  const [columnSizing, setColumnSizing] = useStored<ColumnSizingState>(
    "mobboss.widths",
    {},
  );
  const table = useReactTable({
    data: rows,
    columns,
    getRowId: (r) => r.file.path,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    state: {
      sorting,
      columnVisibility: {
        ...columnVisibility,
        ...(showDiff ? { diff: true } : {}),
      },
      columnSizing,
    },
    onSortingChange: setSorting,
    onColumnVisibilityChange: setColumnVisibility,
    onColumnSizingChange: setColumnSizing,
    columnResizeMode: "onChange",
  });
  const ordered = table.getRowModel().rows;
  const scroll = useRef<HTMLDivElement>(null);
  const anchor = useRef<string | null>(null);
  const virtual = useVirtualizer({
    count: ordered.length,
    getScrollElement: () => scroll.current,
    estimateSize: () => 36,
    overscan: 15,
  });
  const allSelected =
    ordered.length > 0 && ordered.every((r) => selected.has(r.id));
  function choose(id: string, shift: boolean, additive: boolean) {
    const next = additive || shift ? new Set(selected) : new Set<string>();
    const a = ordered.findIndex((r) => r.id === anchor.current),
      b = ordered.findIndex((r) => r.id === id);
    if (shift && a >= 0)
      for (let i = Math.min(a, b); i <= Math.max(a, b); i++)
        next.add(ordered[i].id);
    else {
      if (additive && next.has(id)) next.delete(id);
      else next.add(id);
      anchor.current = id;
    }
    setSelected(next);
  }
  return (
    <section className="table-section">
      <div className="table-tools">
        <span>{rows.length.toLocaleString()} matching rows</span>
        <details>
          <summary>Columns</summary>
          <div className="column-menu">
            {table.getAllLeafColumns().map((c) => (
              <label key={c.id}>
                <input
                  type="checkbox"
                  checked={c.getIsVisible()}
                  onChange={c.getToggleVisibilityHandler()}
                />
                {String(c.columnDef.header)}
              </label>
            ))}
            <button
              onClick={() => {
                setColumnSizing({});
                setSorting([{ id: "filename", desc: false }]);
              }}
            >
              Reset widths & sort
            </button>
          </div>
        </details>
      </div>
      <div
        ref={scroll}
        className="table-scroll"
        role="table"
        aria-label="Media inventory"
        aria-rowcount={ordered.length}
        aria-colcount={table.getVisibleLeafColumns().length + 1}
      >
        <div style={{ width: table.getTotalSize() + 44, minWidth: "100%" }}>
          <div role="row" className="table-head">
            <div role="columnheader" className="check-cell">
              <input
                aria-label="Select all filtered files"
                type="checkbox"
                checked={allSelected}
                onChange={() => {
                  const next = new Set(selected);
                  ordered.forEach((r) =>
                    allSelected ? next.delete(r.id) : next.add(r.id),
                  );
                  setSelected(next);
                }}
              />
            </div>
            {table.getHeaderGroups()[0].headers.map((h) => (
              <div
                role="columnheader"
                aria-sort={
                  h.column.getIsSorted() === "asc"
                    ? "ascending"
                    : h.column.getIsSorted() === "desc"
                      ? "descending"
                      : "none"
                }
                className="heading-cell"
                style={{ width: h.getSize() }}
                key={h.id}
              >
                <button onClick={h.column.getToggleSortingHandler()}>
                  {flexRender(h.column.columnDef.header, h.getContext())}
                  <span>
                    {h.column.getIsSorted() === "asc"
                      ? " ↑"
                      : h.column.getIsSorted() === "desc"
                        ? " ↓"
                        : ""}
                  </span>
                </button>
                <div
                  role="separator"
                  aria-label={`Resize ${h.id}`}
                  className="resize"
                  onMouseDown={h.getResizeHandler()}
                  onTouchStart={h.getResizeHandler()}
                />
              </div>
            ))}
          </div>
          <div style={{ height: virtual.getTotalSize(), position: "relative" }}>
            {virtual.getVirtualItems().map((item) => {
              const r = ordered[item.index];
              return (
                <div
                  role="row"
                  aria-rowindex={item.index + 2}
                  aria-selected={selected.has(r.id)}
                  tabIndex={0}
                  className={`media-row ${selected.has(r.id) ? "selected" : ""}`}
                  key={r.id}
                  style={{
                    position: "absolute",
                    top: 0,
                    left: 0,
                    height: 36,
                    width: "100%",
                    transform: `translateY(${item.start}px)`,
                  }}
                  onClick={(e) =>
                    choose(r.id, e.shiftKey, e.metaKey || e.ctrlKey)
                  }
                  onKeyDown={(e) => {
                    if (e.key === " " || e.key === "Enter") {
                      e.preventDefault();
                      choose(r.id, e.shiftKey, e.metaKey || e.ctrlKey);
                    }
                  }}
                >
                  <div role="cell" className="check-cell">
                    <input
                      aria-label={`Select ${r.original.values.filename}`}
                      type="checkbox"
                      checked={selected.has(r.id)}
                      onClick={(e) => e.stopPropagation()}
                      onChange={() => choose(r.id, false, true)}
                    />
                  </div>
                  {r.getVisibleCells().map((c) => (
                    <div
                      role="cell"
                      key={c.id}
                      className={`data-cell ${c.column.id === "track" ? "track" : ""}`}
                      style={{ width: c.column.getSize() }}
                      title={String(c.getValue())}
                    >
                      {flexRender(c.column.columnDef.cell, c.getContext())}
                    </div>
                  ))}
                </div>
              );
            })}
          </div>
        </div>
        {!rows.length && (
          <div className="empty">
            <div className="empty-icon">M</div>
            <h2>Your media, accounted for.</h2>
            <p>
              Choose a folder or mounted volume to start an inventory.
              <br />
              Search and filters narrow your results here.
            </p>
          </div>
        )}
      </div>
    </section>
  );
}
