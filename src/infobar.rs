use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::photo_object::PhotoObject;

pub struct InfoBar {
    pub root: gtk::Box,
    preview: gtk::Image,
    filename: gtk::Label,
    subtitle: gtk::Label,
    details: gtk::Box,
    pub favorite: gtk::Button,
    pub add_to_album: gtk::MenuButton,
    pub one_to_one: gtk::ToggleButton,
    pub rotate: gtk::Button,
    pub export: gtk::Button,
    pub more: gtk::Button,
    pub grid_zoom_menu: gtk::MenuButton,
    pub grid_zoom_out: gtk::Button,
    pub grid_zoom_reset: gtk::Button,
    pub grid_zoom_in: gtk::Button,
    has_photo: Rc<Cell<bool>>,
}

impl InfoBar {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        root.set_height_request(58);
        root.set_valign(gtk::Align::End);
        root.set_margin_top(0);
        root.set_margin_bottom(0);
        root.set_margin_start(16);
        root.set_margin_end(16);
        root.add_css_class("photo-info-bar");

        let preview = gtk::Image::new();
        preview.set_pixel_size(40);
        preview.set_size_request(40, 40);
        preview.set_width_request(40);
        preview.set_height_request(40);
        preview.set_hexpand(false);
        preview.set_vexpand(false);
        preview.set_halign(gtk::Align::Center);
        preview.set_valign(gtk::Align::Center);
        preview.set_overflow(gtk::Overflow::Hidden);
        preview.add_css_class("info-preview");

        root.append(&preview);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        // Keep enough room for a useful filename, but allow the bottom bar to
        // fit restored/narrow windows without pushing action buttons offscreen.
        text.set_width_request(140);
        text.set_hexpand(false);

        let filename = gtk::Label::new(Some("No photo selected"));
        filename.set_xalign(0.0);
        filename.set_ellipsize(gtk::pango::EllipsizeMode::End);
        filename.add_css_class("info-title");
        text.append(&filename);

        let subtitle = gtk::Label::new(Some("Select a photo to see details"));
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.add_css_class("dim-label");
        text.append(&subtitle);

        root.append(&text);

        let details = gtk::Box::new(gtk::Orientation::Horizontal, 28);
        details.set_hexpand(true);
        details.set_valign(gtk::Align::Center);
        details.set_visible(false);

        for (label, value) in [
            ("Taken", "—"),
            ("Camera", "—"),
            ("Dimensions", "—"),
            ("Size", "—"),
        ] {
            let metric = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let key = gtk::Label::new(Some(label));
            key.set_xalign(0.0);
            key.add_css_class("dim-label");
            key.add_css_class("metric-key");

            let val = gtk::Label::new(Some(value));
            val.set_xalign(0.0);
            val.set_ellipsize(gtk::pango::EllipsizeMode::End);
            val.add_css_class("metric-val");

            metric.append(&key);
            metric.append(&val);
            details.append(&metric);
        }
        root.append(&details);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_valign(gtk::Align::Center);

        let favorite = gtk::Button::from_icon_name("emote-love-symbolic");
        configure_action_button(&favorite);
        favorite.add_css_class("favorite-btn");
        favorite.set_tooltip_text(Some("Add to Favourites"));

        let add_to_album = gtk::MenuButton::new();
        add_to_album.set_icon_name("folder-new-symbolic");
        configure_action_button(&add_to_album);
        add_to_album.set_tooltip_text(Some("Add to Album"));

        let grid_zoom_menu = gtk::MenuButton::new();
        grid_zoom_menu.set_icon_name("view-grid-symbolic");
        configure_action_button(&grid_zoom_menu);
        grid_zoom_menu.set_tooltip_text(Some("Grid size"));
        grid_zoom_menu.set_direction(gtk::ArrowType::Up);

        let grid_zoom_popover = gtk::Popover::new();
        grid_zoom_popover.set_has_arrow(true);
        grid_zoom_popover.set_position(gtk::PositionType::Top);

        let grid_zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        grid_zoom_box.set_margin_top(6);
        grid_zoom_box.set_margin_bottom(6);
        grid_zoom_box.set_margin_start(6);
        grid_zoom_box.set_margin_end(6);

        let grid_zoom_out = gtk::Button::with_label("−");
        configure_action_button(&grid_zoom_out);
        grid_zoom_out.set_width_request(34);
        grid_zoom_out.set_height_request(34);

        let grid_zoom_reset = gtk::Button::with_label("Reset");
        configure_action_button(&grid_zoom_reset);
        grid_zoom_reset.set_width_request(56);
        grid_zoom_reset.set_height_request(34);

        let grid_zoom_in = gtk::Button::with_label("+");
        configure_action_button(&grid_zoom_in);
        grid_zoom_in.set_width_request(34);
        grid_zoom_in.set_height_request(34);

        grid_zoom_box.append(&grid_zoom_out);
        grid_zoom_box.append(&grid_zoom_reset);
        grid_zoom_box.append(&grid_zoom_in);
        grid_zoom_popover.set_child(Some(&grid_zoom_box));
        grid_zoom_menu.set_popover(Some(&grid_zoom_popover));

        let one_to_one = gtk::ToggleButton::with_label("1:1");
        configure_action_button(&one_to_one);
        one_to_one.add_css_class("one-to-one-btn");
        one_to_one.set_tooltip_text(Some("Show at 100% (1:1)"));

        let rotate = gtk::Button::from_icon_name("object-rotate-right-symbolic");
        configure_action_button(&rotate);
        rotate.set_tooltip_text(Some("Rotate clockwise"));

        let export = gtk::Button::from_icon_name("document-save-symbolic");
        configure_action_button(&export);
        export.set_tooltip_text(Some("Export photo"));

        let more = gtk::Button::from_icon_name("emblem-system-symbolic");
        configure_action_button(&more);
        more.set_tooltip_text(Some("Settings"));

        actions.append(&favorite);
        actions.append(&add_to_album);
        actions.append(&grid_zoom_menu);
        actions.append(&one_to_one);
        actions.append(&rotate);
        actions.append(&export);
        actions.append(&more);
        root.append(&actions);

        // The action buttons are the controls that must always remain usable.
        // On narrow windows, reclaim space from optional metadata first, then
        // from filename/preview presentation. This prevents the bar's natural
        // minimum width from making the application appear clipped.
        let has_photo = Rc::new(Cell::new(false));
        let has_photo_for_resize = has_photo.clone();
        let details_for_resize = details.clone();
        let text_for_resize = text.clone();
        let preview_for_resize = preview.clone();
        root.add_tick_callback(move |bar, _| {
            let width = bar.width();
            if width > 0 {
                details_for_resize.set_visible(has_photo_for_resize.get() && width >= 900);
                text_for_resize.set_visible(width >= 620);
                preview_for_resize.set_visible(width >= 520);
            }
            glib::ControlFlow::Continue
        });

        Self {
            root,
            preview,
            filename,
            subtitle,
            details,
            favorite,
            add_to_album,
            one_to_one,
            rotate,
            export,
            more,
            grid_zoom_menu,
            grid_zoom_out,
            grid_zoom_reset,
            grid_zoom_in,
            has_photo,
        }
    }

    pub fn set_photo(&self, photo: Option<&PhotoObject>) {
        let Some(photo) = photo else {
            self.has_photo.set(false);
            self.filename.set_text("No photo selected");
            self.subtitle.set_text("Select a photo to see details");
            self.preview.set_icon_name(Some("image-x-generic-symbolic"));
            self.details.set_visible(false);
            set_metric_values(&self.details, ["—", "—", "—", "—"]);
            self.favorite.set_sensitive(false);
            self.add_to_album.set_sensitive(false);
            self.rotate.set_sensitive(false);
            self.export.set_sensitive(false);
            self.more.set_sensitive(true);
            self.favorite.remove_css_class("active");
            self.favorite.set_icon_name("emote-love-symbolic");
            return;
        };

        self.has_photo.set(true);
        self.filename.set_text(&photo.filename());

        let cached = photo.cached_thumbnail_path();
        let existing = cached.as_deref().filter(|path| Path::new(path).is_file());
        if let Some(thumb_path) = existing {
            if let Some(rotated) =
                crate::photo_texture::rotated_thumbnail(thumb_path, photo.rotation())
            {
                self.preview.set_paintable(Some(&rotated));
            } else {
                self.preview.set_from_file(Some(thumb_path));
            }
        } else {
            self.preview.set_icon_name(Some("image-x-generic-symbolic"));
        }

        self.details.set_visible(self.root.width() >= 900);
        let dimensions = if photo.width() > 0 && photo.height() > 0 {
            format!("{} × {}", photo.width(), photo.height())
        } else {
            "Unknown".to_string()
        };
        let size = format_size(photo.size_bytes());
        let camera = photo
            .camera()
            .unwrap_or_else(|| "Unknown camera".to_string());
        let raw_date = photo
            .taken_at()
            .unwrap_or_else(|| "Unknown date".to_string());
        let formatted_date = format_date(&raw_date);

        self.subtitle.set_text(&camera);
        set_metric_values(&self.details, [formatted_date, camera, dimensions, size]);

        self.favorite.set_sensitive(true);
        self.add_to_album.set_sensitive(true);
        self.rotate.set_sensitive(true);
        self.export.set_sensitive(true);
        self.more.set_sensitive(true);

        if photo.favorite() {
            self.favorite.set_icon_name("emote-love-symbolic");
            self.favorite.add_css_class("active");
            self.favorite
                .set_tooltip_text(Some("Remove from Favourites"));
        } else {
            self.favorite.set_icon_name("emote-love-symbolic");
            self.favorite.remove_css_class("active");
            self.favorite.set_tooltip_text(Some("Add to Favourites"));
        }
    }
}

fn configure_action_button<W: IsA<gtk::Widget>>(button: &W) {
    let widget = button.upcast_ref::<gtk::Widget>();
    widget.set_size_request(34, 34);
    widget.set_width_request(34);
    widget.set_height_request(34);
    widget.set_hexpand(false);
    widget.set_vexpand(false);
    widget.set_halign(gtk::Align::Center);
    widget.set_valign(gtk::Align::Center);
    widget.add_css_class("photo-action-button");
}

fn set_metric_values<const N: usize>(details: &gtk::Box, values: [impl AsRef<str>; N]) {
    let mut child = details.first_child();
    for value in values {
        let Some(metric) = child
            .as_ref()
            .and_then(|widget| widget.downcast_ref::<gtk::Box>())
        else {
            break;
        };
        if let Some(label) = metric.last_child().and_downcast::<gtk::Label>() {
            label.set_text(value.as_ref());
        }
        child = child.and_then(|widget| widget.next_sibling());
    }
}

pub(crate) fn format_size(size: i64) -> String {
    if size >= 1_000_000 {
        format!("{:.1} MB", size as f64 / 1_000_000.0)
    } else if size >= 1_000 {
        format!("{:.1} KB", size as f64 / 1_000.0)
    } else {
        format!("{size} B")
    }
}

pub(crate) fn format_date(value: &str) -> String {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return parsed.format("%b %-d, %Y, %-I:%M %p").to_string();
    }
    if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return parsed.format("%b %-d, %Y, %-I:%M %p").to_string();
    }
    value.to_string()
}
