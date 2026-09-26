# Gallery v2 architecture

Branch: `architecture/gallery-v2`

Base: `rc-bugfixes`

## Goal

Keep the deployment-ready RC application and replace only the Folder gallery layout behavior that made zoom structurally expensive.

## Architecture rule

Zoom must never rebuild, splice, reorder, or recreate photo membership.

The gallery keeps one direct photo model and zoom only changes presentation geometry:

- tile width / height
- GridView column count
- layout invalidation

The legacy Folder ListView represented each display row as a model item containing up to the current number of columns. A 6→5 column zoom therefore changed the model structure and required rebuilding Folder rows.

Gallery v2 routes Folder mode through the existing direct photo `GtkGridView`, where one model item remains one photo regardless of zoom level.

## Measured evidence from the prior experiment

On the same 395-photo Folder test:

- legacy Folder ListView zoom: about 219.9 ms median, 255.3 ms maximum
- direct photo GridView zoom: about 4.9 ms median, 51.7 ms maximum
- direct path performed zero Folder-row rebuilds

This was strong prototype evidence rather than a final controlled benchmark.

## Current branch behavior

The direct Folder GridView is the default.

The original Folder ListView is still present as a fallback. Launch with:

```sh
PICASA_LEGACY_FOLDER_LIST=1 cargo run
```

to compare against the old implementation.

The direct GridView currently uses the existing external folder indicator because GTK GridView does not expose the same full-width section-header factory available to GtkListView in the GTK API used by PIC.

## What has not been changed

This branch deliberately preserves the RC application around the gallery:

- database
- scanner/import pipeline
- albums
- favourites
- editor
- collage
- lightbox/viewer
- context actions
- themes
- network shares

## Immediate validation targets

1. Ctrl-wheel zoom repeatedly from minimum to maximum.
2. Confirm no Folder row rebuild appears in trace output on the default path.
3. Scroll large local folders while zooming.
4. Verify folder navigation lands on the intended photos.
5. Verify multi-selection, activation, context menu and viewer opening.
6. Compare with `PICASA_LEGACY_FOLDER_LIST=1` if a regression appears.
7. Only after interaction parity is stable, solve in-flow Folder section headings/actions without tying model membership to column count.

## Do not merge yet

Keep this branch separate from `rc-bugfixes` and `main` until runtime behavior is verified with the real library.
