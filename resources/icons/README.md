# Bundled icon theme subset

`icons.gresource` (compiled from `icons.gresource.xml`) embeds the Adwaita
symbolic icons this app references as **flat SVG files at the resource root**
(`/picrs/icons/<name>-symbolic.svg`).

GTK 4.22's `gtk_icon_theme_add_resource_path()` enumerates only the direct
children of the registered path and registers them as unthemed fallback
icons (extension stripped, SVG preferred). Subdirectory layouts such as
`hicolor/symbolic/actions/...` or `scalable/actions/...` are silently ignored
there, despite what the doc comment implies.

Linux resolves these names from the system Adwaita theme first; the bundle
matters on Windows/macOS, which ship no system icon theme at all.

To add an icon: drop the SVG into this directory, add a `<file>` entry to
`icons.gresource.xml`, and recompile:

    glib-compile-resources --sourcedir=resources --target=resources/icons.gresource \
        resources/icons.gresource.xml

Icon sources: adwaita-icon-theme (LGPL-3.0-or-later OR CC-BY-SA-3.0).
`view-sidebar-symbolic.svg` is a stand-in copy of `sidebar-show-symbolic.svg`
(upstream name absent in the source theme version); `window/build.rs` falls
back to it via its has_icon chain.
