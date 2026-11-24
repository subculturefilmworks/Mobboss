# Third-party notices

The PMR parsing algorithm in `crates/mobboss-core/src/pmr.rs` is ported from DJFio/MDV's `snippets/pmrtool.cpp`:
https://github.com/DJFio/MDV/blob/master/snippets/pmrtool.cpp

Copyright © 2020 DJFio.

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## FFmpeg sidecar

This software uses FFmpeg (https://ffmpeg.org/), distributed as a separate,
replaceable executable. Mob Boss does not link against its libraries. FFmpeg
is copyright the FFmpeg developers and is provided without warranty.

- **macOS:** FFmpeg **8.1.3**, LGPL 2.1 or later. Exact source archive:
  https://ffmpeg.org/releases/ffmpeg-8.1.3.tar.xz
  (`7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3`).
  Source tree: https://github.com/FFmpeg/FFmpeg/tree/n8.1.3.
  The unmodified source is built using the complete configure flags in
  `scripts/fetch-ffmpeg.mjs`; no GPL/nonfree options or external codec libraries
  are enabled. Each architecture is built separately and universal apps use lipo.
- **Windows x64:** FFmpeg **n8.1.3-41-ga8029b6528**, LGPL 3 or later,
  BtbN/FFmpeg-Builds `autobuild-2026-11-24-12-41`, static LGPL variant.
  Exact binary archive:
  https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-24-12-41/ffmpeg-n8.1.3-41-ga8029b6528-win64-lgpl-8.1.zip
  (`2201df4d6cbdcbbda9e612645605735821bb4cd0b120a9683f970c4c4e2de8e4`).
  Exact FFmpeg source: https://github.com/FFmpeg/FFmpeg/tree/a8029b6528.
  Build recipes, dependency sources and versions:
  https://github.com/FFmpeg-Builds/FFmpeg-Builds/tree/b9ebe81.
  The upstream license is preserved with the portable ZIP. No `--enable-gpl`,
  `--enable-nonfree` or `--enable-libx264` build is accepted.

FFmpeg is free software under the GNU Lesser General Public License: recipients
may copy, modify and redistribute it under that license. Copies of LGPL 2.1,
LGPL 3 and the GPL 3 text incorporated by LGPL 3 are included in `docs/licenses`
and distributed with the app. Those license texts govern FFmpeg; the Mob Boss
application retains its own MIT/Apache-2.0 license. Replacing the sidecar with a
compatible executable is supported. Mac source rebuilds use
`node scripts/fetch-ffmpeg.mjs --target universal-apple-darwin`; Windows rebuilds
follow the pinned upstream build recipes. Keep the notices with redistributed
binaries. A development `MOBBOSS_FFMPEG` override is never used by release-fetch
scripts, and the system GPL build is never bundled.
