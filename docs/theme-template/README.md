# PIC community theme template

A starting point for building your own PIC appearance theme. Copy the folder,
rename it, recolor the palette — done.

## Install in three steps

1. **Copy** this folder into your PIC installation's theme folder and give it
   your theme's id (lowercase, no spaces — this is the stable name PIC saves):

   ```
   css/themes/my-theme/          <- the copy, folder renamed
   ```

   `css/themes/` sits next to the PIC executable (same place as the bundled
   `css/themes/standard/`, `css/themes/teal/`, …).

2. **Rename** `theme.css` stays `theme.css` — instead, edit the metadata at
   the top of the file:

   ```css
   /* picasa-theme
      version: 1
      name: My Theme
      dark: false
      mode: overlay
      window-controls: native
   */
   ```

3. **Open PIC → Settings → Themes.** Your theme is already in the Appearance
   list — no restart needed. The list rescans the folder every time the page
   opens.

## Rules of the road

- **The metadata block must be the first comment in the file.** Only the
  first `/* … */` block is parsed; a marker in a later block is ignored.
- **Folder name = theme id.** It is what PIC stores in its settings, so never
  rename a folder that people already use — bump the `name:` instead.
- **`theme.css` required.** Extra files (previews, notes) are allowed; a
  second `*.css` file without a `theme.css` makes the folder invisible.
- **Keep valid GTK CSS.** PIC loads themes through GTK's CSS parser;
  malformed rules and unsupported properties (width, height, transform,
  filter values other than blur(), flex/grid, …) are dropped with a parser
  warning, not a crash — but the effect simply won't appear.
- **Neutralize the search entry's background** (the template already does):
  the inner `GtkSearchEntry` paints its system view background over your
  wrapper, causing light-on-light or dark-on-dark text.
- **Unstyled surfaces fall through to the dark base theme.** Light themes
  should keep the Surfaces section; dark themes should recolor it.
- **Badges and favourites** ship with conventional colors (amber offline,
  red favourites). Recolor them if your palette disagrees.

## Metadata reference

| Key | Values | Meaning |
|-----|--------|------------------|
| `version` | `1` | Theme contract version of this file. Current is 1. |
| `name` | any text | Label in Settings → Themes. Defaults to the folder name. |
| `dark` | `true` / `false` | Dark themes prefer the dark color scheme and the dark lightbox backdrop; the header icon shows a moon. |
| `mode` | `overlay` / `base` | Leave `overlay`. `base` themes replace the built-in dark foundation — advanced use only. |
| `window-controls` | `native` / `traffic-light` | `traffic-light` swaps the native GTK minimize/maximize/close buttons for PIC's gel-style traffic lights while your theme is active (style them via the section 10 block in `theme.css`). |

Unknown keys are ignored, so adding `version: 1` today is forward-compatible.

## Testing your theme

- Toggle between your theme and **Standard GTK4** to spot unstyled surfaces
  (they will look dark).
- Check the three selection states: a selected sidebar row (Favourites), the
  scroll-follow folder marker (browse into a folder), and multi-selected
  photo tiles.
- Check the segmented toggles: Edit → Crop (Free / Landscape) and the Collage
  panel (Mosaic / Smart / Grid).
- Invalid metadata values are dropped with a warning on the console; the
  theme still loads with defaults.

See also `docs/THEMES.md` for the full discovery and layering model.
