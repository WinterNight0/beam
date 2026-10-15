# beam-gui assets

Everything the window draws text and icons with, bundled into `beam-gui` with
`include_bytes!` so it looks the same on Windows and Linux and nothing is
fetched at run time (ADR-0051). Licence texts are in `LICENSES/`.

| File | What | Version | Source | Licence |
|---|---|---|---|---|
| `fonts/Inter-Regular.ttf`, `Inter-SemiBold.ttf`, `Inter-Bold.ttf` | interface typeface | 4.1 | `extras/ttf/` in `Inter-4.1.zip`, github.com/rsms/inter releases | SIL OFL 1.1 |
| `fonts/JetBrainsMono-Regular.ttf` | monospace: fingerprints, invites, paths | 2.304 | `fonts/ttf/` in `JetBrainsMono-2.304.zip`, github.com/JetBrains/JetBrainsMono releases | SIL OFL 1.1 |
| `fonts/FluentSystemIcons-beam.ttf` | icons | `main`, 2026-10-14 | `fonts/FluentSystemIcons-Regular.ttf`, github.com/microsoft/fluentui-system-icons, **cut down** to the glyphs in `src/icons.rs` | MIT |

The fonts are unmodified, except the icon font, which is subset. Fluent UI
System Icons is MIT-licensed and reserves no font name, so the subset keeps
its family name, `FluentSystemIcons-Regular`.

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
