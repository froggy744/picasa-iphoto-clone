# Appearance Themes

Themes are plain folders on disk — nothing is hardcoded in the app. The
appearance picker (Settings → Themes, also reachable from the header-bar
sun/moon button) rescans the theme folder every time it is shown, so a new
theme appears without restarting.

## Location

```
<app folder>/css/themes/<theme-id>/theme.css
```

The app resolves `css/themes` relative to its working directory, exactly like
the `images/` runtime folder. The shipped themes live in the repository at
`css/themes/` and build scripts bundle them next to the binary.

## Theme folder

| File | Required | Purpose |
|------|----------|---------|
| `theme.css` | preferred | The stylesheet. |
| any single `*.css` | fallback | Used only when `theme.css` is absent and it is the **only** stylesheet in the folder. |
| anything else | optional | Extra files (images, notes, backups) are ignored. |

A folder with several stylesheets and no `theme.css` is rejected on purpose,
so a stray backup copy cannot break discovery.

## Metadata header

The display name and behaviour flags come from a comment at the very top of
the stylesheet:

```css
/* picasa-theme
   version: 1
   name: Deep Ocean
   dark: false
   mode: overlay
*/
```

| Key | Values | Default | Meaning |
|-----|--------|---------|---------|
| `version` | `1` | — | Theme contract version of this file. Unknown keys are ignored; current contract is 1. |
| `name` | any text | folder name, humanized | Label shown in the picker. |
| `dark` | `true`/`false` (also `yes`/`on`/`1`) | `false` | Dark themes force the app-wide color scheme to prefer dark and use the dark lightbox backdrop; the header icon switches to a moon. |
| `mode` | `overlay` or `base` | `overlay` | `base` marks the one theme that is always loaded beneath every overlay theme (the shipped iPhoto Dark). |

The header only counts as metadata when its first line contains the marker
`picasa-theme`; ordinary licence comments are ignored.

## Identity and persistence

- The **theme id** is the folder name — it is what gets saved to the settings
  database. Renaming a folder makes the saved selection fall back to
  `standard`, then to the first theme found.
- The **display name** is free to change at any time via the metadata header.
- If `css/themes` is missing entirely the app runs on the stock GTK
  appearance.

## Layering

Lowest to highest priority on the GDK display:

1. `css::install_foundation` — embedded base + platform CSS.
2. The `mode: base` theme (shipped: `iphone`).
3. The selected overlay theme.

When the base theme itself is selected, no overlay provider is added.

## Shipped themes

Folder ids match the values older releases saved, so existing installs keep
their selection: `standard` (Standard GTK4), `iphone` (iPhoto Dark, base),
`teal` (Teal), `blue` (Deep Ocean), `glass` (Apple Glass), `superman`
(Superman).

## Authoring tips

- **Start from the community template: `docs/theme-template/`** — a complete,
  commented theme with every selector the app exercises (surfaces, photo
  tiles, sidebar selected + scroll-follow marker, segmented toggles, info
  bar, context menu). Copy it to `css/themes/<your-id>/` and recolor the
  palette block. See its README for the three-step install.
- `css/themes/standard/theme.css` is the shortest real example: it resets the
  dark base back to the stock libadwaita look.
- `css/themes/blue/theme.css` shows a fully self-contained palette.
- Always style the scroll-follow folder marker
  (`.navigation-sidebar row.sidebar-scroll-location`) and the segmented
  toggles (`.crop-aspect-button:checked`, `.crop-orientation-row button:checked`,
  `.collage-layout-tile:checked`, `.collage-tabs > button:checked`) — they are
  separate mechanisms from `row:selected` and otherwise fall through to the
  built-in base theme's styling.
- Themes only style widgets; layout and behaviour live in the app. After
  editing a theme, reopen Settings → Themes (or toggle the theme) to reload
  the stylesheet.
