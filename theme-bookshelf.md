# Bookshelf Theme TODO

## Before implementation

- Finalize the bookshelf background artwork and aspect ratio.
- Finalize the album overlay artwork and confirm all overlays share one geometry.
- Measure each shelf's vertical baseline in the background.
- Measure the overlay photo opening and overall dimensions.
- Decide the target album size and spacing at different window widths.

## Implementation

- Change only the Bookshelf renderer.
- Preserve the current responsive thumbnail sizing, wrapping, and spacing.
- Anchor each Bookshelf row to its shelf baseline during resize.
- Keep Default and Album Covers unchanged.
- Verify empty albums, portrait and landscape covers, long titles, and multiple rows.

## Verification

- Resize the window through several widths and confirm albums remain seated on shelves.
- Confirm overlays stay aligned with their photo openings.
- Confirm switching to Default or Album Covers restores their existing layouts.
