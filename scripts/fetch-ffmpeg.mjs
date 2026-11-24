#!/usr/bin/env node
// Release preparation never consults MOBBOSS_FFMPEG, PATH ffmpeg, or Homebrew ffmpeg.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  createReadStream,
  existsSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  copyFileSync,
  chmodSync,
  rmSync,
  renameSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateRawSync } from "node:zlib";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pins = JSON.parse(
  readFileSync(join(root, "scripts/ffmpeg-sources.json"), "utf8"),
);
const destination = join(root, "apps/desktop/src-tauri/binaries");
const cache = join(root, "target/ffmpeg-cache");
const recipe = createHash("sha256")
  .update(readFileSync(fileURLToPath(import.meta.url)))
  .update(JSON.stringify(pins))
  .digest("hex");
const option = process.argv.findIndex((s) => s === "--target");
const host =
  process.platform === "darwin"
    ? `${process.arch === "arm64" ? "aarch64" : "x86_64"}-apple-darwin`
    : process.platform === "win32"
      ? "x86_64-pc-windows-msvc"
      : "x86_64-unknown-linux-gnu";
const target =
  option >= 0
    ? process.argv[option + 1]
    : process.env.TAURI_ENV_TARGET_TRIPLE || host;
if (!target) throw new Error("Missing --target value");
if (!target.endsWith("apple-darwin") && !target.endsWith("windows-msvc")) {
  console.log(
    "Linux development: no ffmpeg is bundled; set MOBBOSS_FFMPEG to opt into previews.",
  );
  process.exit(0);
}
mkdirSync(destination, { recursive: true });
mkdirSync(cache, { recursive: true });
async function digest(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}
function run(program, args, options = {}) {
  return execFileSync(program, args, { stdio: "inherit", ...options });
}
async function download(pin, name) {
  const path = join(cache, name);
  if (existsSync(path) && (await digest(path)) === pin.sha256) return path;
  const temp = `${path}.download`;
  rmSync(temp, { force: true });
  run("curl", [
    "--fail",
    "--location",
    "--retry",
    "3",
    "--proto",
    "=https",
    "--tlsv1.2",
    "--output",
    temp,
    pin.url,
  ]);
  if ((await digest(temp)) !== pin.sha256) {
    rmSync(temp, { force: true });
    throw new Error(`Checksum mismatch for ${name}`);
  }
  renameSync(temp, path);
  return path;
}
function validate(binary, platform) {
  const output = (args) =>
    execFileSync(binary, ["-hide_banner", ...args], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      maxBuffer: 4 * 1024 * 1024,
    });
  const version = output(["-version"]),
    license = output(["-L"]);
  if (
    /--enable-(?:gpl|nonfree|libx264)(?:\s|$)/.test(version) ||
    !/Lesser General Public License/.test(license)
  )
    throw new Error("Refusing a non-LGPL ffmpeg");
  const decoders = output(["-decoders"]),
    encoders = output(["-encoders"]);
  for (const name of [
    "dnxhd",
    "pcm_s16le",
    "pcm_s24le",
    "h264",
    "aac",
    "pcm_s16le",
    ...(platform === "windows" ? ["vp8", "vp9", "opus"] : []),
  ])
    if (!new RegExp(`\\b${name}\\b`).test(decoders))
      throw new Error(`ffmpeg missing decoder ${name}`);
  for (const name of [
    "mjpeg",
    "aac",
    "pcm_s16le",
    ...(platform === "windows"
      ? ["h264_mf", "libvpx", "libvpx-vp9", "libopus"]
      : ["h264_videotoolbox"]),
  ])
    if (!new RegExp(`\\b${name}\\b`).test(encoders))
      throw new Error(`ffmpeg missing encoder ${name}`);
  for (const [flag, names] of [
    ["-demuxers", ["mxf", "mov"]],
    [
      "-muxers",
      platform === "windows"
        ? ["mp4", "image2", "webm", "null", "s16le"]
        : ["mp4", "image2", "s16le"],
    ],
    [
      "-filters",
      [
        "scale",
        "overlay",
        "lutrgb",
        "pan",
        "amerge",
        "adelay",
        "apad",
        "null",
        "anull",
        "aformat",
        "anullsrc",
        "buffer",
        "abuffer",
        "buffersink",
        "abuffersink",
      ],
    ],
  ]) {
    const list = output([flag]);
    for (const name of names)
      if (!new RegExp(`\\b${name}\\b`).test(list))
        throw new Error(`ffmpeg missing ${name}`);
  }
}
async function reused(output, pin, targetName) {
  const manifest = `${output}.json`;
  if (!existsSync(output) || !existsSync(manifest)) return false;
  try {
    const info = JSON.parse(readFileSync(manifest, "utf8"));
    return (
      info.source_sha256 === pin.sha256 &&
      info.recipe_sha256 === recipe &&
      info.target === targetName &&
      info.binary_sha256 === (await digest(output))
    );
  } catch {
    return false;
  }
}
async function record(output, pin, targetName) {
  const info = {
    recipe_sha256: recipe,
    version: pin.version,
    target: targetName,
    source: pin.source,
    source_sha256: pin.sha256,
    binary_sha256: await digest(output),
    bytes: readFileSync(output).length,
  };
  writeFileSync(`${output}.json`, JSON.stringify(info, null, 2) + "\n");
  console.log(
    `${targetName}: ${info.bytes.toLocaleString("en-US")} bytes (LGPL sidecar)`,
  );
}
// Read only the pinned ZIP entry. Never extract archive-controlled paths.
function zipFile(archive, suffix) {
  const bytes = readFileSync(archive);
  let end = bytes.length - 22;
  while (
    end >= Math.max(0, bytes.length - 65557) &&
    bytes.readUInt32LE(end) !== 0x06054b50
  )
    end--;
  if (end < 0 || bytes.readUInt32LE(end) !== 0x06054b50)
    throw new Error("Invalid ZIP directory");
  const count = bytes.readUInt16LE(end + 10);
  let offset = bytes.readUInt32LE(end + 16);
  for (let i = 0; i < count; i++) {
    if (bytes.readUInt32LE(offset) !== 0x02014b50)
      throw new Error("Invalid ZIP entry");
    const method = bytes.readUInt16LE(offset + 10),
      size = bytes.readUInt32LE(offset + 20),
      plain = bytes.readUInt32LE(offset + 24),
      nameLen = bytes.readUInt16LE(offset + 28),
      extra = bytes.readUInt16LE(offset + 30),
      comment = bytes.readUInt16LE(offset + 32),
      local = bytes.readUInt32LE(offset + 42);
    const name = bytes.toString("utf8", offset + 46, offset + 46 + nameLen);
    if (name.endsWith(suffix)) {
      if (plain > 250_000_000 || bytes.readUInt32LE(local) !== 0x04034b50)
        throw new Error("Invalid sidecar ZIP size/header");
      const start =
        local +
        30 +
        bytes.readUInt16LE(local + 26) +
        bytes.readUInt16LE(local + 28);
      const data = bytes.subarray(start, start + size),
        result =
          method === 0
            ? Buffer.from(data)
            : method === 8
              ? inflateRawSync(data, { maxOutputLength: 250_000_000 })
              : null;
      if (!result || result.length !== plain)
        throw new Error("Invalid sidecar ZIP compression");
      return result;
    }
    offset += 46 + nameLen + extra + comment;
  }
  throw new Error(`Pinned ZIP has no ${suffix}`);
}
async function windows() {
  if (target !== "x86_64-pc-windows-msvc")
    throw new Error(`Unsupported Windows target ${target}`);
  const pin = pins.windows,
    output = join(destination, `ffmpeg-${target}.exe`);
  if (!(await reused(output, pin, target))) {
    const archive = await download(pin, "ffmpeg-windows-lgpl.zip");
    writeFileSync(`${output}.new`, zipFile(archive, "/bin/ffmpeg.exe"));
    renameSync(`${output}.new`, output);
    writeFileSync(
      join(destination, "FFmpeg-Windows-LICENSE.txt"),
      zipFile(archive, "/LICENSE.txt"),
    );
  }
  if (process.platform === "win32") validate(output, "windows");
  else
    console.log(
      "Cross-prepared Windows executable; runtime validation must run on Windows.",
    );
  await record(output, pin, target);
}
async function macArch(arch) {
  const targetName = `${arch}-apple-darwin`,
    pin = pins.macos,
    output = join(destination, `ffmpeg-${targetName}`);
  if (await reused(output, pin, targetName)) {
    if ((arch === "aarch64") === (process.arch === "arm64"))
      validate(output, "macos");
    return output;
  }
  if (process.platform !== "darwin")
    throw new Error(
      "macOS ffmpeg source builds require Xcode command-line tools on macOS",
    );
  const archive = await download(pin, `ffmpeg-${pin.version}.tar.xz`);
  const source = join(cache, `ffmpeg-${pin.version}`);
  rmSync(source, { recursive: true, force: true });
  mkdirSync(source, { recursive: true });
  run("tar", ["-xf", archive, "-C", source, "--strip-components=1"]);
  const build = join(cache, `build-${pin.version}-${arch}`);
  rmSync(build, { recursive: true, force: true });
  mkdirSync(build, { recursive: true });
  const nativeArch = arch === "aarch64" ? "arm64" : "x86_64",
    minimum = arch === "aarch64" ? "11.0" : "10.15";
  const flags = [
    "--prefix=/ffmpeg",
    "--disable-autodetect",
    "--disable-everything",
    "--disable-doc",
    "--disable-debug",
    "--disable-network",
    "--disable-shared",
    "--enable-static",
    "--disable-x86asm",
    "--enable-cross-compile",
    "--target-os=darwin",
    `--arch=${arch}`,
    "--cc=clang",
    `--extra-cflags=-arch ${nativeArch} -mmacosx-version-min=${minimum}`,
    `--extra-ldflags=-arch ${nativeArch} -mmacosx-version-min=${minimum}`,
    "--enable-ffmpeg",
    "--disable-ffprobe",
    "--disable-ffplay",
    "--enable-protocol=file,pipe",
    "--enable-demuxer=mxf,mov",
    "--enable-decoder=dnxhd,rawvideo,h264,aac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_s16be,pcm_s24be",
    "--enable-parser=dnxhd,dnxuc,h264,aac",
    "--enable-encoder=mjpeg,h264_videotoolbox,aac,pcm_s16le",
    "--enable-videotoolbox",
    "--enable-muxer=image2,mp4,s16le",
    "--enable-filter=scale,format,split,lutrgb,overlay,aresample,pan,amerge,apad,atrim,asetpts,setpts,adelay,null,anull,aformat,anullsrc,nullsink,anullsink,trim,buffer,abuffer,buffersink,abuffersink",
    "--enable-swscale",
    "--enable-swresample",
  ];
  // Relative configure path keeps developer folder names out of the embedded buildconf.
  run(`../ffmpeg-${pin.version}/configure`, flags, { cwd: build });
  run("make", ["-j", process.env.MOBBOSS_FFMPEG_JOBS || "4"], { cwd: build });
  copyFileSync(join(build, "ffmpeg"), output);
  chmodSync(output, 0o755);
  if ((arch === "aarch64") === (process.arch === "arm64"))
    validate(output, "macos");
  else
    console.log(
      `${targetName}: cross-built slice; native runtime validation required`,
    );
  await record(output, pin, targetName);
  return output;
}
if (target.endsWith("windows-msvc")) await windows();
else if (target === "universal-apple-darwin") {
  const a = await macArch("aarch64"),
    b = await macArch("x86_64"),
    output = join(destination, "ffmpeg-universal-apple-darwin");
  run("lipo", ["-create", a, b, "-output", output]);
  chmodSync(output, 0o755);
  validate(output, "macos");
  await record(output, pins.macos, target);
} else if (["aarch64-apple-darwin", "x86_64-apple-darwin"].includes(target))
  await macArch(target.split("-")[0]);
else throw new Error(`Unsupported target ${target}`);
