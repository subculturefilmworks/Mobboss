import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { bytes, requiresDestination, type MediaFile } from "./model";
type Op = "move" | "copy" | "collect";
interface Action {
  op: Op;
  source: string;
  dest: string;
  duplicate: boolean;
  step: number;
  file: MediaFile;
}
interface Plan {
  project: string;
  actions: Action[];
  drives: {
    root: string;
    files: number;
    bytes: number;
    required: number;
    available: number;
  }[];
  warnings: string[];
  conflicts: string[];
}
interface Result {
  logs: string[];
  completed: number;
  skipped: number;
  cancelled: boolean;
  warnings: string[];
  touched: string[];
}
interface Progress {
  done: number;
  total: number;
  source: string;
}
interface LogRow {
  op: string;
  source: string;
  dest: string;
  size: number;
  checksum: string;
  status: string;
}
export default function Operations({
  disabled,
  files,
  project,
  selected,
  onBusy,
  onRefresh,
  onError,
}: {
  disabled: boolean;
  files: MediaFile[];
  project: string;
  selected: Set<string>;
  onBusy: (b: boolean) => void;
  onRefresh: () => Promise<void>;
  onError: (s: string) => void;
}) {
  const [menu, setMenu] = useState(false);
  const [op, setOp] = useState<Op>("collect");
  const [stage, setStage] = useState<
    "" | "setup" | "planning" | "preview" | "confirm" | "running" | "result"
  >("");
  const [name, setName] = useState("");
  const [destination, setDestination] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [undo, setUndo] = useState<{
    rows: LogRow[];
    drives: Plan["drives"];
    warnings: string[];
  } | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const [sure, setSure] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const listeners = [
      listen<Progress>("operation-progress", (e) => setProgress(e.payload)),
      listen<{ Ok?: Result; Err?: string }>("operation-finished", async (e) => {
        onBusy(false);
        if (e.payload.Err) {
          onError(e.payload.Err);
          setStage("");
        } else {
          setResult(e.payload.Ok!);
          setStage("result");
        }
        await onRefresh().catch((e) => onError(String(e)));
      }),
    ];
    return () => {
      listeners.forEach((p) => {
        p.then((f) => f());
      });
    };
  }, [onBusy, onRefresh, onError]);
  async function act(fn: () => Promise<void>) {
    try {
      setError("");
      await fn();
    } catch (e) {
      setError(String(e));
      onError(String(e));
      if (stage === "planning") setStage("setup");
    }
  }
  function begin(operation: Op) {
    setOp(operation);
    setName(
      project ||
        (selected.size
          ? files.find((f) => selected.has(f.path))?.metadata.project ||
            "Selection"
          : ""),
    );
    setError("");
    setDestination("");
    setPlan(null);
    setUndo(null);
    setSure(false);
    setMenu(false);
    setStage("setup");
  }
  async function preview() {
    setStage("planning");
    try {
      const value = await invoke<Plan>("plan_operation", {
        op,
        project: name,
        paths: selected.size
          ? [...selected]
          : files
              .filter((f) => (f.metadata.project || "Unknown") === project)
              .map((f) => f.path),
        destination: destination || null,
      });
      setPlan(value);
      setStage("preview");
    } catch (e) {
      setStage("setup");
      throw e;
    }
  }
  async function undoLog(path?: string) {
    if (!path) {
      const chosen = await open({
        title: "Choose a collect log",
        filters: [{ name: "Operation log", extensions: ["csv"] }],
      });
      if (typeof chosen !== "string") return;
      path = chosen;
    }
    setUndo(
      await invoke<{
        rows: LogRow[];
        drives: Plan["drives"];
        warnings: string[];
      }>("plan_undo", { path }),
    );
    setPlan(null);
    setSure(false);
    setMenu(false);
    setStage("preview");
  }
  async function execute() {
    setProgress(null);
    setStage("running");
    onBusy(true);
    try {
      await invoke("start_operation", { confirmed: sure, undo: undo !== null });
    } catch (e) {
      onBusy(false);
      setStage("confirm");
      throw e;
    }
  }
  const operationFiles = selected.size
    ? files.filter((f) => selected.has(f.path))
    : files.filter((f) => (f.metadata.project || "Unknown") === project);
  const needsDestination = requiresDestination(operationFiles);
  const blocked = (plan?.drives || undo?.drives || []).some(
    (d) => d.available < d.required,
  );
  return (
    <>
      <div className="operations-menu">
        <button disabled={disabled} onClick={() => setMenu(!menu)}>
          Actions ▾
        </button>
        {menu && (
          <div className="actions-dropdown">
            {(["move", "copy", "collect"] as Op[]).map((o) => (
              <button
                key={o}
                disabled={!selected.size && !project}
                onClick={() => begin(o)}
              >
                {o[0].toUpperCase() + o.slice(1)}{" "}
                {selected.size ? `${selected.size} selected files` : "project"}…
              </button>
            ))}
            <button onClick={() => act(() => undoLog())}>Undo from log…</button>
            {!selected.size && !project && (
              <p className="hint">Choose one project or select media rows.</p>
            )}
          </div>
        )}
      </div>
      {stage && (
        <div className="modal-backdrop">
          <section
            className="modal operation-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="operation-title"
          >
            <h2 id="operation-title">
              {stage === "result"
                ? "Operation complete"
                : stage === "running"
                  ? "Working safely…"
                  : undo
                    ? "Undo transfers"
                    : `${op[0].toUpperCase() + op.slice(1)} media`}
            </h2>
            {error && (
              <p role="alert" className="operation-error">
                {error}
              </p>
            )}
            {stage === "setup" && (
              <>
                <p>
                  {selected.size
                    ? `${selected.size} selected indexed files`
                    : `All indexed media for ${project}`}
                </p>
                <label>
                  Project folder name
                  <input
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                  />
                </label>
                <p className="hint">
                  {op === "collect"
                    ? "Step 1: Move media into Media By Project on each source drive. Step 2 (optional): Copy the collected files to a destination."
                    : op === "move"
                      ? "Move into Media By Project on each source drive."
                      : "Copy and verify media at a destination. Originals stay in place."}
                </p>
                {
                  <label>
                    {op === "copy" || needsDestination
                      ? "Destination root (required)"
                      : op === "move"
                        ? "Optional destination root (defaults to each source drive)"
                        : "Optional copy destination root"}
                    <div className="destination-picker">
                      <input
                        value={destination}
                        onChange={(e) => setDestination(e.target.value)}
                        placeholder="Choose a folder or drive…"
                      />
                      <button
                        onClick={() =>
                          act(async () => {
                            const d = await open({
                              directory: true,
                              title: "Choose destination root",
                            });
                            if (typeof d === "string") setDestination(d);
                          })
                        }
                      >
                        Choose…
                      </button>
                    </div>
                  </label>
                }
                {needsDestination && (
                  <p className="hint">
                    Some selected media is outside an Avid MediaFiles tree.
                    Choose a destination root explicitly; Collect will move that
                    media directly into the selected root’s project collection.
                  </p>
                )}
                <div className="modal-actions">
                  <button onClick={() => setStage("")}>Cancel</button>
                  <button
                    className="primary"
                    disabled={
                      !name.trim() ||
                      ((op === "copy" || needsDestination) && !destination)
                    }
                    onClick={() => act(preview)}
                  >
                    Build dry-run plan
                  </button>
                </div>
              </>
            )}
            {stage === "planning" && (
              <p role="status">
                Checking indexed files, checksums, conflicts and free space…
              </p>
            )}
            {stage === "preview" && (
              <>
                <p className="eyebrow">DRY RUN · NO FILES CHANGED</p>
                {plan && (
                  <>
                    <p>
                      {plan.actions.length} planned actions for{" "}
                      <strong>{plan.project}</strong>
                    </p>
                    {plan.actions.some((a) => a.step === 1) && (
                      <p>
                        Step 1: Move{" "}
                        {plan.actions.filter((a) => a.step === 0).length} files
                        on their source drives.
                        <br />
                        Step 2: Copy{" "}
                        {plan.actions.filter((a) => a.step === 1).length} files
                        to the destination.
                      </p>
                    )}
                    <div className="plan-drives">
                      {plan.drives.map((d) => (
                        <div key={d.root}>
                          <strong>{d.root}</strong>
                          <p>
                            {d.files} actions ·{" "}
                            <span title={`${d.bytes} bytes`}>
                              {bytes(d.bytes)}
                            </span>{" "}
                            ·{" "}
                            <span title={`${d.available} bytes`}>
                              {bytes(d.available)}
                            </span>{" "}
                            free
                          </p>
                          <small>
                            Required including margin:{" "}
                            <span title={`${d.required} bytes`}>
                              {bytes(d.required)}
                            </span>
                          </small>
                        </div>
                      ))}
                    </div>
                    {plan.conflicts.length > 0 && (
                      <details open>
                        <summary>
                          {plan.conflicts.length} conflicts resolved by renaming
                        </summary>
                        <pre>{plan.conflicts.join("\n")}</pre>
                      </details>
                    )}
                    <ul className="operation-warnings">
                      {plan.warnings.map((w, i) => (
                        <li key={i}>{w}</li>
                      ))}
                    </ul>
                    <details>
                      <summary>View every action</summary>
                      <div className="action-list">
                        {plan.actions.map((a, i) => (
                          <div key={i}>
                            <b>
                              Step {a.step + 1} ·{" "}
                              {a.duplicate ? "Skip duplicate" : a.op}
                            </b>
                            <p>{a.source}</p>
                            <p>→ {a.dest}</p>
                          </div>
                        ))}
                      </div>
                    </details>
                  </>
                )}
                {undo && (
                  <>
                    <p>
                      Reverse moves and delete verified copies, in reverse
                      order.
                    </p>
                    <div className="plan-drives">
                      {undo.drives.map((d) => (
                        <div key={d.root}>
                          <strong>{d.root}</strong>
                          <p>
                            {d.files} move-backs ·{" "}
                            <span title={`${d.available} bytes`}>
                              {bytes(d.available)}
                            </span>{" "}
                            free ·{" "}
                            <span title={`${d.required} bytes`}>
                              {bytes(d.required)}
                            </span>{" "}
                            required with margin
                          </p>
                        </div>
                      ))}
                    </div>
                    <ul className="operation-warnings">
                      {undo.warnings.map((w, i) => (
                        <li key={i}>{w}</li>
                      ))}
                    </ul>
                    <div className="action-list">
                      {undo.rows.map((r, i) => (
                        <div key={i}>
                          <b>Undo {r.op}</b>
                          <p>{r.dest}</p>
                          <p>→ {r.source}</p>
                        </div>
                      ))}
                    </div>
                  </>
                )}
                <div className="modal-actions">
                  <button onClick={() => setStage("")}>Close dry run</button>
                  <button
                    className="primary"
                    disabled={
                      blocked ||
                      plan?.actions.length === 0 ||
                      undo?.rows.length === 0
                    }
                    onClick={() => {
                      setSure(false);
                      setStage("confirm");
                    }}
                  >
                    Continue…
                  </button>
                </div>
              </>
            )}
            {stage === "confirm" && (
              <>
                <div className="quit-warning">
                  <strong>Quit Media Composer before moving media</strong>
                  <p>
                    Close Avid on every computer using these files before
                    continuing.
                  </p>
                </div>
                <p>
                  {undo
                    ? "This will reverse the logged transfers. Copies are deleted only while their checksums match."
                    : `This will execute ${plan?.actions.length} planned actions. Moves remove the source after safe transfer.`}
                </p>
                <p>
                  Avid will rebuild its media databases on next launch. Point
                  Avid at the collected Avid MediaFiles tree, or use the project
                  folder as a NEXIS workspace root.
                </p>
                <label className="confirm-check">
                  <input
                    type="checkbox"
                    checked={sure}
                    onChange={(e) => setSure(e.target.checked)}
                  />
                  Yes, I’m sure. Media Composer is closed.
                </label>
                <div className="modal-actions">
                  <button onClick={() => setStage("preview")}>
                    Back to plan
                  </button>
                  <button
                    className="primary"
                    disabled={!sure}
                    onClick={() => act(execute)}
                  >
                    Confirm and {undo ? "undo" : op}
                  </button>
                </div>
              </>
            )}
            {stage === "running" && (
              <>
                <p>
                  {progress?.done || 0} /{" "}
                  {progress?.total || plan?.actions.length || "…"} files
                </p>
                <progress
                  max={progress?.total || 1}
                  value={progress?.done || 0}
                />
                <p className="hint">{progress?.source}</p>
                <p>
                  Cancel finishes the current file safely. Completed actions
                  remain logged and can be undone.
                </p>
                <div className="modal-actions">
                  <button
                    onClick={() =>
                      act(async () => {
                        await invoke("cancel");
                      })
                    }
                  >
                    Cancel after current file
                  </button>
                </div>
              </>
            )}
            {stage === "result" && result && (
              <>
                <p className="result-count">
                  {result.completed} completed · {result.skipped} skipped
                  {result.cancelled ? " · Cancelled" : ""}
                </p>
                <ul className="operation-warnings">
                  {result.warnings.map((w, i) => (
                    <li key={i}>{w}</li>
                  ))}
                </ul>
                <details open>
                  <summary>Touched media folders</summary>
                  <pre>{result.touched.join("\n")}</pre>
                </details>
                <details>
                  <summary>CSV logs</summary>
                  <pre>{result.logs.join("\n")}</pre>
                </details>
                <div className="modal-actions">
                  <button
                    onClick={() =>
                      act(async () => {
                        await invoke("reveal_log", { path: result.logs[0] });
                      })
                    }
                  >
                    Reveal log ↗
                  </button>
                  {!undo && (
                    <button onClick={() => act(() => undoLog(result.logs[0]))}>
                      Undo…
                    </button>
                  )}
                  <button className="primary" onClick={() => setStage("")}>
                    Done
                  </button>
                </div>
              </>
            )}
          </section>
        </div>
      )}
    </>
  );
}
