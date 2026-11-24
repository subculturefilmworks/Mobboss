import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "./Dialog";
import MediaTable, { useStored } from "./MediaTable";
import {
  bytes,
  filename,
  diffRows,
  filter,
  projects,
  summaryCsv,
  summaryHtml,
  tableCsv,
  totalSize,
  type MediaFile,
  type Settings,
  type Volume,
  type Progress,
  type Report,
  type Filters,
} from "./model";
import "./style.css";
import Operations from "./Operations";
import { usePreviews } from "./Previews";
import ClipTable from "./ClipTable";
import { groupClips } from "./model";
const desktop = "__TAURI_INTERNALS__" in window;
const emptyFilters: Filters = {
  search: "",
  projects: [],
  track: "",
  codec: "",
  folder: "",
  diff: "",
};
function App() {
  const [files, setFiles] = useState<MediaFile[]>([]);
  const previews = usePreviews(desktop, files);
  const [archived, setArchived] = useState<MediaFile[]>([]);
  const [settings, setSettings] = useState<Settings>({
    index: "",
    roots: [],
    concurrency: 8,
  });
  const [volumes, setVolumes] = useState<Volume[]>([]);
  const [view, setView] = useStored("mobboss.view", "files");
  const [mode, setMode] = useStored("mobboss.mode", "quick");
  const [theme, setTheme] = useStored("mobboss.theme", "dark");
  const [filters, setFilters] = useState<Filters>(emptyFilters);
  const [selected, setSelected] = useState(new Set<string>());
  const [progress, setProgress] = useState<Progress | null>(null);
  const [busy, setBusy] = useState(false);
  const [operating, setOperating] = useState(false);
  const operationBusy = useCallback((b: boolean) => {
    setBusy(b);
    setOperating(b);
  }, []);
  const refresh = useCallback(async () => {
    setFiles(await invoke<MediaFile[]>("files"));
    setSelected(new Set());
    setReports([]);
    setArchived([]);
  }, []);
  const [reports, setReports] = useState<Report[]>([]);
  const [notice, setNotice] = useState(
    desktop
      ? ""
      : "Browser preview · Open the desktop app to scan local media.",
  );
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [ready, setReady] = useState(false);
  const pending = useRef(new Map<string, MediaFile>());
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const unlisten: (() => void)[] = [];
    const timer = window.setInterval(() => {
      if (!pending.current.size) return;
      const update = new Map(pending.current);
      pending.current.clear();
      setFiles((old) => {
        const map = new Map(old.map((f) => [f.path, f]));
        for (const [p, f] of update) map.set(p, f);
        return [...map.values()];
      });
    }, 100);
    (async () => {
      unlisten.push(
        await listen<Progress>("scan-progress", ({ payload: p }) => {
          setProgress(p);
          for (const f of p.files) pending.current.set(f.path, f);
        }),
      );
      unlisten.push(
        await listen<{ Ok?: Report[]; Err?: string }>(
          "scan-finished",
          ({ payload: r }) => {
            pending.current.clear();
            setArchived([]);
            const paths = (r.Ok || []).flatMap((report) => report.diff.missing);
            if (paths.length)
              invoke<MediaFile[]>("missing_files", { paths })
                .then(setArchived)
                .catch((e) => setNotice(String(e)));
            setBusy(false);
            if (r.Err) setNotice(r.Err);
            else {
              setReports(r.Ok || []);
              setNotice(
                `Inventory complete · ${(r.Ok || []).reduce((n, x) => n + x.files, 0).toLocaleString()} verified files`,
              );
            }
            invoke<MediaFile[]>("files")
              .then((f) => {
                setFiles(f);
                setSelected((old) => {
                  const present = new Set(f.map((x) => x.path));
                  return new Set([...old].filter((p) => present.has(p)));
                });
              })
              .catch((e) => setNotice(String(e)));
          },
        ),
      );
      const [s, f, v] = await Promise.all([
        invoke<Settings>("settings"),
        invoke<MediaFile[]>("files"),
        invoke<Volume[]>("volumes"),
      ]);
      if (!disposed) {
        setSettings(s);
        setFiles(f);
        setVolumes(v);
        setReady(true);
      }
    })().catch((e) => setNotice(String(e)));
    return () => {
      disposed = true;
      clearInterval(timer);
      unlisten.forEach((fn) => fn());
    };
  }, []);
  const rows = useMemo(
    () => diffRows(files, reports, archived),
    [files, reports, archived],
  );
  const filtered = useMemo(
    () =>
      filter(
        filters.diff ? rows : rows.filter((r) => r.values.diff !== "missing"),
        filters,
      ),
    [rows, filters],
  );
  const clips = useMemo(() => {
    const matching = new Set(filtered.map((r) => r.file.path));
    return groupClips(rows.map((r) => r.file)).filter((c) =>
      c.files.some((f) => matching.has(f.path)),
    );
  }, [rows, filtered]);
  const summary = useMemo(() => projects(files), [files]);
  const selectedRows = useMemo(
    () => rows.filter((r) => selected.has(r.file.path)),
    [rows, selected],
  );
  const setFilter = <K extends keyof Filters>(key: K, value: Filters[K]) =>
    setFilters((old) => ({ ...old, [key]: value }));
  const run = (action: () => Promise<unknown>) => {
    action().catch((e) => setNotice(String(e)));
  };
  async function persist(value: Settings) {
    const saved = await invoke<Settings>("save_settings", { value });
    setSettings(saved);
  }
  async function scan(roots = settings.roots) {
    if (!roots.length) return;
    await persist({ ...settings, roots });
    pending.current.clear();
    setReports([]);
    setArchived([]);
    setProgress(null);
    setBusy(true);
    setNotice("Scanning read-only…");
    try {
      await invoke("start_scan", { mode });
    } catch (e) {
      setBusy(false);
      throw e;
    }
  }
  async function pick() {
    const choice = await open({
      directory: true,
      multiple: true,
      title: "Choose media sources",
    });
    if (choice) {
      const roots = [
        ...new Set([
          ...settings.roots,
          ...(Array.isArray(choice) ? choice : [choice]),
        ]),
      ];
      await scan(roots);
    }
  }
  async function exportContent(name: string, content: string) {
    if (desktop) {
      const path = await save({
        defaultPath: name,
        filters: [
          {
            name: name.endsWith(".html") ? "HTML" : "CSV",
            extensions: [name.split(".").pop()!],
          },
        ],
      });
      if (path) await invoke("export_report", { path, content });
    } else {
      const url = URL.createObjectURL(
        new Blob([content], {
          type: name.endsWith(".html") ? "text/html" : "text/csv",
        }),
      );
      const a = document.createElement("a");
      a.href = url;
      a.download = name;
      a.click();
      URL.revokeObjectURL(url);
    }
  }
  const codecs = [
    ...new Set(files.map((f) => f.metadata.codec).filter(Boolean)),
  ].sort() as string[];
  const tracks = [
    ...new Set(files.map((f) => f.metadata.track_kind).filter(Boolean)),
  ].sort() as string[];
  const folders = [...new Set(files.map((f) => f.folder))].sort();
  return (
    <div className="app">
      <header>
        <div className="brand-icon">M</div>
        <div className="brand">
          <h1>Mob Boss</h1>
          <p>Media management for Avid Media Composer</p>
        </div>
        <span className="readonly">● PLAN FIRST</span>
        <div className="header-actions">
          <Operations
            disabled={!ready || busy}
            files={files}
            project={filters.projects.length === 1 ? filters.projects[0] : ""}
            selected={selected}
            onBusy={operationBusy}
            onRefresh={refresh}
            onError={setNotice}
          />
          <button onClick={() => setTheme(theme === "dark" ? "light" : "dark")}>
            {theme === "dark" ? "☀ Light" : "☾ Dark"}
          </button>
          <button onClick={() => setSettingsOpen(true)}>Settings</button>
        </div>
      </header>
      <div className="workspace">
        <aside>
          <div className="section-label">
            SOURCES <span>{settings.roots.length}</span>
          </div>
          <button
            className="primary scan-button"
            disabled={!ready || busy}
            onClick={() => run(pick)}
          >
            ＋ Scan…
          </button>
          <div className="scan-options">
            <label htmlFor="mode">Scan mode</label>
            <select
              id="mode"
              value={mode}
              onChange={(e) => setMode(e.target.value)}
              disabled={busy}
            >
              <option value="quick">Quick</option>
              <option value="auto">Auto</option>
              <option value="deep">Deep</option>
            </select>
          </div>
          <p className="hint">
            {mode === "deep"
              ? "Verify every MXF header."
              : mode === "auto"
                ? "Adapt scanning to each media folder."
                : "Show PMR inventory first, then fill in details."}
          </p>
          <div className="sources">
            {settings.roots.map((root) => (
              <div className="source" key={root}>
                <button
                  title={root}
                  disabled={busy}
                  onClick={() => setFilter("folder", root)}
                >
                  ▱ {root.split(/[\\/]/).pop() || root}
                </button>
                <button
                  aria-label={`Remove source ${root}`}
                  disabled={busy}
                  onClick={() =>
                    run(() =>
                      persist({
                        ...settings,
                        roots: settings.roots.filter((r) => r !== root),
                      }),
                    )
                  }
                >
                  ×
                </button>
              </div>
            ))}
          </div>
          {settings.roots.length > 0 && (
            <button
              className="wide"
              disabled={!ready || busy}
              onClick={() => run(() => scan())}
            >
              Rescan all sources
            </button>
          )}
          <details className="volumes" open>
            <summary>Mounted volumes & shares</summary>
            {volumes.map((v) => (
              <button
                className="volume"
                title={v.path}
                key={v.path}
                disabled={busy}
                onClick={() =>
                  run(() => scan([...new Set([...settings.roots, v.path])]))
                }
              >
                <span>▱ {v.name || v.path}</span>
                <small>
                  <span title={`${v.available} bytes`}>
                    {bytes(v.available)}
                  </span>{" "}
                  free
                </small>
              </button>
            ))}
            {!volumes.length && (
              <p className="hint">
                Mounted local and network volumes appear here.
              </p>
            )}
          </details>
          <div className="section-label projects-label">
            PROJECTS <span>{summary.length}</span>
          </div>
          <button
            className={`project ${!filters.projects.length ? "active" : ""}`}
            onClick={() => setFilter("projects", [])}
          >
            <span>All projects</span>
            <small>
              {files.length.toLocaleString()} ·{" "}
              <span title={`${totalSize(files)} bytes`}>
                {bytes(totalSize(files))}
              </span>
            </small>
          </button>
          {summary.map((p) => (
            <label
              className={`project ${filters.projects.includes(p.name) ? "active" : ""}`}
              key={p.name}
            >
              <span>
                <input
                  type="checkbox"
                  checked={filters.projects.includes(p.name)}
                  onChange={(e) =>
                    setFilter(
                      "projects",
                      e.target.checked
                        ? [...filters.projects, p.name]
                        : filters.projects.filter((n) => n !== p.name),
                    )
                  }
                />
                {p.name}
              </span>
              <small>
                {p.count.toLocaleString()} files ·{" "}
                <span title={`${p.size} bytes`}>{bytes(p.size)}</span>
              </small>
            </label>
          ))}
          <div className="summary-exports">
            <button
              onClick={() =>
                run(() => exportContent("projects.csv", summaryCsv(files)))
              }
            >
              CSV ↗
            </button>
            <button
              onClick={() =>
                run(() => exportContent("projects.html", summaryHtml(files)))
              }
            >
              HTML ↗
            </button>
          </div>
          <div className="sidebar-foot">
            Verified media management
            <br />
            Your media stays where it belongs.
          </div>
        </aside>
        <main>
          <div className="main-title">
            <div>
              <div className="eyebrow">MEDIA LIBRARY</div>
              <h2>
                {filters.projects.length === 1
                  ? filters.projects[0]
                  : "All media"}
              </h2>
            </div>
            <div className="toolbar">
              <button
                disabled={selectedRows.length !== 1 || !desktop || busy}
                onClick={() =>
                  run(() =>
                    invoke("reveal", { path: selectedRows[0].file.path }),
                  )
                }
              >
                Reveal file ↗
              </button>
              <button
                disabled={!filtered.length}
                onClick={() =>
                  run(() =>
                    exportContent(
                      "mobboss-media.csv",
                      tableCsv(selectedRows.length ? selectedRows : filtered),
                    ),
                  )
                }
              >
                Export {selectedRows.length ? "selected" : "filtered"} CSV ↗
              </button>
            </div>
          </div>
          <div className="search-row">
            <input
              className="search"
              aria-label="Search across all columns"
              placeholder="Search filenames, projects, codecs, paths…"
              value={filters.search}
              onChange={(e) => setFilter("search", e.target.value)}
            />
            <select
              aria-label="Diff view"
              value={filters.diff}
              onChange={(e) => setFilter("diff", e.target.value)}
            >
              <option value="">All inventory</option>
              <option value="all">Show diff</option>
              <option value="missing">In DB · missing on disk</option>
              <option value="unlisted">On disk · not in DB</option>
              <option value="changed">Changed since last scan</option>
            </select>
          </div>
          <div className="filter-row">
            <span>FILTERS</span>
            <select
              aria-label="Track filter"
              value={filters.track}
              onChange={(e) => setFilter("track", e.target.value)}
            >
              <option value="">All tracks</option>
              {tracks.map((t) => (
                <option key={t}>{t}</option>
              ))}
            </select>
            <select
              aria-label="Codec filter"
              value={filters.codec}
              onChange={(e) => setFilter("codec", e.target.value)}
            >
              <option value="">All codecs</option>
              {codecs.map((c) => (
                <option key={c}>{c}</option>
              ))}
            </select>
            <select
              aria-label="Folder or volume filter"
              value={filters.folder}
              onChange={(e) => setFilter("folder", e.target.value)}
            >
              <option value="">All folders / volumes</option>
              {[...new Set([...settings.roots, ...folders])].map((f) => (
                <option key={f}>{f}</option>
              ))}
            </select>
            <button onClick={() => setFilters(emptyFilters)}>
              Clear filters
            </button>
          </div>
          {busy && !operating && (
            <div className="progress-panel" role="status">
              <div>
                <strong>
                  {progress?.phase === "inventory"
                    ? "Inventory ready"
                    : progress?.phase === "enrichment"
                      ? "Reading MXF headers"
                      : progress?.phase === "complete"
                        ? "Finishing"
                        : "Discovering media"}
                </strong>
                <span>
                  {progress?.done || 0} / {progress?.total || "…"}
                </span>
                <button onClick={() => run(() => invoke("cancel"))}>
                  Cancel scan
                </button>
              </div>
              <progress
                max={progress?.total || 1}
                value={progress?.done || 0}
              />
              <small title={progress?.current_folder}>
                {progress?.current_folder || "Listing source folders…"}
              </small>
            </div>
          )}
          <div className="view-toggle" role="group" aria-label="Inventory view">
            <button
              aria-pressed={view === "files"}
              onClick={() => setView("files")}
            >
              Files
            </button>
            <button
              aria-pressed={view === "clips"}
              onClick={() => setView("clips")}
            >
              Clips
            </button>
            <button
              aria-pressed={view === "grid"}
              onClick={() => setView("grid")}
            >
              Grid
            </button>
          </div>
          <div className="preview-status" role="status">
            <span>
              {previews.running
                ? `Thumbnails ${previews.progress?.done || 0} / ${previews.progress?.total || clips.length}`
                : previews.status.message}
              {Object.keys(previews.errors).length > 0
                ? ` · ${Object.keys(previews.errors).length} failed (retry available)`
                : ""}
            </span>
            {previews.running ? (
              <button onClick={() => run(previews.cancel)}>
                Cancel thumbnails
              </button>
            ) : (
              previews.status.available && (
                <button disabled={busy} onClick={() => run(previews.retry)}>
                  Generate / retry thumbnails
                </button>
              )
            )}
          </div>
          {view !== "files" ? (
            <ClipTable
              thumbnailErrors={previews.errors}
              thumbnails={previews.status.thumbnails}
              grid={view === "grid"}
              clips={clips}
              selected={selected}
              setSelected={setSelected}
            />
          ) : (
            <MediaTable
              showDiff={!!filters.diff}
              rows={filtered}
              selected={selected}
              setSelected={setSelected}
            />
          )}
          {reports.map((report, i) => {
            const r = report.reconciliation;
            return (
              r && (
                <details key={i} className="reconciliation">
                  <summary title={r.root}>
                    {r.root}: {r.media_files.toLocaleString()} media files ·{" "}
                    {r.database_files.length} Avid database files (.pmr/.mdb) ·{" "}
                    {r.other_files} other files · {r.folders} folders — total{" "}
                    {r.total_bytes.toLocaleString()} bytes
                  </summary>
                  {r.database_files.map((db) => (
                    <p
                      key={db.path}
                      title={`${db.path} · ${db.description} · Modified ${db.modified_ns ? new Date(Number(db.modified_ns) / 1e6).toLocaleString() : "unknown"}`}
                    >
                      {filename(db.path)} ·{" "}
                      {db.path.toLowerCase().endsWith(".mdb")
                        ? "Avid media database"
                        : "Avid media index"}{" "}
                      ·{" "}
                      <span title={`${db.size.toLocaleString()} bytes`}>
                        {bytes(db.size)}
                      </span>
                      {db.pmr_missing !== null && (
                        <>
                          {" "}
                          · PMR vs MXF:{" "}
                          {db.pmr_missing === 0 && db.pmr_extra === 0
                            ? "agree"
                            : `${db.pmr_missing} missing from disk / ${db.pmr_extra} extra on disk`}
                        </>
                      )}
                    </p>
                  ))}
                  <details>
                    <summary>
                      Other files ({r.other_files}; showing up to 200)
                    </summary>
                    <pre>{r.other_names.join("\n")}</pre>
                  </details>
                </details>
              )
            );
          })}
          <div className="notice" role="status">
            {notice}
            {reports.some((r) => r.warnings.length > 0) && (
              <details>
                <summary>
                  {reports.reduce((n, r) => n + r.warnings.length, 0)}
                  {" scan warning"}
                  {reports.reduce((n, r) => n + r.warnings.length, 0) === 1
                    ? ""
                    : "s"}
                </summary>
                <pre>{reports.flatMap((r) => r.warnings).join("\n")}</pre>
              </details>
            )}
            {filters.diff && (
              <span>
                {" "}
                Diff uses PMR where available, otherwise the prior index.
                Missing rows retain indexed metadata when available.
              </span>
            )}
          </div>
          <footer>
            <span>
              <b>{files.length.toLocaleString()}</b> files ·{" "}
              <span title={`${totalSize(files)} bytes`}>
                {bytes(totalSize(files))}
              </span>
            </span>
            <span>
              Filtered <b>{filtered.length.toLocaleString()}</b> ·{" "}
              <span title={`${totalSize(filtered.map((r) => r.file))} bytes`}>
                {bytes(totalSize(filtered.map((r) => r.file)))}
              </span>
            </span>
            <span className="selected-total">
              Selected <b>{selectedRows.length.toLocaleString()}</b> ·{" "}
              <span
                title={`${totalSize(selectedRows.map((r) => r.file))} bytes`}
              >
                {bytes(totalSize(selectedRows.map((r) => r.file)))}
              </span>
            </span>
            {selected.size > 0 && (
              <button onClick={() => setSelected(new Set())}>
                Clear selection
              </button>
            )}
          </footer>
        </main>
      </div>
      {settingsOpen && (
        <div className="modal-backdrop">
          <section
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="settings-title"
          >
            <h2 id="settings-title">Settings</h2>
            <label>
              Index location
              <input
                value={settings.index}
                onChange={(e) =>
                  setSettings({ ...settings, index: e.target.value })
                }
                disabled={busy}
              />
            </label>
            <p className="hint">
              Default: your local app-data folder. The index and exported
              reports must stay outside media sources.
            </p>
            <label>
              Parallel header reads
              <input
                type="number"
                min="1"
                max="64"
                value={settings.concurrency}
                disabled={busy}
                onChange={(e) =>
                  setSettings({
                    ...settings,
                    concurrency: Number(e.target.value),
                  })
                }
              />
            </label>
            <p className="hint">
              8 is the default. Mounted shares must be connected in Finder or
              Explorer to appear in Sources.
            </p>
            <div className="modal-actions">
              <button
                onClick={() => {
                  if (desktop)
                    run(async () =>
                      setSettings(await invoke<Settings>("settings")),
                    );
                  setSettingsOpen(false);
                }}
              >
                Close
              </button>
              <button
                className="primary"
                disabled={busy || !desktop}
                onClick={() =>
                  run(async () => {
                    await persist(settings);
                    setFiles(await invoke<MediaFile[]>("files"));
                    setSelected(new Set());
                    setReports([]);
                    setSettingsOpen(false);
                    setNotice("Settings saved");
                  })
                }
              >
                Save settings
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
