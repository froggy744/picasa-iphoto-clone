# PIC rebuild scope

This branch is the clean rebuild of PIC.

## Foundation rule

- Base branch: `prototype/library-folder-groups`.
- The prototype folder-group gallery architecture is the source of truth.
- RC code may supply the shell, CSS, controls, persistence and application features.
- RC gallery/grid architecture must not replace the prototype gallery.

## RC reference

- Visual/feature reference: `rc-bugfixes`.

## Explicitly deferred

Do not add these until the rebuilt core is stable:

- Edit mode
- Collage
- Network Shares

## Port order

1. Preserve prototype grouped continuous gallery behaviour.
2. Match the RC frame, CSS class contract and responsive sidebar/header layout.
3. Add persistent library/indexing and import/refresh.
4. Add albums, favourites and search around the gallery.
5. Add lightbox/viewer, rotation, export and metadata.
6. Add settings/themes and remaining non-deferred RC features.
