# veil icon

The icon of veil. `veil.svg` is the master; the PNGs are exports of it and everything under `variants/` is another style.

## Files

| Path | What |
|---|---|
| `veil.svg` | The main icon, the one the README shows. A copy of `variants/scene/veil-free-light.svg`. 256x256 `viewBox`, transparent background, no embedded raster images, no text, no external links. |
| `veil-512.png`, `veil-256.png`, `veil-128.png` | Exports of `veil.svg`. |
| `variants/scene/` | Style "scene" (the master is one of these). |
| `variants/plain/` | Style "plain". |
| `variants/flat/` | Style "flat". |
| `variants/tray/` | Style "tray": a 24 px mark, light and dark only. |

In each style, `free` has a transparent background and `tile` sits on a rounded square; `light` and `dark` are for light and dark backgrounds.

## How the PNGs are made

```bash
for s in 512 256 128; do
  inkscape veil.svg --export-type=png --export-filename=veil-$s.png -w $s -h $s
done
```

Nothing in the build uses these files: the package and the release tarball do not install them, and the daemon has no window or launcher entry.

## Terms

The icon is covered by the licence of the repository, and its name and logo are reserved in `ADDITIONAL-PERMISSIONS.md`.
