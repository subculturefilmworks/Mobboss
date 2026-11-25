import { execFileSync } from "node:child_process";
import { readFileSync, appendFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export function buildInfo({
  env = process.env,
  git = () =>
    execFileSync("git", ["rev-parse", "--short", "HEAD"], {
      stdio: ["ignore", "pipe", "ignore"],
    })
      .toString()
      .trim(),
  now = () => new Date(),
} = {}) {
  const cargo = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8");
  const base = cargo.match(
    /\[workspace.package\][\s\S]*?version = "([^"]+)"/,
  )[1];
  const tag =
    env.GITHUB_REF_TYPE === "tag"
      ? env.GITHUB_REF_NAME?.replace(/^v/, "")
      : undefined;
  let commit;
  try {
    commit = git();
  } catch {
    /* Source archives have no git metadata. */
  }
  return {
    version: tag || `${base}-dev`,
    date:
      env.MOBBOSS_BUILD_DATE ||
      now().toISOString().slice(0, 16).replace("T", " ") + " UTC",
    commit: commit || "unknown",
  };
}

// CI shares one timestamp with Vite and Cargo and keeps bundle versions numeric.
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const info = buildInfo();
  const numeric = info.version.match(/^(\d+\.\d+\.\d+)(?:-|$)/)?.[1];
  if (!numeric) throw new Error(`Invalid release version: ${info.version}`);
  const configPath = new URL(
    "../apps/desktop/src-tauri/tauri.conf.json",
    import.meta.url,
  );
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  config.version = numeric;
  writeFileSync(configPath, JSON.stringify(config, null, 2) + "\n");
  if (process.env.GITHUB_ENV)
    appendFileSync(process.env.GITHUB_ENV, `MOBBOSS_BUILD_DATE=${info.date}\n`);
}
