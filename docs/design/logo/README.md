# The Boss

Cinema Samurai red `#EC2726`, ink `#141414`. The supplied 24-unit fedora geometry is preserved. Its brim spans 72% of the tile body; the glyph's vertical bounds are centred with a 2% downward optical adjustment.

- `mobboss-icon.svg` / `mobboss-icon.png`: 1024 canvas, transparent 100 px margins, 824 px body, 185 px corner radius. The macOS ICNS uses this grid.
- `mobboss-mark.svg`: reusable glyph with `currentColor` fill and red hatband.
- `mobboss-tile.svg`: full-bleed Windows/Linux tile.
- `mobboss-tile-small.svg`: band-free tile for 32 px and below, also used as the favicon.
- `mobboss-preview.png`: actual-size 16, 32, 64, 128 and 512 px tile renders.

The header uses a 44 px red tile with a 31.5 px visible brim, omitting the band at that glyph size.

To regenerate, install the desktop's locked npm dependencies and Pillow, then run from the repository root:

```sh
python3 docs/design/logo/generate.py
```

The script invokes `tauri icon` for both platform variants and the small-size source. It preserves Tauri's macOS container and replaces its 16/32 px representations with band-free versions; similarly it replaces the small ICO frames. Windows ICO frames have no macOS margin. Only desktop assets are copied from Tauri's generated output.

Generated files in `apps/desktop/src-tauri/icons`:

| File | Pixels |
| --- | --- |
| 32x32.png | 32 × 32 |
| 64x64.png | 64 × 64 |
| 128x128.png | 128 × 128 |
| 128x128@2x.png | 256 × 256 |
| icon.png | 512 × 512 |
| Square30x30Logo.png | 30 × 30 |
| Square44x44Logo.png | 44 × 44 |
| Square71x71Logo.png | 71 × 71 |
| Square89x89Logo.png | 89 × 89 |
| Square107x107Logo.png | 107 × 107 |
| Square142x142Logo.png | 142 × 142 |
| Square150x150Logo.png | 150 × 150 |
| Square284x284Logo.png | 284 × 284 |
| Square310x310Logo.png | 310 × 310 |
| StoreLogo.png | 50 × 50 |
| icon.ico | 16, 24, 32, 48, 64, 256 |
| icon.icns | 16, 32, 64, 128, 256, 512, 1024 (including Retina representations) |
