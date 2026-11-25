import { chromium } from "playwright";
import assert from "node:assert/strict";
const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE,
});
try {
  const page = await browser.newPage();
  await page.goto(process.env.UI_URL || "http://127.0.0.1:1420");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  assert.match(
    await page.locator(".build-version").innerText(),
    /^v\S+ · \d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC · (?:[0-9a-f]+|unknown)$/,
  );
  for (const width of [1280, 1024]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(async () => {
      const { default: React } =
        await import("/node_modules/.vite/deps/react.js");
      const { default: ReactDOM } =
        await import("/node_modules/.vite/deps/react-dom_client.js");
      const { default: Player } = await import("/src/Player.tsx");
      document.getElementById("root").style.display = "none";
      let host = document.getElementById("player-test");
      if (!host) {
        host = document.createElement("div");
        host.id = "player-test";
        document.body.append(host);
      }
      const root = ReactDOM.createRoot(host);
      const metadata = {
        project: "Paper Harbor".repeat(20),
        codec: "DNxHD",
        track_kind: "video",
        duration: 864000,
        edit_rate: { numerator: 24, denominator: 1 },
        timecode_fps: 24,
      };
      const clip = {
        id: "synthetic-player",
        name: "Moon".repeat(50),
        tracks: "V1".repeat(100),
        metadata,
        files: [{ path: "/Invented-Island/moon.mxf", metadata }],
      };
      window.openTestPlayer = () =>
        root.render(
          React.createElement(Player, {
            clip,
            onClose: () => root.render(null),
          }),
        );
      window.openTestPlayer();
    });
    const modal = page.getByRole("dialog", {
      name: "Moon".repeat(50),
      exact: true,
    });
    const close = page.getByRole("button", {
      name: "Close player",
      exact: true,
    });
    await close.waitFor();
    assert.equal(
      await page.locator("#player-title").getAttribute("title"),
      "Moon".repeat(50),
    );
    const m = await modal.boundingBox();
    const b = await close.boundingBox();
    assert.ok(
      b.x >= m.x &&
        b.y >= m.y &&
        b.x + b.width <= m.x + m.width &&
        b.y + b.height <= m.y + m.height,
    );
    assert.equal(
      await modal.evaluate((el) => el.scrollWidth <= el.clientWidth),
      true,
    );
    assert.equal(
      await page
        .locator(".player-title")
        .evaluate((el) => el.scrollWidth <= el.clientWidth),
      true,
    );
    await close.click();
    await modal.waitFor({ state: "detached" });
    await page.evaluate(() => window.openTestPlayer());
    await close.waitFor();
    await page.keyboard.press("Escape");
    await modal.waitFor({ state: "detached" });
    await page.evaluate(() => document.getElementById("player-test").remove());
  }
  console.log(
    "Player passed: 200-character title, long metadata, visible clickable close, no horizontal overflow and Escape at 1280/1024; Settings build identity.",
  );
} finally {
  await browser.close();
}
