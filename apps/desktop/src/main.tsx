import SourcePath from "./SourcePath";
import {
  mergeSources,
  rememberMounted,
  projectCheckState,
  checkedSources,
  sameSource,
  type Discovery,
} from "./sources";
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
  groupedInteger,
  sortClips,
  type ClipSort,
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
  type Progress,
  type Report,
  type Filters,
} from "./model";
import "./style.css";
import Operations from "./Operations";
import { usePreviews } from "./Previews";
import Player from "./Player";
import type { Clip } from "./model";
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
  const [player, setPlayer] = useState<Clip | null>(null);
  const [files, setFiles] = useState<MediaFile[]>([]);
  const previews = usePreviews(desktop, files);
  const [archived, setArchived] = useState<MediaFile[]>([]);
  const [settings, setSettings] = useState<Settings>({
    index: "",
    roots: [],
    concurrency: 8,
    mounted_sources: [],
    mounted_checks: {},
    custom_folders: [],
  });
  const [denied, setDenied] = useState<{ name: string; path: string }[]>([]);
  const [searching, setSearching] = useState(false);
  const [view, setView] = useStored("mobboss.view", "files");
  const [clipSort, setClipSort] = useStored<ClipSort>("mobboss.clip-sort", {
    key: "name",
    desc: false,
  });
  const [visibleClips, setVisibleClips] = useState<string[]>([]);
  const viewportChanged = useCallback(
    (ids: string[]) =>
      setVisibleClips((old) =>
        old.length === ids.length && old.every((id, i) => id === ids[i])
          ? old
          : ids,
      ),
    [],
  );
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
  const [includeThumbnails, setIncludeThumbnails] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const settingsSave = useRef<Promise<unknown>>(Promise.resolve());
  const settingsRevision = useRef(0);
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
      const [s, f] = await Promise.all([
        invoke<Settings>("settings"),
        invoke<MediaFile[]>("files"),
      ]);
      if (!disposed) {
        setSettings(s);
        setFiles(f);
        setReady(true);
      }
    })().catch((e) => setNotice(String(e)));
    return () => {
      disposed = true;
      clearInterval(timer);
      unlisten.forEach((fn) => fn());
    };
  }, []);
  async function searchDatabases() {
    setSearching(true);
    try {
      const result = await invoke<Discovery>("discover_sources", {
        refresh: true,
      });
      setDenied(result.denied || []);
      await persist({
        ...settings,
        mounted_sources: mergeSources(
          settings.mounted_sources,
          result.found,
          settings.mounted_checks,
        ),
        mounted_checks: rememberMounted(
          settings.mounted_sources,
          settings.mounted_checks,
        ),
        custom_folders: settings.custom_folders.map((row) => ({
          ...row,
          offline: !result.online.some((p) => sameSource(p, row.path)),
        })),
      });
    } finally {
      setSearching(false);
    }
  }
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
    return sortClips(
      groupClips(rows.map((r) => r.file)).filter((c) =>
        c.files.some((f) => matching.has(f.path)),
      ),
      clipSort,
    );
  }, [rows, filtered, clipSort]);
  useEffect(() => {
    if (desktop)
      invoke("prioritize", {
        visible: view === "files" ? [] : visibleClips,
        display: clips.map((c) => c.id),
      }).catch((e) => setNotice(String(e)));
  }, [clips, visibleClips, view]);
  const initialThumbnails = useRef(false);
  useEffect(() => {
    if (
      !desktop ||
      !ready ||
      busy ||
      !previews.status.available ||
      initialThumbnails.current
    )
      return;
    initialThumbnails.current = true;
    invoke("prioritize", {
      visible: view === "files" ? [] : visibleClips,
      display: clips.map((c) => c.id),
    })
      .then(() => invoke("start_thumbnails"))
      .catch((e) => {
        initialThumbnails.current = false;
        setNotice(String(e));
      });
  }, [ready, busy, previews.status.available, clips, visibleClips, view]);
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
    value = {
      ...value,
      mounted_checks: rememberMounted(
        value.mounted_sources,
        value.mounted_checks,
      ),
    };
    const revision = ++settingsRevision.current;
    const previous = settings;
    setSettings(value);
    // Keep rapid checkbox changes in order, including slow/offline settings validation.
    const request = settingsSave.current
      .catch(() => {})
      .then(() => invoke<Settings>("save_settings", { value }));
    settingsSave.current = request;
    try {
      const saved = await request;
      if (settingsRevision.current === revision) setSettings(saved);
    } catch (error) {
      if (settingsRevision.current === revision) setSettings(previous);
      throw error;
    }
  }
  async function scan(roots = checkedSources(settings)) {
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
      const paths = Array.isArray(choice) ? choice : [choice];
      await persist({
        ...settings,
        custom_folders: [
          ...settings.custom_folders,
          ...paths
            .filter(
              (path) =>
                !settings.custom_folders.some((row) =>
                  sameSource(row.path, path),
                ),
            )
            .map((path) => ({
              path,
              checked: true,
              offline: false,
              volume: "",
            })),
        ],
      });
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
        <div className="header-actions">
          <details className="export-menu">
            <summary>Export ▾</summary>
            <div className="export-options">
              {" "}
              <label className="report-thumbnails">
                <input
                  type="checkbox"
                  checked={includeThumbnails}
                  onChange={(e) => setIncludeThumbnails(e.target.checked)}
                />
                Include thumbnails in HTML
              </label>
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
                    run(async () => {
                      const thumbnails =
                        includeThumbnails && desktop
                          ? await invoke<Record<string, string>>(
                              "thumbnail_data",
                              {
                                clipIds: groupClips(files).map((c) => c.id),
                              },
                            )
                          : undefined;
                      await exportContent(
                        "projects.html",
                        summaryHtml(
                          files,
                          includeThumbnails ? thumbnails || {} : undefined,
                        ),
                      );
                    })
                  }
                >
                  HTML ↗
                </button>
              </div>
            </div>
          </details>
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
          <button
            disabled={!ready || busy || searching}
            title="Click to scan attached drives."
            onClick={() => run(searchDatabases)}
          >
            Search for databases
          </button>
          {denied.map((volume) => (
            <div className="permission-hint" key={volume.path}>
              <strong>{volume.name} · Permission needed</strong>
              <p className="hint">
                Mob Boss doesn't have access to {volume.name}. Allow it in
                System Settings › Privacy &amp; Security › Files and Folders.
              </p>
              <button
                onClick={() => run(() => invoke("open_volume_permissions"))}
              >
                Open System Settings
              </button>
            </div>
          ))}
          <button
            className="primary scan-button"
            title="Click to initiate scanning for projects in the selected folders below."
            disabled={
              !ready || busy || searching || !checkedSources(settings).length
            }
            onClick={() => run(() => scan())}
          >
            Scan
          </button>
          <div className="rail-lists">
            {(["mounted_sources", "custom_folders"] as const).map((kind) => (
              <section key={kind} data-source-kind={kind}>
                <div className="section-label">
                  {kind === "mounted_sources"
                    ? "Mounted Sources"
                    : "Custom Folders"}
                  {kind === "custom_folders" && (
                    <button
                      disabled={!ready || busy || searching}
                      onClick={() => run(pick)}
                    >
                      Add folder
                    </button>
                  )}
                </div>
                {settings[kind].map((row) => (
                  <div
                    className={`source-row ${row.offline ? "offline" : ""}`}
                    key={row.path}
                  >
                    <label title={row.path}>
                      <input
                        type="checkbox"
                        checked={row.checked}
                        disabled={busy || searching}
                        onChange={(e) =>
                          run(() =>
                            persist({
                              ...settings,
                              [kind]: settings[kind].map((r) =>
                                sameSource(r.path, row.path)
                                  ? { ...r, checked: e.target.checked }
                                  : r,
                              ),
                            }),
                          )
                        }
                      />
                      <SourcePath row={row} />
                    </label>
                    {kind === "custom_folders" && (
                      <button
                        aria-label={`Remove source ${row.path}`}
                        disabled={busy || searching}
                        onClick={() =>
                          run(() =>
                            persist({
                              ...settings,
                              [kind]: settings[kind].filter(
                                (r) => !sameSource(r.path, row.path),
                              ),
                            }),
                          )
                        }
                      >
                        ×
                      </button>
                    )}
                  </div>
                ))}
              </section>
            ))}
            <div className="section-label projects-label">
              Projects <span>{summary.length}</span>
            </div>
            <div
              className={`project project-all ${!filters.projects.length ? "active" : ""}`}
            >
              <input
                type="checkbox"
                aria-label="All projects"
                checked={
                  projectCheckState(
                    summary.map((p) => p.name),
                    filters.projects,
                  ).checked
                }
                ref={(node) => {
                  if (node)
                    node.indeterminate = projectCheckState(
                      summary.map((p) => p.name),
                      filters.projects,
                    ).indeterminate;
                }}
                onChange={(e) =>
                  setFilter(
                    "projects",
                    e.target.checked ? summary.map((p) => p.name) : [],
                  )
                }
              />
              <button onClick={() => setFilter("projects", [])}>
                <span>All projects</span>
                <small title={`${totalSize(files)} bytes`}>
                  {bytes(totalSize(files))}
                </small>
              </button>
            </div>
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
                  <span title={`${p.size} bytes`}>{bytes(p.size)}</span>
                </small>
              </label>
            ))}
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
              sort={clipSort}
              setSort={setClipSort}
              onViewportChange={viewportChanged}
              thumbnailErrors={previews.errors}
              onPreview={setPlayer}
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
                    {groupedInteger(r.total_bytes)} bytes
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
      {player && (
        <Player
          key={player.id}
          clip={player}
          poster={previews.status.thumbnails[player.id]}
          onClose={() => setPlayer(null)}
        />
      )}
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
              Scan mode
              <select
                value={mode}
                disabled={busy}
                onChange={(e) => setMode(e.target.value)}
              >
                <option value="quick">Quick</option>
                <option value="auto">Auto</option>
                <option value="deep">Deep</option>
              </select>
            </label>
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
              Explorer before clicking Search for databases.
            </p>
            <p className="hint">
              Playback stays in memory and is discarded when the app quits.
              Thumbnails use {bytes(previews.status.thumbnail_bytes || 0)} on
              disk.
            </p>
            <button
              disabled={!desktop || busy}
              onClick={() =>
                run(async () => {
                  await invoke("clear_thumbnails");
                  await previews.refresh();
                  setNotice("Thumbnails cleared");
                })
              }
            >
              Clear thumbnails
            </button>
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
