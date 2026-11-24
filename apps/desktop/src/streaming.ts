import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export interface StreamStatus {
  phase: "reading" | "encoding" | "ready";
  seconds: number;
  duration: number;
  input_bytes: number;
  input_files: number;
  output_bytes: number;
  eta_seconds: number | null;
  encoder: string;
  buffered_end: number;
  memory_bytes: number;
  backpressure: boolean;
}
export interface PlaybackMetrics {
  firstFrameMs: number;
  seek: boolean;
}
export interface BufferedRange {
  start: number;
  end: number;
}
export const CLIENT_BYTES = 40_000_000;
export function isBuffered(ranges: BufferedRange[], time: number) {
  return ranges.some((r) => time >= r.start && time < r.end - 0.02);
}
export function seekAction(
  ranges: BufferedRange[],
  time: number,
  duration: number,
  frameStep = 1 / 24,
) {
  const target = Math.max(0, Math.min(Math.max(0, duration - frameStep), time));
  return { target, restart: !isBuffered(ranges, target) };
}
type MediaSourceConstructor = typeof MediaSource;
export function mediaSourceFor(
  mime: string,
): MediaSourceConstructor | undefined {
  const managed = (
    window as unknown as { ManagedMediaSource?: MediaSourceConstructor }
  ).ManagedMediaSource;
  if (typeof MediaSource !== "undefined" && MediaSource.isTypeSupported(mime))
    return MediaSource;
  if (managed?.isTypeSupported(mime)) return managed;
  return undefined;
}
function ranges(buffer: TimeRanges): BufferedRange[] {
  return Array.from({ length: buffer.length }, (_, i) => ({
    start: buffer.start(i),
    end: buffer.end(i),
  }));
}
function delay(ms: number, signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    const abort = () => {
      clearTimeout(timer);
      reject(new DOMException("Stopped", "AbortError"));
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, ms);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
  });
}
function waitEvent(target: EventTarget, name: string, signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => {
      cleanup();
      reject(new Error(`Streaming ${name} timed out. Retry the preview.`));
    }, 10_000);
    const cleanup = () => {
      clearTimeout(timeout);
      target.removeEventListener(name, done);
      target.removeEventListener("error", failed);
      signal.removeEventListener("abort", aborted);
    };
    const done = () => {
      cleanup();
      resolve();
    };
    const failed = () => {
      cleanup();
      reject(new Error("The webview could not decode this stream."));
    };
    const aborted = () => {
      cleanup();
      reject(new DOMException("Stopped", "AbortError"));
    };
    target.addEventListener(name, done, { once: true });
    target.addEventListener("error", failed, { once: true });
    signal.addEventListener("abort", aborted, { once: true });
    if (signal.aborted) aborted();
  });
}

/** One MSE append loop. Generation checks and AbortController fence every async boundary. */
export class StreamingPlayer {
  private source?: MediaSource;
  private buffer?: SourceBuffer;
  private url?: string;
  private session?: string;
  private abort?: AbortController;
  private timer?: ReturnType<typeof setInterval>;
  private generation = 0;
  private disposed = false;
  private loaded = false;
  private blob = false;
  private tickPending = false;
  private playhead = 0;
  private firstFrame?: (event: Event) => void;
  constructor(
    private video: HTMLVideoElement,
    private clipId: string,
    private tracks: number[],
    readonly duration: number,
    private callbacks: {
      busy: (busy: boolean) => void;
      status: (status: StreamStatus) => void;
      encoder: (encoder: string) => void;
      position: (time: number) => void;
      error: (error: string) => void;
      metrics: (metrics: PlaybackMetrics) => void;
    },
    private frameStep = 1 / 24,
  ) {}
  buffered() {
    return ranges(this.video.buffered);
  }
  seek(time: number) {
    if (!Number.isFinite(time)) return false;
    const action = seekAction(
      this.buffered(),
      time,
      this.duration,
      this.frameStep,
    );
    this.callbacks.position(action.target);
    if (this.blob) {
      this.video.currentTime = action.target;
      return false;
    }
    if (action.restart) {
      void this.open(action.target, !this.video.paused);
      return true;
    }
    this.video.currentTime = action.target;
    return false;
  }
  private release() {
    this.abort?.abort();
    if (this.timer) clearInterval(this.timer);
    if (this.firstFrame)
      this.video.removeEventListener("loadeddata", this.firstFrame);
    if (this.buffer?.updating && this.source?.readyState === "open") {
      try {
        this.buffer.abort();
      } catch {
        /* source already detached */
      }
    }
    this.video.pause();
    this.video.removeAttribute("src");
    this.video.load();
    if (this.url) URL.revokeObjectURL(this.url);
    this.url = undefined;
    this.buffer = undefined;
    this.source = undefined;
    this.loaded = false;
    this.blob = false;
    if (this.session)
      void invoke("stop_stream", { sessionId: this.session }).catch(() => {});
    this.session = undefined;
  }
  dispose() {
    this.disposed = true;
    this.generation++;
    this.release();
  }
  async open(start = 0, autoplay = true) {
    if (this.disposed) return;
    const generation = ++this.generation;
    this.release();
    const id = crypto.randomUUID();
    this.session = id;
    this.playhead = start;
    this.callbacks.busy(true);
    this.callbacks.error("");
    this.callbacks.position(start);
    const abort = new AbortController();
    this.abort = abort;
    const { signal } = abort;
    const began = performance.now();
    const current = () =>
      !this.disposed && generation === this.generation && !signal.aborted;
    const fetchPart = async (part: string) => {
      while (true) {
        const response = await fetch(
          `${convertFileSrc(id, "preview")}/${part}`,
          { signal, cache: "no-store" },
        );
        if (response.status === 408) {
          await delay(50, signal);
          continue;
        }
        if (response.status === 410)
          throw new Error(
            "This buffered range was released. Seek again to restart playback.",
          );
        if (!response.ok)
          throw new Error(
            (await response.text()) ||
              `Streaming request failed (${response.status})`,
          );
        return response;
      }
    };
    const append = async (data: ArrayBuffer) => {
      if (!current() || !this.buffer) return;
      const done = waitEvent(this.buffer, "updateend", signal);
      this.buffer.appendBuffer(data);
      await done;
    };
    try {
      const linux = /linux/i.test(navigator.platform);
      const transport =
        linux && !mediaSourceFor('video/mp4; codecs="vp09.00.10.08,opus"')
          ? mediaSourceFor('video/webm; codecs="vp9,vorbis"')
            ? "webm"
            : "blob"
          : "fmp4";
      const openedStream = await invoke<{
        first_sequence: number;
        reused: boolean;
      }>("open_stream", {
        clipId: this.clipId,
        tracks: this.tracks,
        sessionId: id,
        // The bounded Linux Blob fallback needs the complete timeline for seeking.
        start: transport === "blob" ? 0 : start,
        transport,
      });
      if (!current()) {
        await invoke("stop_stream", { sessionId: id });
        return;
      }
      const tick = async () => {
        if (this.tickPending || !current()) return;
        this.tickPending = true;
        try {
          this.playhead = this.loaded ? this.video.currentTime : start;
          const status = await invoke<StreamStatus>("stream_tick", {
            sessionId: id,
            playhead: this.playhead,
          });
          if (current()) this.callbacks.status(status);
        } catch (error) {
          if (current()) this.callbacks.error(String(error));
        } finally {
          this.tickPending = false;
        }
      };
      void tick();
      this.timer = setInterval(() => void tick(), 250);
      const response = await fetchPart("init");
      const mime = response.headers.get("Content-Type") || "";
      if (response.headers.get("X-Preview-Mode") === "blob") {
        if (!linux) throw new Error("Blob transport is restricted to Linux.");
        const data = await response.blob();
        if (!current()) return;
        this.blob = true;
        this.loaded = true;
        this.callbacks.encoder(
          response.headers.get("X-Preview-Encoder") || mime,
        );
        this.firstFrame = () => {
          if (current()) {
            this.callbacks.busy(false);
            this.callbacks.metrics({
              firstFrameMs: performance.now() - began,
              seek: start > 0,
            });
          }
        };
        this.video.addEventListener("loadeddata", this.firstFrame, {
          once: true,
        });
        this.url = URL.createObjectURL(data);
        this.video.addEventListener(
          "loadedmetadata",
          () => {
            if (current()) this.video.currentTime = start;
          },
          { once: true, signal },
        );
        this.video.src = this.url;
        if (autoplay) await this.video.play();
        return;
      }
      const Constructor = mediaSourceFor(mime);
      if (!Constructor)
        throw new Error(`This webview does not support streaming ${mime}.`);
      if (!current()) return;
      this.callbacks.encoder(response.headers.get("X-Preview-Encoder") || mime);
      const source = new Constructor();
      this.source = source;
      this.video.disableRemotePlayback = true;
      const opened = waitEvent(source, "sourceopen", signal);
      this.url = URL.createObjectURL(source);
      this.video.src = this.url;
      await opened;
      source.duration = this.duration;
      this.buffer = source.addSourceBuffer(mime);
      this.buffer.mode = "segments";
      await append(await response.arrayBuffer());
      let sequence = openedStream.first_sequence;
      let first = true;
      let complete = false;
      let retained: { start: number; end: number; bytes: number }[] = [];
      while (current()) {
        // Both sides impose the read window. A paused player does not drain a long clip.
        const last = retained[retained.length - 1];
        if (
          complete ||
          (last &&
            (last.end >= this.playhead + 60 ||
              retained.reduce((n, c) => n + c.bytes, 0) >=
                CLIENT_BYTES - 16_777_216))
        ) {
          await delay(100, signal);
        } else {
          const fragment = await fetchPart(String(sequence));
          if (fragment.status === 204) {
            if (source.readyState === "open" && !this.buffer.updating)
              source.endOfStream();
            complete = true;
            continue;
          }
          const data = await fragment.arrayBuffer();
          await append(data);
          retained.push({
            start: Number(fragment.headers.get("X-Preview-Start")),
            end: Number(fragment.headers.get("X-Preview-End")),
            bytes: data.byteLength,
          });
          sequence++;
          if (first) {
            first = false;
            this.loaded = true;
            this.video.currentTime = Math.max(
              start,
              this.video.buffered.length ? this.video.buffered.start(0) : start,
            );
            this.firstFrame = () => {
              if (current()) {
                this.callbacks.busy(false);
                this.callbacks.metrics({
                  firstFrameMs: performance.now() - began,
                  seek: start > 0,
                });
              }
            };
            if (this.video.readyState >= 2)
              this.firstFrame(new Event("loadeddata"));
            else
              this.video.addEventListener("loadeddata", this.firstFrame, {
                once: true,
              });
            if (autoplay)
              this.video.play().catch((error) => {
                if (current()) this.callbacks.error(String(error));
              });
          }
        }
        const trim = this.playhead - 30;
        if (
          trim > 0 &&
          this.buffer.buffered.length &&
          this.buffer.buffered.start(0) < trim - 0.5
        ) {
          const done = waitEvent(this.buffer, "updateend", signal);
          this.buffer.remove(0, trim);
          await done;
        }
        retained = retained.filter((c) => c.end >= trim);
        if (complete && source.readyState === "open" && !this.buffer.updating)
          source.endOfStream();
      }
    } catch (error) {
      if (current()) {
        if (this.timer) clearInterval(this.timer);
        this.abort?.abort();
        this.callbacks.error(String(error));
        this.callbacks.busy(false);
        void invoke("stop_stream", { sessionId: id }).catch(() => {});
      }
    }
  }
}
