#!/usr/bin/env bash
# Rebuild the bundled icon GResource (resources/icons.gresource).
#
# Bundles the COMPLETE Adwaita SVG icon set (symbolic + scalable fullcolor)
# as flat SVG files at the GResource root. GTK 4.22's
# gtk_icon_theme_add_resource_path() only enumerates the direct children of
# the registered path as unthemed fallback icons, so every icon must sit at
# /picrs/icons/<name>.svg — subdirectory layouts are silently ignored.
#
# Linux resolves names from the system Adwaita theme first; this bundle is
# what makes icons render on Windows/macOS, which ship no system icon theme.
#
# Usage:
#   ./build-icon-bundle.sh [SOURCE_THEME_DIR]
#
#   SOURCE_THEME_DIR defaults to /usr/share/icons/Adwaita (override on
#   macOS with e.g. ~/adwaita/Adwaita after installing adwaita-icon-theme
#   with Homebrew: brew install adwaita-icon-theme).
#
# The generated icons.gresource.xml is checked in so that ordinary builds
# only need glib-compile-resources (or the prebuilt icons.gresource).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ICON_DIR="${SCRIPT_DIR}/icons"
SOURCE_DIR="${1:-${ADWAITA_THEME_DIR:-/usr/share/icons/Adwaita}}"

if [[ ! -d "${SOURCE_DIR}/symbolic" ]]; then
    echo "error: ${SOURCE_DIR} does not look like an Adwaita icon theme" >&2
    exit 1
fi

echo "Importing SVGs from ${SOURCE_DIR}"

# Context search order. First context that provides a name wins; the only
# upstream duplicate is folder-drag-accept-symbolic (places vs status).
CONTEXTS=(
    symbolic/actions
    symbolic/ui
    symbolic/status
    symbolic/places
    symbolic/devices
    symbolic/mimetypes
    symbolic/categories
    symbolic/emotes
    symbolic/legacy
    scalable/actions
    scalable/apps
    scalable/categories
    scalable/devices
    scalable/emblems
    scalable/emotes
    scalable/mimetypes
    scalable/places
    scalable/status
    scalable/ui
)

# Start from a clean slate so stale icons do not linger in the bundle.
rm -f "${ICON_DIR}"/*.svg

seen_file="${SCRIPT_DIR}/.icon-names.tmp"
: > "${seen_file}"

for context in "${CONTEXTS[@]}"; do
    src="${SOURCE_DIR}/${context}"
    [[ -d "${src}" ]] || continue
    for svg in "${src}"/*.svg; do
        [[ -e "${svg}" ]] || continue
        name="$(basename "${svg}")"
        if grep -qxF "${name}" "${seen_file}"; then
            continue
        fi
        echo "${name}" >> "${seen_file}"
        cp "${svg}" "${ICON_DIR}/${name}"
    done
done
rm -f "${seen_file}"

# Stand-ins for names Adwaita upstream does not ship:
# - view-sidebar-symbolic: copy of sidebar-show-symbolic.svg
# - sidebar-hide-symbolic: horizontally mirrored sidebar-show-symbolic.svg
#   (matrix(-1 0 0 1 16 0) flips the 16x16 viewBox)
if [[ ! -e "${ICON_DIR}/view-sidebar-symbolic.svg" ]]; then
    cp "${ICON_DIR}/sidebar-show-symbolic.svg" "${ICON_DIR}/view-sidebar-symbolic.svg"
fi
if [[ ! -e "${ICON_DIR}/sidebar-hide-symbolic.svg" ]]; then
    sed 's/<g fill="#2e3436">/<g fill="#2e3436" transform="matrix(-1 0 0 1 16 0)">/' \
        "${ICON_DIR}/sidebar-show-symbolic.svg" > "${ICON_DIR}/sidebar-hide-symbolic.svg"
fi

count="$(find "${ICON_DIR}" -name '*.svg' | wc -l)"
echo "Imported ${count} icons into resources/icons/"

# Regenerate icons.gresource.xml (sorted, one <file> entry per SVG).
xml="${SCRIPT_DIR}/icons.gresource.xml"
{
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<gresources>'
    echo '  <gresource prefix="/picrs">'
    find "${ICON_DIR}" -name '*.svg' -exec basename {} \; | LC_ALL=C sort | while read -r name; do
        echo "    <file compressed=\"true\" preprocess=\"xml-stripblanks\">icons/${name}</file>"
    done
    echo '  </gresource>'
    echo '</gresources>'
} > "${xml}"

echo "Wrote ${xml}"

glib-compile-resources --sourcedir="${SCRIPT_DIR}" --target="${SCRIPT_DIR}/icons.gresource" "${xml}"
echo "Compiled ${SCRIPT_DIR}/icons.gresource ($(du -h "${SCRIPT_DIR}/icons.gresource" | cut -f1))"
