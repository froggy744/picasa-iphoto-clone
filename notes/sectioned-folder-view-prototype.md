# Picasa-style Folder View — Prototype B Notes

## Current conclusion

Prototype B (`prototype-sectioned-view`) is the strongest architecture tested so far for a Picasa-style continuous Folder view.

The design keeps:
- one stable photo item per photo;
- a flat photo model;
- folder grouping as presentation metadata;
- zoom as geometry-only;
- bounded widget realization through recycling;
- continuous scrolling;
- real full-width folder boundaries/headers;
- fresh photo rows at every folder boundary.

## Architectures already tested

### Rejected: old multi-photo row ListView
The old Folder-mode architecture grouped several photos into row objects based on current column count.

Problems:
- changing zoom changed the row/model structure;
- rows had to be rebuilt whenever column count changed;
- relayout and scroll anchoring became expensive;
- zoom smoothness suffered.

Do not return to this design.

### Rejected: ListView -> FlowBox per folder
A prototype used:
- outer GtkListView: one row per folder;
- inner GtkFlowBox: photos in that folder.

Observed result with 5,000 photos:
- all 50 FlowBoxes were live;
- all 5,000 tile widgets were live;
- the 2,000-photo stress folder created 2,000 widgets;
- virtualization therefore failed at the photo level.

Do not integrate this design into PIC.

## Prototype B architecture

Experimental branch:
`prototype-sectioned-view`

Prototype binary:
`src/bin/sectioned-view-prototype.rs`

Concept:

```
GtkScrolledWindow
    |
    +-- section-aware virtualized surface
          |
          +-- flat photo model
          +-- Vec<FolderSection>
          +-- SectionGeometry / LayoutIndex
          +-- small recycled pool of photo tiles
          +-- small recycled pool of headers
```

The prototype currently uses a GtkFixed-based surface to prove the layout/virtualization behavior before writing a production-quality custom GtkWidget/GtkScrollable implementation.

## Prototype B results

Synthetic test:
- 5,000 photos total;
- 50 folders;
- one stress-test folder containing 2,000 photos.

### Virtualization
Observed examples:
- 56 live tiles, 84 total created;
- after jumping to the 2,000-photo folder: ~77-82 live tiles;
- after extensive zoom testing: 101 live tiles, 116 pooled, 217 total created.

Important:
The 2,000-photo folder did NOT create 2,000 tile widgets.

### Geometry
Section geometry rebuilds were generally about 0-8 microseconds in the synthetic test.

Zoom does not:
- rebuild the photo model;
- rebuild folder membership;
- create row objects based on columns;
- change model item identity.

### Zoom anchoring
The corrected zoom test preserved the same stable photo ID through repeated zoom in/out sequences.

Observed:
`anchor_error_px=0.000`

This confirms the geometry can reposition the same anchor photo exactly when columns/tile size change.

### Folder navigation
Direct jump to the 2,000-photo folder remained sub-millisecond in the synthetic test.

## Important remaining concern

Peak RSS rose to roughly 290 MB during the heavier zoom run.

This does NOT currently invalidate the layout architecture, but the recycle pool should not grow indefinitely.

Before production integration:
- cap the recycled tile pool;
- cap the recycled header pool;
- re-run memory testing;
- distinguish GTK allocator high-water behavior from genuinely retained widgets.

## Next recommended work

Do NOT integrate directly into production `rc6` yet.

Next stage should remain on an experimental branch:

1. Cap the recycle pool.
2. Re-test synthetic memory behavior.
3. Replace dummy tiles with the real PIC photo tile presentation.
4. Reuse the existing PhotoObject model.
5. Reuse existing thumbnail cache/loaders.
6. Reuse existing zoom ladder.
7. Reuse existing stable photo-ID anchor concepts.
8. Reuse existing selection state where practical.
9. Feed real folder ranges into the section metadata.
10. Test with the real photo library and real thumbnails.
11. Measure:
    - live tile count;
    - total tile creation;
    - thumbnail loading;
    - scroll smoothness;
    - Ctrl+wheel zoom smoothness;
    - anchor stability;
    - memory;
    - sidebar folder jumps.
12. Only if the real-photo prototype remains smooth should Folder mode in `rc6` be replaced.

## Hard architecture rules

These should remain non-negotiable:

- One photo = one stable model item.
- Folder grouping is presentation metadata.
- Zoom changes geometry only.
- Zoom must never rebuild photo membership.
- Do not create synthetic header/padding photo items.
- Do not rebuild multi-photo row objects when column count changes.
- Keep thumbnail decode/cache independent from section layout.
- Only realize visible/near-visible photo widgets.
- Folder-to-folder navigation should reuse the loaded continuous stream whenever possible.

## Current status

Prototype B has passed the two main proof-of-concept gates:
- bounded virtualization;
- stable geometry-only zoom anchoring.

The next advice/review should be evaluated against these measured results before changing direction.
