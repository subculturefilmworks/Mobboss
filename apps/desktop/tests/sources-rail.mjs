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
        return [
          "Moon Orchard",
          "Paper Harbor",
          "Amber Transit",
          "Winter Signal",
        ].map((project, i) => ({
          path: `/Volumes/Story-Island/Avid MediaFiles/MXF/1/clip${i}.mxf`,
          folder: "/Volumes/Story-Island/Avid MediaFiles/MXF/1",
          size: 1234567890 * (i + 1),
          modified_ns: "0",
          metadata: {
            project,
            clip_name: "Scene " + (i + 1),
            track_kind: "video",
          },
        }));
      if (cmd === "preview_status")
        return {
          available: false,
          message: "Previews unavailable",
          thumbnails: {},
        };
      if (cmd === "discover_sources")
        return {
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
      if (cmd === "pick_path") return ["/Volumes/Added-Island/Media/MXF/1"];
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
// Discovery is explicit, keeps existing checks, and never scans.
await page
  .getByRole("button", { name: "Search for databases", exact: true })
  .click();
await page.waitForFunction(
  () => window.testSettings.mounted_sources.length === 5,
);
const merged = await page.evaluate(() => window.testSettings.mounted_sources);
assert.equal(merged[1].checked, false);
assert.equal(merged[1].offline, true);
assert.equal(merged[4].checked, true);
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
  await page.getByRole("button", { name: "Scan", exact: true }).isDisabled(),
  true,
);
await page.locator(".source-row input").nth(1).check();
await page.locator(".source-row input").nth(5).check();
await page.getByRole("button", { name: "Scan", exact: true }).click();
const saved = await page.evaluate(
  () => window.calls.filter((c) => c.cmd === "save_settings").at(-1).args.value,
);
assert.deepEqual(saved.roots, [
  saved.mounted_sources[1].path,
  saved.custom_folders[0].path,
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
