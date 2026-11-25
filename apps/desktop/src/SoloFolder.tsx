import { useLayoutEffect, useRef, useState } from "react";
import { middleEllipsis, sourceLabel, type SourceRow } from "./sources";

/** Native options use the same path label and pixel measurement as the rail. */
export default function SoloFolder({
  rows,
  value,
  onChange,
}: {
  rows: SourceRow[];
  value: string;
  onChange: (path: string) => void;
}) {
  const ref = useRef<HTMLSelectElement>(null);
  const [labels, setLabels] = useState<string[]>([]);
  useLayoutEffect(() => {
    const node = ref.current!;
    const context = document.createElement("canvas").getContext("2d")!;
    const resize = () => {
      context.font = getComputedStyle(node).font;
      setLabels(
        rows.map((row) =>
          middleEllipsis(
            sourceLabel(row),
            Math.max(0, node.clientWidth - 32),
            (text) => context.measureText(text).width,
          ),
        ),
      );
    };
    const observer = new ResizeObserver(resize);
    observer.observe(node);
    resize();
    return () => observer.disconnect();
  }, [rows]);
  return (
    <select
      ref={ref}
      aria-label="Solo folder"
      title={value || "All ticked folders"}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    >
      <option value="">All ticked folders</option>
      {rows.map((row, i) => (
        <option key={row.path} value={row.path} title={row.path}>
          {labels[i] || sourceLabel(row)}
        </option>
      ))}
    </select>
  );
}
