# PIC — What Changed

A plain-language summary of the work done on this branch: the fixes, the
performance work, and the small feature tweaks. No code details — just what a
user will notice.

---

## Performance — the big one

PIC was sluggish on large libraries. Opening, scrolling and switching views
could freeze the whole window for seconds. That is fixed.

- **Scrolling the photo grid is smooth again.** The main cause was that the app
  was doing far too much work for every row of photos it showed, including
  scanning the entire library again and again. That repeated work is gone.
- **The Folder view opens dramatically faster.** Building the folder stream
  used to take about a second; it is now a fraction of that.
- **Zooming stays where you are.** Previously, changing the thumbnail size could
  throw the view back to the top of a folder or to a random spot. The view now
  stays on the same photos, and every zoom level fills the rows properly.
- **Switching between views is much faster.** Moving between All Photos,
  Favourites, Albums and Folders no longer rebuilds everything from scratch
  when it doesn't need to.
- **The sidebar no longer stalls the library** while it follows the folder you
  are scrolled to.
- **Thumbnails appear while you scroll** instead of arriving late, and the app
  no longer wastes effort decoding pictures that are far off screen.
- **Startup is quick** and stays quick.

## Freezes and stalls

- Fixed a case where the window could lock up for several seconds when a photo
  lived on a slow or sleeping external drive.
- Fixed long pauses caused by the folder list and the thumbnail grid rebuilding
  themselves more often than necessary.
- Removed a problem where a large library could make the interface feel stuck
  while it reordered the folder view.

## Offline photos and external drives

- Photos whose originals are unavailable are correctly marked again, including
  when a drive is disconnected while the app is running.
- Cached thumbnails still show for offline photos, and the app clearly marks
  the original as unavailable.

## Sidebar

- **The Library section stays put.** Albums and Folders now share one scrolling
  area, so a long album list can no longer push the folders off screen — you
  just scroll down through both.
- **Double-click the Albums or Folders heading** to collapse or expand it.
- **Watched folders are marked** with a small eye on the folder icon, so you can
  see at a glance which folders PIC is watching for changes.
- **Imported-only folder view** lines the folder icons up neatly with the rest
  of the sidebar.

## Photo viewer

- **Drag to pan whenever the photo is zoomed in**, not only at full 1:1 size.
- The cursor shows a **grab hand** whenever you can drag the photo around.
- Pressing **1:1 / space** peeks at the photo at full size, and pressing it
  again returns you to the normal fit-to-window view.

## Collage

- **The collage preview is much sharper.** Tiles used to be drawn from a small
  cached thumbnail and could look soft when enlarged; they are now prepared at
  a much better quality for the preview, while still appearing instantly.

## Edit Mode

- B&W and Sepia can be combined. (Marked as disregarded in the final pass.)

---

## Summary

The headline is simple: **PIC is now fast and responsive on large libraries.**
Scrolling, folder browsing, zooming and switching views all behave the way you
expect, freezes are gone, offline storage is handled properly, and the sidebar
and viewer got a set of small usability improvements.
