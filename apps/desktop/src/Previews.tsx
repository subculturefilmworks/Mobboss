import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
export interface PreviewStatus {
  available: boolean;
  message: string;
  thumbnails: Record<string, string>;
  thumbnail_bytes?: number;
}
interface ThumbnailProgress {
  done: number;
  total: number;
  clip_id: string;
  path: string | null;
  error: string | null;
  cancelled: boolean;
}
export function usePreviews(desktop: boolean, files: unknown) {
  const [status, setStatus] = useState<PreviewStatus>({
    available: false,
    message: "Previews unavailable",
    thumbnails: {},
  });
  const [progress, setProgress] = useState<ThumbnailProgress | null>(null);
  const [running, setRunning] = useState(false);
  const [errors, setErrors] = useState<Record<string, string>>({});
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const cleanups: (() => void)[] = [];
    (async () => {
      cleanups.push(
        await listen<ThumbnailProgress>(
          "thumbnail-progress",
          ({ payload: p }) => {
            if (disposed) return;
            setProgress(p);
            setRunning(true);
            if (p.error && !p.cancelled)
              setErrors((old) => ({ ...old, [p.clip_id]: p.error! }));
            if (p.path)
              setStatus((old) => ({
                ...old,
                thumbnails: { ...old.thumbnails, [p.clip_id]: p.path! },
              }));
          },
        ),
      );
      cleanups.push(
        await listen<boolean>("thumbnails-finished", () => {
          if (!disposed) {
            setRunning(false);
            invoke<PreviewStatus>("preview_status")
              .then((s) => {
                if (!disposed) setStatus(s);
              })
              .catch(() => {});
          }
        }),
      );
      const s = await invoke<PreviewStatus>("preview_status");
      if (!disposed) {
        setStatus(s);
      }
    })().catch((e) => {
      if (!disposed) setStatus((old) => ({ ...old, message: String(e) }));
    });
    return () => {
      disposed = true;
      cleanups.forEach((fn) => fn());
    };
  }, [desktop]);
  useEffect(() => {
    if (desktop)
      invoke<PreviewStatus>("preview_status")
        .then(setStatus)
        .catch(() => {});
  }, [desktop, files]);
  return {
    status,
    refresh: async () =>
      setStatus(await invoke<PreviewStatus>("preview_status")),
    progress,
    running,
    errors,
    retry: async () => {
      setErrors({});
      setRunning(true);
      try {
        await invoke("start_thumbnails");
      } catch (e) {
        setRunning(false);
        setStatus((old) => ({ ...old, message: String(e) }));
      }
    },
    cancel: () => invoke("cancel_thumbnails"),
  };
}
export function Poster({
  path,
  name,
  error,
  uncompressedTitle = false,
}: {
  path?: string;
  name: string;
  error?: string;
  uncompressedTitle?: boolean;
}) {
  const label = uncompressedTitle
    ? "No preview (Avid uncompressed title)"
    : error
      ? "Preview unavailable"
      : "No thumbnail";
  return path ? (
    <img
      className="poster"
      loading="lazy"
      decoding="async"
      src={convertFileSrc(path)}
      alt={`Poster for ${name}`}
    />
  ) : (
    <span
      className="poster placeholder"
      aria-label={label}
      title={
        uncompressedTitle
          ? label
          : error
            ? `Preview unavailable: ${error}`
            : label
      }
    >
      {uncompressedTitle
        ? "No preview · title"
        : error
          ? "Unavailable"
          : "No preview"}
    </span>
  );
}
