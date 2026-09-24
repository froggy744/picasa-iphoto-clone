# PIC — Text Editor UI Layout Refinement

Update the **Text tab** in the PIC photo editor (Rust + GTK4/libadwaita) to match the approved mockup.

This is a UI layout refinement. Preserve all existing text editing functionality, non-destructive editing, persistence, undo/redo, and export behaviour.

Do not modify the Overlays or Crop panels.

## 1. Add Text / Remove Text

Move the existing **Remove Text** button from the bottom of the panel to the top, directly beside **Add Text**.

Layout:

`[ Add Text ] [ Remove Text ]`

* Both buttons occupy equal width.
* Add Text remains the primary action.
* Remove Text remains a secondary action.
* Remove Text should be disabled when no text layer is selected.
* Keep existing button handlers and functionality.

## 2. Text Layers and Text Content

Keep the existing text layer list and selected text editor.

Maintain this order:

1. Add Text / Remove Text
2. Text Layers
3. Text Content
4. Font family and size
5. Colour
6. Bold / Italic and text alignment
7. Opacity
8. Position and Fit
9. Offset
10. Reset Placement

Use consistent spacing and margins throughout.

## 3. Replace Position Buttons With a 3×3 Grid

Remove the existing five horizontal position buttons:

* Top Left
* Top Right
* Centre
* Bottom Left
* Bottom Right

Replace them with a single compact, square **3×3 position selector**, like the approved mockup.

The grid must provide nine anchor positions:

| Top Left    | Top Centre    | Top Right    |
| ----------- | ------------- | ------------ |
| Centre Left | Centre        | Centre Right |
| Bottom Left | Bottom Centre | Bottom Right |

Requirements:

* Nine equal square cells forming one unified square block.
* No gaps between cells.
* Subtle borders between cells.
* Selected cell highlighted using the existing theme's selection/accent styling.
* Use GNOME symbolic icons or simple alignment glyphs rather than lengthy text labels.
* Tooltips must identify each position.
* Centre is the default position.
* All nine positions must correctly position the selected text layer.
* Preserve the existing drag-to-position functionality.

Do not implement a visual-only selector. Every position must work.

## 4. Position and Fit Layout

Place the position selector and Fit controls side by side.

Example:

```text
POSITION                 FIT

┌────┬────┬────┐         [ Fit to Width  ]
│ ↖  │ ↑  │ ↗  │         [ Fit to Screen ]
├────┼────┼────┤
│ ←  │ ■  │ →  │
├────┼────┼────┤
│ ↙  │ ↓  │ ↘  │
└────┴────┴────┘
```

Both Fit buttons should have the same width and height.

Fit to Width and Fit to Screen must operate on the selected text layer, using the existing editor's coordinate and scaling system.

Keep the positioning controls compact enough to fit inside the adjustable editor panel.

## 5. Offset Controls

Directly beneath Position and Fit, provide the selected text layer's position offsets.

```text
OFFSET

X offset                         [ 0 px  ]
Y offset                         [ 0 px  ]
```

* Preserve the existing offset functionality if already implemented.
* Offset values must update when text is dragged or repositioned.
* Changing offset values must update the text on the canvas.
* Use image-relative coordinates so placement remains consistent across preview zoom levels and export resolution.
* Do not allow changing editor zoom to alter the actual saved text position.

If offsets are currently implicit in the text layer's position data, adapt the existing implementation rather than introducing conflicting position state.

## 6. Reset Placement

Move **Reset Placement** below the Offset controls.

It must be the last control in the Text panel.

```text
OFFSET

X offset                         [ 0 px  ]
Y offset                         [ 0 px  ]

[          Reset Placement          ]
```

Reset Placement must reset the selected text layer's position and offsets to their defaults.

It must NOT remove the text layer, reset its styling, or reset unrelated photo edits.

## 7. Styling

Follow the existing PIC dark GTK4/libadwaita design:

* Compact spacing.
* Consistent button heights.
* Consistent panel margins.
* No oversized empty areas.
* Respect the selected application theme.
* Avoid hardcoded dark colours that break the Standard GTK theme.
* Keep the panel scrollable when the window is smaller.
* Preserve the adjustable editor panel width.

## 8. Scope and Validation

Only change the Text editor UI and the minimum supporting logic necessary for the nine-position selector and offsets.

Do not refactor unrelated editor functionality.

After implementation:

1. Run `cargo fmt --check`.
2. Run `cargo check`.
3. Run relevant editor tests.
4. Verify that all nine positions work.
5. Verify that drag positioning, offset editing, reset placement, undo/redo, saving and export remain functional.
6. Report the files changed and any warnings or failed tests.

**Important:** Preserve all existing behaviour not explicitly changed above. Do not modify the Overlays panel yet.
