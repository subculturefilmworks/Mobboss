"""Regenerate desktop icons: python3 docs/design/logo/generate.py (requires Pillow and npm ci)."""
from pathlib import Path
import io
import shutil
import struct
import subprocess
import tempfile
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[3]
LOGO = Path(__file__).resolve().parent
ICONS = ROOT / 'apps/desktop/src-tauri/icons'
CLI = ROOT / 'apps/desktop/node_modules/.bin/tauri'


def generate(source, output, sizes=()):
    args = [str(CLI), 'icon', str(source), '-o', str(output)]
    for size in sizes:
        args.extend(['-p', str(size)])
    subprocess.run(args, check=True)


def icns_chunks(data):
    pos, chunks = 8, {}
    while pos < len(data):
        size = struct.unpack('>I', data[pos + 4:pos + 8])[0]
        chunks[data[pos:pos + 4]] = data[pos:pos + size]
        pos += size
    return chunks


with tempfile.TemporaryDirectory(prefix='mobboss-icons-') as tmp:
    tmp = Path(tmp)
    mac, tile, small, preview, master = [tmp / name for name in ['mac', 'tile', 'small', 'preview', 'master']]
    generate(LOGO / 'mobboss-icon.svg', mac)
    generate(LOGO / 'mobboss-tile.svg', tile)
    generate(LOGO / 'mobboss-tile-small.svg', small, [16, 24, 30, 32])
    generate(LOGO / 'mobboss-tile.svg', preview, [64, 128, 512])
    generate(LOGO / 'mobboss-icon.svg', master, [1024])
    for file in tile.iterdir():
        if file.is_file():
            shutil.copy(file, ICONS / file.name)
    shutil.copy(master / '1024x1024.png', LOGO / 'mobboss-icon.png')
    shutil.copy(small / '32x32.png', ICONS / '32x32.png')
    shutil.copy(small / '30x30.png', ICONS / 'Square30x30Logo.png')
    shutil.copy(LOGO / 'mobboss-tile-small.svg', ROOT / 'apps/desktop/public/icon.svg')

    # Preserve Tauri's larger ICO frames; small frames omit the band.
    ico = Image.open(tile / 'icon.ico')
    frames = []
    for width, height in sorted(ico.ico.sizes()):
        im = Image.open(small / f'{width}x{width}.png') if width <= 32 else ico.ico.getimage((width, height))
        buf = io.BytesIO()
        im.save(buf, format='PNG')
        frames.append((width, height, buf.getvalue()))
    offset = 6 + 16 * len(frames)
    data = bytearray(struct.pack('<HHH', 0, 1, len(frames)))
    for width, height, frame in frames:
        data.extend(struct.pack('<BBBBHHII', width % 256, height % 256, 0, 0, 1, 32, len(frame), offset))
        offset += len(frame)
    for _, _, frame in frames:
        data.extend(frame)
    (ICONS / 'icon.ico').write_bytes(data)

    # Mac 16/32 representations (including legacy RGB/mask chunks) omit the band.
    svg = (LOGO / 'mobboss-icon.svg').read_text()
    start = svg.index('<path d="M6.6')
    end = svg.index('/>', start) + 2
    (tmp / 'mac-small.svg').write_text(svg[:start] + svg[end:])
    generate(tmp / 'mac-small.svg', tmp / 'mac-small')
    chunks = icns_chunks((mac / 'icon.icns').read_bytes())
    bandless = icns_chunks((tmp / 'mac-small/icon.icns').read_bytes())
    for key in [b'is32', b's8mk', b'il32', b'l8mk', b'ic11', b'icp4', b'icp5']:
        if key in chunks:
            chunks[key] = bandless[key]
    data = b''.join(chunks.values())
    (ICONS / 'icon.icns').write_bytes(b'icns' + struct.pack('>I', len(data) + 8) + data)

    canvas = Image.new('RGB', (880, 580), '#eeeeee')
    draw, x = ImageDraw.Draw(canvas), 24
    for size in [16, 32, 64, 128, 512]:
        im = Image.open((small if size <= 32 else preview) / f'{size}x{size}.png')
        canvas.paste(im, (x, 36 + (512 - size) // 2), im)
        draw.text((x, 554), f'{size} px', fill='#141414')
        x += size + 20
    canvas.save(LOGO / 'mobboss-preview.png')
