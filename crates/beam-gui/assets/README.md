# beam-gui assets

Everything the window draws text, icons and logos with, bundled into
`beam-gui` with `include_bytes!` so it looks the same on Windows and Linux and
nothing is fetched at run time (ADR-0051, ADR-0054). Licence texts for the
fonts are in `LICENSES/`.

| File | What | Version | Source | Licence |
|---|---|---|---|---|
| `fonts/Inter-Regular.ttf`, `Inter-SemiBold.ttf`, `Inter-Bold.ttf` | interface typeface | 4.1 | `extras/ttf/` in `Inter-4.1.zip`, github.com/rsms/inter releases | SIL OFL 1.1 |
| `fonts/JetBrainsMono-Regular.ttf` | monospace: fingerprints, invites, paths | 2.304 | `fonts/ttf/` in `JetBrainsMono-2.304.zip`, github.com/JetBrains/JetBrainsMono releases | SIL OFL 1.1 |
| `fonts/FluentSystemIcons-beam.ttf` | icons | `main`, 2026-10-14 | `fonts/FluentSystemIcons-Regular.ttf`, github.com/microsoft/fluentui-system-icons, **cut down** to the glyphs in `src/icons.rs` | MIT |

The fonts are unmodified, except the icon font, which is subset. Fluent UI
System Icons is MIT-licensed and reserves no font name, so the subset keeps
its family name, `FluentSystemIcons-Regular`.

## Logos

| File | What | Source |
|---|---|---|
| `images/logo-b.png` | beam's mark (the bolt in its circle): top of the rail, welcome card | the project's own artwork, `Logo_B`, cropped to the mark and scaled to 256 × 256 |
| `images/beam.png` | the full logo (the bolt on a folder): window and taskbar icon | the project's own artwork, `BEAM`, cropped and scaled to 256 × 256 |

The repository's README shows the full logo from `assets/beam-logo.png` at the
root (512 × 512). The logos must stay square PNGs:
`images::tests::the_bundled_logos_decode` checks both. To change one, replace
the file with a square PNG (256 × 256 is plenty for a 40 px mark at 200 %
zoom on a 2× screen).

## Program icons

| File | What |
|---|---|
| `images/beam-gui.ico` | `beam-gui.exe`'s icon in Explorer, the Start menu and the taskbar: the full logo at 16–256 px, built in by `build.rs` through `images/beam-gui.rc` (Windows only, ADR-0055) |
| `../../beam/assets/beam.ico` | the same for `beam.exe`: the mark |
| `packaging/linux/` (repository root) | the Linux menu entries and the same icons as PNGs (48, 128, 256 px), installed by `scripts/linux/install.sh`; on Windows the setup wizard (`packaging/windows/beam.iss`) uses the `.ico` files |

The `.ico` files are the project's own artwork. To change an icon, replace the
`.ico` (keep its sizes: 16, 32, 48, 64, 128, 256) and remake the Linux PNGs
from it.

## Changing the icons

The icon font holds only the glyphs beam uses (32; the full font is 2.8 MB,
the subset under 7 KB). To add one:

1. Find its name in `FluentSystemIcons-Regular.json` from the same
   repository (`ic_fluent_<name>_20_regular`) and note its codepoint.
2. Add it to `src/icons.rs`.
3. Rebuild the subset from the full font with fontTools (a development tool,
   not a dependency of beam), listing **every** codepoint in `icons.rs`:

   ```
   pip install fonttools
   pyftsubset FluentSystemIcons-Regular.ttf --layout-features='' \
       --unicodes=U+F5A8,U+F18D,... --output-file=FluentSystemIcons-beam.ttf
   ```

`icons::tests::every_icon_is_in_the_bundled_font` fails if an icon in
`icons.rs` is missing from the font.
