import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { bytes, filename, timecode, type Clip } from "./model";
interface PreparationProgress {
  request_id: string;
  phase: "reading" | "encoding" | "finalizing" | "loading";
  fraction: number;
  seconds: number;
  duration: number;
  input_bytes: number;
  input_files: number;
  output_bytes?: number | null;
  eta_seconds?: number | null;
  speed?: number | null;
}
export function clipAudioFiles(clip: Clip) {
  const seen = new Set<string>();
  return clip.files
    .filter((f) => f.metadata.track_kind === "audio")
    .filter((f) => {
      const key = f.metadata.file_package_uid || f.path;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
}
export default function Player({
  clip,
  poster,
  onClose,
}: {
  clip: Clip;
  poster?: string;
  onClose: () => void;
}) {
  const r =
    clip.metadata.timecode_rate ||
    (clip.metadata.track_kind === "video" ? clip.metadata.edit_rate : null);
  const frameStep =
    r && r.numerator > 0
      ? r.denominator / r.numerator
      : 1 / (clip.metadata.timecode_fps || 24);
  const uncompressedTitle = clip.files.some(
    (f) => f.metadata.codec === "DNxUncompressed",
  );
  const audio = clipAudioFiles(clip);
  const [left, setLeft] = useState(0),
    [right, setRight] = useState(audio.length > 1 ? 1 : -1);
  const [attempt, setAttempt] = useState(0),
    [path, setPath] = useState(""),
    [error, setError] = useState(""),
    [fraction, setFraction] = useState(0),
    [busy, setBusy] = useState(true),
    [position, setPosition] = useState(0),
    [length, setLength] = useState(0),
    [playing, setPlaying] = useState(false);
  const [preparation, setPreparation] = useState<PreparationProgress | null>(
    null,
  );
  const [source, setSource] = useState("");
  const [encoder, setEncoder] = useState("");
  const video = useRef<HTMLVideoElement>(null),
    job = useRef(""),
    reverse = useRef<number | undefined>(undefined),
    speed = useRef(1);
  const stopReverse = () => {
    if (reverse.current !== undefined) {
      clearInterval(reverse.current);
      reverse.current = undefined;
    }
  };
  const seek = (seconds: number) => {
    const v = video.current;
    if (v && Number.isFinite(v.duration)) {
      v.currentTime = Math.max(0, Math.min(v.duration, seconds));
      setPosition(v.currentTime);
    }
  };
  const play = () => {
    stopReverse();
    const v = video.current;
    if (v) {
      v.playbackRate = 1;
      speed.current = 1;
      v.play().catch((e) => setError(String(e)));
    }
  };
  useEffect(() => {
    let blobUrl: string | undefined;
    let disposed = false,
      unlisten: (() => void) | undefined;
    const requestId = `${Date.now()}-${Math.random()}`;
    job.current = requestId;
    setBusy(true);
    setPlaying(false);
    setPath("");
    setSource("");
    setEncoder("");
    setError("");
    setFraction(0);
    setPreparation(null);
    setPosition(0);
    setLength(0);
    if (uncompressedTitle) {
      setBusy(false);
      return;
    }
    (async () => {
      unlisten = await listen<PreparationProgress>(
        "proxy-progress",
        ({ payload: p }) => {
          if (!disposed && p.request_id === requestId) {
            setFraction(p.fraction);
            setPreparation(p);
          }
        },
      );
      if (disposed) {
        unlisten();
        return;
      }
      const preview = await invoke<{ path: string; encoder: string }>(
        "prepare_proxy",
        {
          clipId: clip.id,
          tracks: audio.length ? (right >= 0 ? [left, right] : [left]) : [],
          requestId,
        },
      );
      const path = preview.path;
      setFraction(1);
      let source = convertFileSrc(path);
      // WebKitGTK delegates media fetching to GStreamer, which cannot read Tauri's
      // custom URI scheme. Fetch from the scoped asset protocol into a browser Blob;
      // native Blob seeking works. macOS/Windows stream the range-capable asset URL.
      if (/linux/i.test(navigator.platform)) {
        if (!disposed)
          setPreparation((old) => (old ? { ...old, phase: "loading" } : old));
        const response = await fetch(source);
        if (!response.ok)
          throw new Error(`Could not load cached preview (${response.status})`);
        blobUrl = URL.createObjectURL(await response.blob());
        source = blobUrl;
      }
      if (!disposed) {
        setSource(source);
        setEncoder(preview.encoder);
        setPath(path);
        setFraction(1);
        setBusy(false);
      } else if (blobUrl) URL.revokeObjectURL(blobUrl);
    })().catch((e) => {
      if (!disposed) {
        setError(String(e));
        setBusy(false);
      }
    });
    return () => {
      disposed = true;
      if (blobUrl) URL.revokeObjectURL(blobUrl);
      unlisten?.();
      stopReverse();
      invoke("cancel_proxy", { requestId }).catch(() => {});
    };
  }, [clip.id, left, right, attempt]);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (
        e.target instanceof HTMLElement &&
        /^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName)
      )
        return;
      const v = video.current;
      if (!v || !path) return;

      if (
        !["j", "k", "l", " ", "ArrowLeft", "ArrowRight"].includes(
          e.key.toLowerCase(),
        )
      )
        return;
      e.preventDefault();
      switch (e.key.toLowerCase()) {
        case "j": {
          v.pause();
          speed.current =
            reverse.current === undefined ? 1 : Math.min(8, speed.current * 2);
          stopReverse();
          reverse.current = window.setInterval(() => {
            seek(v.currentTime - 0.1 * speed.current);
            if (v.currentTime === 0) stopReverse();
          }, 100);
          break;
        }
        case "k":
          stopReverse();
          v.pause();
          break;
        case "l":
          stopReverse();
          speed.current = v.paused ? 1 : Math.min(8, v.playbackRate * 2);
          v.playbackRate = speed.current;
          v.play().catch((e) => setError(String(e)));
          break;
        case " ":
          if (v.paused) play();
          else {
            stopReverse();
            v.pause();
          }
          break;
        case "arrowleft":
          stopReverse();
          v.pause();
          seek(v.currentTime - frameStep);
          break;
        case "arrowright":
          stopReverse();
          v.pause();
          seek(v.currentTime + frameStep);
          break;
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [path, clip, onClose]);
  return (
    <div className="modal-backdrop">
      <section
        className="modal preview-player"
        role="dialog"
        aria-modal="true"
        aria-labelledby="player-title"
      >
        <div className="player-title">
          <div>
            <h2 id="player-title">{clip.name}</h2>
            <small>
              {clip.tracks} · {clip.metadata.project || "Unknown"}
            </small>
            {encoder && (
              <small style={{ display: "block" }}>Preview: {encoder}</small>
            )}
          </div>
          <button onClick={onClose} aria-label="Close player">
            Close
          </button>
        </div>
        <video
          ref={video}
          autoPlay
          playsInline
          preload="metadata"
          src={source || undefined}
          poster={poster ? convertFileSrc(poster) : undefined}
          onTimeUpdate={(e) => setPosition(e.currentTarget.currentTime)}
          onLoadedMetadata={(e) => setLength(e.currentTarget.duration)}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onError={() => {
            if (path)
              setError(
                "The webview could not play this preview. Check the platform media codecs.",
              );
          }}
        />
        {uncompressedTitle && (
          <p role="status" className="hint">
            Preview not available for DNxUncompressed (Avid uncompressed title).
          </p>
        )}
        {!uncompressedTitle && (
          <>
            <div className="player-transport">
              <button
                disabled={!path}
                onClick={() => {
                  if (video.current?.paused) play();
                  else {
                    stopReverse();
                    video.current?.pause();
                  }
                }}
              >
                {playing ? "Pause" : "Play"}
              </button>
              <input
                type="range"
                aria-label="Scrub preview"
                min={0}
                max={Number.isFinite(length) ? length : 0}
                step={frameStep}
                value={position}
                disabled={!path}
                onChange={(e) => seek(Number(e.target.value))}
              />
              <output>{timecode(clip.metadata, position)}</output>
            </div>
            <p className="hint">
              J reverse · K pause · L forward · ← / → step a frame ·{" "}
              {position.toFixed(2)} /{" "}
              {Number.isFinite(length) ? length.toFixed(2) : "0.00"} s
            </p>
            {audio.length > 0 && (
              <div className="audio-selectors">
                <label>
                  Left / mono
                  <select
                    value={left}
                    onChange={(e) => setLeft(Number(e.target.value))}
                  >
                    {audio.map((f, i) => (
                      <option disabled={i === right} key={f.path} value={i}>
                        A{i + 1} · {filename(f.path)}
                      </option>
                    ))}
                  </select>
                </label>
                {audio.length > 1 && (
                  <label>
                    Right
                    <select
                      value={right}
                      onChange={(e) => setRight(Number(e.target.value))}
                    >
                      <option value={-1}>
                        Duplicate left to both speakers
                      </option>
                      {audio.map((f, i) => (
                        <option disabled={i === left} key={f.path} value={i}>
                          A{i + 1} · {filename(f.path)}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
              </div>
            )}
          </>
        )}
        {busy && (
          <div className="proxy-progress" role="status">
            <progress
              max={1}
              value={
                !preparation || preparation.phase === "reading"
                  ? undefined
                  : fraction
              }
            />
            <div>
              <strong>
                {!preparation || preparation.phase === "reading"
                  ? "Reading input headers…"
                  : preparation.phase === "loading"
                    ? "Loading cached preview…"
                    : preparation.phase === "finalizing"
                      ? "Finalizing preview…"
                      : `Encoding · ${Math.round(fraction * 100)}%`}
              </strong>
              {preparation && (
                <small>
                  {preparation.input_files} input
                  {preparation.input_files === 1 ? "" : "s"} ·{" "}
                  {bytes(preparation.input_bytes)} source total
                </small>
              )}
              {preparation?.phase === "encoding" && (
                <small>
                  {preparation.seconds.toFixed(1)} /{" "}
                  {preparation.duration.toFixed(1)} s
                  {preparation.eta_seconds != null
                    ? ` · ETA ~${Math.ceil(preparation.eta_seconds)} s`
                    : " · estimating ETA…"}
                  {preparation.output_bytes != null
                    ? ` · ${bytes(preparation.output_bytes)} encoded`
                    : ""}
                </small>
              )}
            </div>
            <button
              onClick={() =>
                invoke("cancel_proxy", { requestId: job.current }).catch((e) =>
                  setError(String(e)),
                )
              }
            >
              Cancel
            </button>
          </div>
        )}
        {error && (
          <p role="alert" className="preview-error">
            {error}
          </p>
        )}
        {!busy && error && (
          <button onClick={() => setAttempt((n) => n + 1)}>
            Retry preview
          </button>
        )}
      </section>
    </div>
  );
}
