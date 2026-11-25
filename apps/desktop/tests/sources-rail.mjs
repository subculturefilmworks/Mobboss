import { chromium } from "playwright";
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
});
const page = await browser.newPage({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 1,
});
await page.addInitScript(() => {
  const make = (
    path,
    checked = true,
    offline = false,
    volume = "Story-Island",
  ) => ({ path, checked, offline, volume });
  window.calls = [];
  window.testSettings = {
    index: "/Catalog-Island/catalog.sqlite",
    roots: [],
    concurrency: 8,
    mounted_sources: [
      make("/Volumes/Story-Island/Avid MediaFiles/MXF/1"),
      make("/Volumes/Story-Island/Avid MediaFiles/MXF/desktop.1", false),
      make(
        "/Volumes/Archive-Island/Avid MediaFiles/MXF/2",
        true,
        true,
        "Archive-Island",
      ),
      make(
        "/Volumes/Legacy-Island/OMFI MediaFiles",
        false,
        false,
        "Legacy-Island",
      ),
    ],
    custom_folders: [
      make("/Volumes/Project-Island/Project Directory/Backup files/MXF/1"),
      make("/Volumes/Project-Island/Selected Media", false),
    ],
  };
  const attached = structuredClone(window.testSettings.mounted_sources);
  let searches = 0;
  let id = 0;
  window.__TAURI_INTERNALS__ = {
    transformCallback: (cb) => {
      window[++id] = cb;
      return id;
    },
    unregisterCallback: () => {},
    invoke: async (cmd, args) => {
      window.calls.push({ cmd, args });
      if (cmd === "settings") return window.testSettings;
      if (cmd === "save_settings") {
        window.testSettings = args.value;
        return args.value;
      }
      if (cmd === "files")
        return ["Moon Orchard", "Paper Harbor", "Amber Transit", ""].map(
          (project, i) => ({
            path: `/Volumes/Story-Island/Avid MediaFiles/MXF/1/clip${i}.mxf`,
            folder: "/Volumes/Story-Island/Avid MediaFiles/MXF/1",
            size: 1234567890 * (i + 1),
            modified_ns: "0",
            metadata: {
              project,
              clip_name: "Scene " + (i + 1),
              track_kind: "video",
            },
          }),
        );
      if (cmd === "preview_status")
        return {
          available: false,
          message: "Previews unavailable",
          thumbnails: {},
        };
      if (cmd === "discover_sources")
        return ++searches > 1
          ? {
              found: attached.map((row) => ({
                ...row,
                available: 0,
                total: 0,
              })),
              denied: [],
              online: attached.map((row) => row.path),
            }
          : {
              found: [
                {
                  path: "/Volumes/Story-Island/Avid MediaFiles/MXF/1",
                  volume: "Story-Island",
                  available: 0,
                  total: 0,
                },
                {
                  path: "/Volumes/New-Island/Avid MediaFiles/MXF/3",
                  volume: "New-Island",
                  available: 0,
                  total: 0,
                },
              ],
              denied: [],
              online: ["/Volumes/Story-Island/Avid MediaFiles/MXF/1"],
            };
      if (cmd === "pick_path")
        return args.save
          ? ["/Catalog-Island/" + args.options.defaultPath]
          : ["/Volumes/Added-Island/Media/MXF/1"];
      return null;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
});
const errors = [];
page.on("pageerror", (e) => errors.push(String(e)));
await page.goto(process.env.UI_URL || "http://127.0.0.1:1420");
await page.getByRole("button", { name: "Scan", exact: true }).waitFor();
await page.waitForTimeout(5500);
await page.evaluate(() => window.dispatchEvent(new Event("focus")));
await page.waitForTimeout(100);
assert.equal(
  await page.evaluate(
    () => window.calls.filter((c) => c.cmd === "discover_sources").length,
  ),
  0,
);
assert.equal(
  await page.getByRole("button", { name: "Scan", exact: true }).isEnabled(),
  true,
);
// Results follow ticks immediately, without a scan; Unknown is last.
assert.equal(
  await page
    .locator("label.project")
    .last()
    .innerText()
    .then((t) => t.includes("Unknown project")),
  true,
);
const storyTick = page
  .locator('[data-source-kind="mounted_sources"] input')
  .first();
await storyTick.uncheck();
assert.equal(await page.locator("label.project").count(), 0);
await storyTick.check();
assert.equal(await page.locator("label.project").count(), 4);
assert.equal(
  await page.evaluate(
    () => window.calls.filter((c) => c.cmd === "start_scan").length,
  ),
  0,
);
assert.equal(
  await page.locator("aside [aria-label='Scan mode'] button").count(),
  3,
);
await page.getByRole("button", { name: "Settings", exact: true }).click();
assert.equal(await page.locator(".modal [aria-label='Scan mode']").count(), 0);
await page.getByRole("button", { name: "Close", exact: true }).click();
// Discovery is explicit, keeps existing checks, and never scans.
await page
  .getByRole("button", { name: "Search for databases", exact: true })
  .click();
await page.waitForFunction(
  () => window.testSettings.mounted_sources.length === 2,
);
const merged = await page.evaluate(() => window.testSettings.mounted_sources);
assert.equal(merged[0].checked, true);
assert.equal(merged[1].checked, true);
assert.equal(
  merged.some((row) => row.path.includes("Archive-Island")),
  false,
);
assert.equal(
  await page.locator('[data-source-kind="mounted_sources"] button').count(),
  0,
);
assert.equal(
  await page
    .locator('[data-source-kind="custom_folders"] .source-row button')
    .count(),
  2,
);
assert.equal(
  await page.evaluate(
    () => window.calls.filter((c) => c.cmd === "discover_sources").length,
  ),
  1,
);
assert.equal(
  await page.evaluate(
    () => window.calls.filter((c) => c.cmd === "start_scan").length,
  ),
  0,
);
// Plugging the original paths back in restores their ticks from settings.
await page
  .getByRole("button", { name: "Search for databases", exact: true })
  .click();
await page.waitForFunction(
  () => window.testSettings.mounted_sources.length === 4,
);
const returned = await page.evaluate(() => window.testSettings.mounted_sources);
assert.deepEqual(
  returned.map((row) => row.checked),
  [true, false, true, false],
);
assert.equal(
  returned.every((row) => !row.offline),
  true,
);
// Summary exports follow project ticks and search, including Unknown project.
await page.locator("label.project").last().locator("input").check();
await page
  .getByPlaceholder("Search filenames, projects, codecs, paths…")
  .fill("clip3");
await page.locator(".export-menu summary").click();
await page.locator(".summary-exports button").first().click();
await page.waitForFunction(() =>
  window.calls.some((c) => c.cmd === "export_report"),
);
const exported = await page.evaluate(
  () =>
    window.calls.filter((c) => c.cmd === "export_report").at(-1).args.content,
);
assert.ok(exported.includes("Unknown project"));
assert.ok(!exported.includes("Moon Orchard"));
await page
  .getByPlaceholder("Search filenames, projects, codecs, paths…")
  .fill("");
await page.getByRole("button", { name: /^All projects/ }).click();
await page.locator(".export-menu summary").click();
// All-project checkbox ticks all/none; the label keeps showing all.
const allProjects = page.getByRole("checkbox", {
  name: "All projects",
  exact: true,
});
const projectTicks = page.locator("label.project input");
assert.equal(await allProjects.isChecked(), false);
await allProjects.check();
assert.equal(await projectTicks.filter({ visible: true }).count(), 4);
assert.equal(await page.locator("label.project input:checked").count(), 4);
assert.equal(await allProjects.evaluate((node) => node.indeterminate), false);
await projectTicks.first().uncheck();
assert.equal(await allProjects.evaluate((node) => node.indeterminate), true);
await allProjects.check();
assert.equal(await page.locator("label.project input:checked").count(), 4);
await allProjects.uncheck();
assert.equal(await page.locator("main tbody tr").count(), 0);
assert.equal(await page.locator("label.project input:checked").count(), 0);
await projectTicks.first().check();
await page.getByRole("button", { name: /^All projects/ }).click();
assert.equal(await page.locator("label.project input:checked").count(), 0);
assert.equal(await page.locator("main .table-scroll").count(), 1);
for (const width of [1280, 1024]) {
  await page.setViewportSize({ width, height: 900 });
  await page.waitForTimeout(100);
  const layout = await page.evaluate(() => {
    const rail = document.querySelector("aside").getBoundingClientRect();
    const main = document.querySelector("main").getBoundingClientRect();
    const table = document
      .querySelector(".table-scroll")
      .getBoundingClientRect();
    return {
      rail: rail.width,
      main: main.width,
      table: table.width,
      right: main.right,
      document: document.documentElement.scrollWidth,
    };
  });
  assert.equal(layout.rail, 320);
  assert.ok(layout.main > 600);
  assert.ok(layout.table > 500);
  assert.ok(layout.right <= width);
  assert.ok(layout.document <= width);
  for (const node of await page.locator(".source-path").all()) {
    const box = await node.boundingBox();
    const style = await node.evaluate((el) => ({
      line: parseFloat(getComputedStyle(el).lineHeight),
      text: el.textContent,
      title: el.title,
    }));
    assert.ok(box.height <= style.line + 1);
    assert.ok(style.title.startsWith("/Volumes/"));
    assert.ok(style.text.startsWith("/Volumes/"));
    if (style.title.includes("/MXF/"))
      assert.ok(
        style.text.endsWith(
          style.title.slice(style.title.lastIndexOf("/MXF/")),
        ),
      );
  }
}
await page.setViewportSize({ width: 1440, height: 900 });
await projectTicks.first().check(); // Show the mixed state in both screenshots.
await page.waitForTimeout(100);
const railSize = await page.locator("aside").boundingBox();
const dark = await page.locator("aside").screenshot();
await page.getByRole("button", { name: "☀ Light", exact: true }).click();
const light = await page.locator("aside").screenshot();
const comparison = await browser.newPage({
  viewport: {
    width: Math.ceil(railSize.width * 2 + 32),
    height: Math.ceil(railSize.height + 48),
  },
});
await comparison.setContent(`<style>body{margin:0;padding:8px;display:flex;gap:16px;background:#e6eaf0;font:12px system-ui;color:#222}figure{margin:0}figcaption{height:32px;line-height:28px}img{display:block}</style>
<figure><figcaption>Dark</figcaption><img src="data:image/png;base64,${dark.toString("base64")}"></figure>
<figure><figcaption>Light</figcaption><img src="data:image/png;base64,${light.toString("base64")}"></figure>`);
await comparison
  .locator("img")
  .evaluateAll((images) => Promise.all(images.map((image) => image.decode())));
const output = new URL("../../../docs/design/", import.meta.url);
await mkdir(output, { recursive: true });
await comparison.screenshot({
  path: new URL("sources-rail-v2.png", output).pathname,
});
await comparison.close();
await page.getByRole("button", { name: "Add folder", exact: true }).click();
await page.waitForFunction(
  () => window.testSettings.custom_folders.length === 3,
);
assert.equal(
  await page.evaluate(
    () => window.calls.filter((c) => c.cmd === "start_scan").length,
  ),
  0,
);
// Uncheck all: Scan must disable. Re-check just one mounted and one custom source.
while (await page.locator(".source-row input:checked").count())
  await page.locator(".source-row input:checked").first().uncheck();
assert.equal(
  await page
    .getByText(
      "Nothing selected. Tick a folder on the left to see its media.",
      { exact: true },
    )
    .count(),
  1,
);
assert.equal(await page.locator("label.project").count(), 0);
assert.equal(
  await page.getByRole("button", { name: "Scan", exact: true }).isDisabled(),
  true,
);
await page.locator(".source-row input").nth(1).check();
await page.locator(".source-row input").last().check();
await page.getByRole("button", { name: "Scan", exact: true }).click();
const saved = await page.evaluate(
  () => window.calls.filter((c) => c.cmd === "save_settings").at(-1).args.value,
);
assert.deepEqual(saved.roots, [
  saved.mounted_sources[1].path,
  saved.custom_folders[2].path,
]);
assert.equal(
  await page.evaluate(
    () => window.calls.filter((c) => c.cmd === "start_scan").length,
  ),
  1,
);
assert.deepEqual(errors, []);
await browser.close();
console.log(
  "UI passed: zero discovery on idle/focus; disabled empty selection; scan selected paths; dark/light capture.",
);
