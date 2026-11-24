import { afterEach, describe, expect, it, vi } from "vitest";
import { isBuffered, seekAction, StreamingPlayer } from "./streaming";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  convertFileSrc: (id: string) => `preview://localhost/${id}`,
}));
afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("streaming seek state", () => {
  const ranges = [
    { start: 0, end: 10 },
    { start: 30, end: 60 },
  ];
  it("keeps seeks and frame steps inside the available buffer", () => {
    expect(seekAction(ranges, 5, 120)).toEqual({ target: 5, restart: false });
    expect(seekAction(ranges, 30 + 1 / 24, 120).restart).toBe(false);
    expect(isBuffered(ranges, 0)).toBe(true);
  });
  it("restarts for holes, evicted history and future ranges", () => {
    for (const time of [10, 20, 60, 100])
      expect(seekAction(ranges, time, 120).restart).toBe(true);
    expect(seekAction([{ start: 30, end: 60 }], 5, 120).restart).toBe(true);
  });
  it("clamps scrub and reverse targets to the clip timeline", () => {
    expect(seekAction(ranges, -1, 120)).toEqual({ target: 0, restart: false });
    expect(seekAction(ranges, 999, 120)).toEqual({
      target: 120 - 1 / 24,
      restart: true,
    });
  });
});

it("fences late opens and closes by session generation without fetching stale bytes", async () => {
  const opens: (() => void)[] = [];
  invoke.mockImplementation((command: string) =>
    command === "open_stream"
      ? new Promise((resolve) =>
          opens.push(() => resolve({ first_sequence: 0, reused: false })),
        )
      : Promise.resolve(),
  );
  vi.stubGlobal("crypto", {
    randomUUID: vi
      .fn()
      .mockReturnValueOnce("session-one")
      .mockReturnValueOnce("session-two"),
  });
  vi.stubGlobal("navigator", { platform: "Win32" });
  const fetch = vi.fn();
  vi.stubGlobal("fetch", fetch);
  const video = {
    pause: vi.fn(),
    removeAttribute: vi.fn(),
    load: vi.fn(),
  } as unknown as HTMLVideoElement;
  const player = new StreamingPlayer(video, "clip", [0, 1], 120, {
    busy: vi.fn(),
    status: vi.fn(),
    encoder: vi.fn(),
    position: vi.fn(),
    error: vi.fn(),
    metrics: vi.fn(),
  });
  const first = player.open(0);
  const second = player.open(90);
  opens[0]();
  await first;
  expect(invoke).toHaveBeenCalledWith("stop_stream", {
    sessionId: "session-one",
  });
  expect(invoke).not.toHaveBeenCalledWith("stop_stream", {
    sessionId: "session-two",
  });
  player.dispose();
  opens[1]();
  await second;
  expect(invoke).toHaveBeenCalledWith("stop_stream", {
    sessionId: "session-two",
  });
  expect(fetch).not.toHaveBeenCalled();
});
