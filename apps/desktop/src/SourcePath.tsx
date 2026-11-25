import { useLayoutEffect, useRef, useState } from "react";
import { middleEllipsis, sourceLabel, type SourceRow } from "./sources";

export default function SourcePath({ row }: { row: SourceRow }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [text, setText] = useState(sourceLabel(row));
  useLayoutEffect(() => {
    const node = ref.current!;
    const context = document.createElement("canvas").getContext("2d")!;
    const resize = () => {
      context.font = getComputedStyle(node).font;
      setText(
        middleEllipsis(
          sourceLabel(row),
          node.clientWidth,
          (text) => context.measureText(text).width,
        ),
      );
    };
    const observer = new ResizeObserver(resize);
    observer.observe(node);
    resize();
    return () => observer.disconnect();
  }, [row.path, row.volume]);
  return (
    <span className="source-path" title={row.path} ref={ref}>
      {text}
    </span>
  );
}
