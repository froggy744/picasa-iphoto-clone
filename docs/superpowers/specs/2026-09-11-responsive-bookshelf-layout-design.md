# Responsive Bookshelf Layout Design

## Goal

Keep album covers aligned with bookshelf surfaces while the Albums view changes width, without changing the existing no-theme layout.

## Approved visual model

Each bookshelf asset represents one repeating album row. The new `images/bookshelf.png` is the reference asset: it is 2172 by 724 source pixels and the strongest shelf edge is near source y=590. The app renders that source as a 288-logical-pixel row, placing the shelf surface at logical y=235.

The background stretches horizontally with the album grid. Side edges may move or stretch. Vertical size never depends on window width.

## Layout behavior

- The background begins at the top of the album `GtkFlowBox`, leaving the Albums heading and count outside the shelf artwork.
- A responsive themed FlowBox row is exactly 288 logical pixels high and has no additional vertical row spacing.
- Every cover bottom is placed at y=235 within its row. Album name and photo count remain below the cover and in front of the shelf fascia.
- The responsive FlowBox expands with `valign=Fill` to the available viewport height so empty shelf rows continue below the final populated album row. Non-responsive modes restore `valign=Start`.
- Photo-count text uses a darker brown in responsive bookshelf mode to remain legible on the wood. The color provider is attached directly to each count label because a provider local to the FlowBox does not cascade to descendants.
- Resizing changes only how many FlowBox children fit across a row. It does not change the background period, child row height, or shelf surface.
- Bookshelf layout applies whether decorative album covers are enabled or disabled.
- When the bookshelf is disabled, existing row spacing, card sizing, margins, and the plain background remain unchanged.

## Background variants

The three background choices remain available. Index zero uses the new `bookshelf.png` and the responsive row geometry. The two existing multi-shelf images remain unchanged as legacy choices until matching one-row redesigns are supplied; their current presentation must not be made worse by stretching all of their shelves into one row.

The responsive background CSS targets the FlowBox descendant and uses `background-size: 100% 288px`, `background-repeat: repeat-y`, and `background-position: center top`. Legacy background CSS retains its existing behavior.

## Testing

Pure geometry tests cover the fixed 288-pixel row, 235-pixel shelf surface, cover alignment at minimum and maximum cover sizes, and selection of responsive geometry only for index zero. CSS tests ensure the normalized background uses fixed vertical sizing while legacy options remain unchanged. A display-backed GTK regression test checks actual expanded allocation, child height/margin, count-label color, covers-off mode, empty libraries, responsive-to-legacy transitions, and restoration of the previous no-theme FlowBox geometry.

## Recovery

The already-pushed commit `6358a459a70272a051f09962c95b62183128083a` remains unchanged. Responsive bookshelf work is committed separately and can be reverted without removing the context-menu feature.
