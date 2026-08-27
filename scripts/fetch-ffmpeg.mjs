// Download a static ffmpeg build for the current target into
// src-tauri/binaries/ffmpeg-<triple>[.exe], where Tauri's externalBin
// ("binaries/ffmpeg") picks it up and bundles it next to the app binary.
// Idempotent: does nothing when the sidecar is already present.
//
// Sources (static, no system dependencies):
//   macOS   : ffmpeg.martin-riedl.de (release channel, snapshot fallback)
//   Windows : BtbN FFmpeg-Builds (LGPL variant)
//   Linux   : BtbN FFmpeg-Builds (GitHub CDN), johnvansickle fallback
import { execSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function hostTriple() {
  if (process.env.TAURI_ENV_TARGET_TRIPLE) return process.env.TAURI_ENV_TARGET_TRIPLE;
  const out = execSync("rustc -vV", { encoding: "utf8" });
  return /host: (\S+)/.exec(out)[1];
}

const SOURCES = {
  "aarch64-apple-darwin": [
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip",
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/snapshot/ffmpeg.zip",
  ],
  "x86_64-apple-darwin": [
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/amd64/release/ffmpeg.zip",
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/amd64/snapshot/ffmpeg.zip",
  ],
  // BtbN's "latest" release does not always carry the master-latest
  // aliases (they appear late in their autobuild cycle), so a PINNED
  // dated autobuild — releases are permanent — backs every alias up.
  "x86_64-pc-windows-msvc": [
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-lgpl.zip",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-27-16-45/ffmpeg-n8.1.2-47-g156bb4d299-win64-lgpl-8.1.zip",
  ],
  "aarch64-pc-windows-msvc": [
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-winarm64-lgpl.zip",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-27-16-45/ffmpeg-n8.1.2-47-g156bb4d299-winarm64-lgpl-8.1.zip",
  ],
  // BtbN first: GitHub's CDN is dependable, and johnvansickle.com
  // occasionally serves an HTML rate-limit page with a 200 status.
  "x86_64-unknown-linux-gnu": [
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-linux64-lgpl.tar.xz",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-27-16-45/ffmpeg-n8.1.2-47-g156bb4d299-linux64-lgpl-8.1.tar.xz",
    "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz",
  ],
  "aarch64-unknown-linux-gnu": [
    "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-linuxarm64-lgpl.tar.xz",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-27-16-45/ffmpeg-n8.1.2-47-g156bb4d299-linuxarm64-lgpl-8.1.tar.xz",
    "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-arm64-static.tar.xz",
  ],
};

function findBinary(dir, name) {
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) {
      const found = findBinary(p, name);
      if (found) return found;
    } else if (entry === name) {
      return p;
    }
  }
  return null;
}

const triple = hostTriple();
const isWindows = triple.includes("windows");
const binName = isWindows ? "ffmpeg.exe" : "ffmpeg";
const dest = join(root, "src-tauri", "binaries", `ffmpeg-${triple}${isWindows ? ".exe" : ""}`);

if (existsSync(dest)) {
  console.log(`ffmpeg sidecar already present: ${dest}`);
  process.exit(0);
}

const urls = SOURCES[triple];
if (!urls) {
  // Unknown target: the app still falls back to a system-installed ffmpeg.
  console.warn(`No static ffmpeg source known for ${triple} — skipping sidecar.`);
  process.exit(0);
}

mkdirSync(dirname(dest), { recursive: true });
const work = mkdtempSync(join(tmpdir(), "still-ffmpeg-"));
let ok = false;
for (const url of urls) {
  try {
    console.log(`Downloading ffmpeg for ${triple}\n  ${url}`);
    const archive = join(work, url.endsWith(".tar.xz") ? "ffmpeg.tar.xz" : "ffmpeg.zip");
    execSync(`curl -fsSL --retry 3 -o "${archive}" "${url}"`, { stdio: "inherit" });
    // bsdtar/GNU tar both read zip and tar.xz on the GitHub runners and macOS.
    execSync(`tar -xf "${archive}" -C "${work}"`, { stdio: "inherit" });
    const found = findBinary(work, binName);
    if (!found) throw new Error(`no ${binName} inside the archive`);
    copyFileSync(found, dest);
    if (!isWindows) chmodSync(dest, 0o755);
    console.log(`ffmpeg sidecar ready: ${dest}`);
    ok = true;
    break;
  } catch (e) {
    console.warn(`Source failed (${e.message ?? e}), trying next…`);
  }
}
rmSync(work, { recursive: true, force: true });
if (!ok) {
  console.error("Could not fetch a static ffmpeg for this target.");
  process.exit(1);
}
