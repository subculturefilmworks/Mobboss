import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  StreamingPlayer,
  type StreamStatus,
  type PlaybackMetrics,
} from "./streaming";
import { bytes, filename, timecode, isAudioOnly, type Clip } from "./model";
import { AudioBadge } from "./Previews";
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
  const audioOnly = isAudioOnly(clip);
  const [left, setLeft] = useState(0),
    [right, setRight] = useState(audio.length > 1 ? 1 : -1);
  const [attempt, setAttempt] = useState(0),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(true),
    [position, setPosition] = useState(0),
    [playing, setPlaying] = useState(false);
  const [preparation, setPreparation] = useState<StreamStatus | null>(null);
  const [encoder, setEncoder] = useState("");
  const [metrics, setMetrics] = useState<PlaybackMetrics | null>(null);
  const picture =
    clip.files.find((f) => f.metadata.track_kind === "video") || clip.files[0];
  const length = Math.max(
    0,
    ...(audioOnly ? audio : [picture]).map((file) => {
      const rate = file?.metadata.edit_rate;
      return rate && rate.numerator > 0
        ? (Math.max(0, file.metadata.duration || 0) * rate.denominator) /
            rate.numerator
        : 0;
    }),
  );
  const video = useRef<HTMLVideoElement>(null),
    controller = useRef<StreamingPlayer | undefined>(undefined),
    reverse = useRef<number | undefined>(undefined),
    speed = useRef(1);
  const stopReverse = () => {
    if (reverse.current !== undefined) {
      clearInterval(reverse.current);
      reverse.current = undefined;
    }
  };
  const seek = (seconds: number) => {
    if (controller.current?.seek(seconds)) stopReverse();
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
    setError("");
    setEncoder("");
    setPreparation(null);
    setMetrics(null);
    setPosition(0);
    setBusy(!uncompressedTitle);
    if (uncompressedTitle || !video.current) return;
    const player = new StreamingPlayer(
      video.current,
      clip.id,
      audio.length ? (right >= 0 ? [left, right] : [left]) : [],
      length,
      {
        busy: setBusy,
        status: setPreparation,
        encoder: setEncoder,
        position: setPosition,
        error: setError,
        metrics: setMetrics,
      },
      frameStep,
    );
    controller.current = player;
    void player.open();
    return () => {
      stopReverse();
      player.dispose();
      controller.current = undefined;
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
      if (!v || !controller.current) return;

      if (
        !["j", "k", "l", " ", "arrowleft", "arrowright"].includes(
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
  }, [clip, onClose]);
  return (
    <div className="modal-backdrop">
      <section
        className="modal preview-player"
        role="dialog"
        aria-modal="true"
        aria-labelledby="player-title"
      >
        <div className="player-title">
          <div className="player-heading">
            <h2 id="player-title" title={clip.name}>
              {clip.name}
            </h2>
            <small>
              {clip.tracks} · {clip.metadata.project || "Unknown"}
            </small>
            {encoder && (
              <small style={{ display: "block" }}>Preview: {encoder}</small>
            )}
          </div>
          <button onClick={onClose} aria-label="Close player">
            ×
          </button>
        </div>
        <div className="player-media">
          <video
            className={audioOnly ? "audio-stream" : undefined}
            ref={video}
            playsInline
            preload="metadata"
            poster={poster ? convertFileSrc(poster) : undefined}
            onTimeUpdate={(e) => {
              if (!busy || e.currentTarget.readyState >= 2)
                setPosition(e.currentTarget.currentTime);
            }}
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onError={() => {
              if (!uncompressedTitle)
                setError(
                  "The webview could not play this preview. Check the platform media codecs.",
                );
            }}
          />
          {audioOnly && (
            <div
              className="waveform-player"
              role="img"
              aria-label={`Audio waveform for ${clip.name}`}
            >
              {poster ? (
                <img src={convertFileSrc(poster)} alt="" />
              ) : (
                <div className="waveform-pending">Audio waveform pending</div>
              )}
              <div
                className="waveform-played"
                style={{
                  width: `${Math.max(0, Math.min(100, (position / (length || 1)) * 100))}%`,
                }}
              />
              <div
                className="waveform-playhead"
                style={{
                  left: `${Math.max(0, Math.min(100, (position / (length || 1)) * 100))}%`,
                }}
              />
              <span className="waveform-caption">
                <AudioBadge /> {clip.tracks}
                {length > 60 ? " · waveform sampled from first 60 s" : ""}
              </span>
            </div>
          )}
        </div>
        {uncompressedTitle && (
          <p role="status" className="hint">
            Preview not available for DNxUncompressed (Avid uncompressed title).
          </p>
        )}
        {!uncompressedTitle && (
          <>
            <div className="player-transport">
              <button
                disabled={busy}
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
                disabled={!length}
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
        {preparation && !busy && (
          <p className="hint" role="status">
            Buffered to {preparation.buffered_end.toFixed(1)} s ·{" "}
            {bytes(preparation.memory_bytes)} in memory
            {preparation.backpressure ? " · Reading paused (buffer full)" : ""}
            {metrics
              ? ` · ${metrics.seek ? "Seek resume" : "First frame"} ${(metrics.firstFrameMs / 1000).toFixed(2)} s`
              : ""}
          </p>
        )}
        {busy && (
          <div className="proxy-progress" role="status">
            <progress />
            <div>
              <strong>
                {!preparation || preparation.phase === "reading"
                  ? "Reading input headers…"
                  : "Encoding first fragment…"}
              </strong>
              {preparation && (
                <small>
                  {preparation.input_files} inputs ·{" "}
                  {bytes(preparation.input_bytes)} source total ·{" "}
                  {bytes(preparation.output_bytes)} streamed
                  {preparation.phase === "encoding"
                    ? ` · ${preparation.seconds.toFixed(1)} s encoded`
                    : ""}
                  {preparation.eta_seconds != null
                    ? ` · ETA first fragment ~${Math.ceil(preparation.eta_seconds)} s`
                    : ""}
                </small>
              )}
            </div>
            <button onClick={onClose}>Cancel</button>
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
