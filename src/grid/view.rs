pub struct Gallery {
    // GtkGridView must remain the direct GtkScrolledWindow child. GTK's list
    // widgets are GtkScrollable and rely on that relationship for correct
    // visible-item allocation and virtualization. Do not wrap this GridView
    // in a Box/Viewport to implement grouping.
    pub root: gtk::GridView,
    pub v2: Rc<crate::gallery_v2::GalleryV2>,
    pub v2_folder: Rc<crate::gallery_v2::GalleryV2Folder>,
    // Alias of v2_folder.root kept while window/navigation call sites are
    // migrated. It is not a legacy Folder renderer.
    pub folder_root: gtk::ListView,
    pub folder_rubberband: gtk::DrawingArea,
    pub group_header: gtk::Box,
    group_title: gtk::Label,
    group_count: gtk::Label,
    folder_store: gio::ListStore,
    // Reusable Folder stream. Populated whenever the Folder rows are rebuilt
    // and restored when re-entering Folder mode so Open in Folder is instant.
    folder_cache: Rc<RefCell<Option<FolderStreamCache>>>,
    // Presentation-only Folder section order. This must never reorder the
    // shared Library photo model. Empty means use the natural range order.
    folder_order: Rc<RefCell<Vec<i64>>>,
    // Includes parent/container folders that have recursive photos but no
    // direct photo range. Those folders still need a real virtual header so a
    // sidebar click can land on the requested folder rather than its first
    // descendant.
    folder_catalog: Rc<RefCell<Vec<FolderCatalogEntry>>>,
    folder_view_changed: Rc<RefCell<Option<Rc<dyn Fn(bool)>>>>,
    selected: Rc<dyn Fn(Option<PhotoObject>)>,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    collage_selection_mode: Rc<Cell<bool>>,
    collage_selected_ids: Rc<RefCell<HashSet<i64>>>,
    current_columns: Rc<Cell<u32>>,
    last_layout_width: Rc<Cell<i32>>,
    tile_width: Rc<Cell<i32>>,
    tile_height: Rc<Cell<i32>>,
    current_photos: Rc<RefCell<Vec<PhotoObject>>>,
    replace_generation: Rc<Cell<u64>>,
    // True while a progressive gallery replacement is still building batches.
    // Folder navigation relies on this to keep retrying until the virtualized
    // Folder rows actually exist, instead of stopping as soon as the backing
    // photo store contains the target id.
    stream_building: Rc<Cell<bool>>,
    // Folder selected from search while the continuous stream is being built.
    // Keep the target with the Gallery so clearing the search cannot drop it.
    pending_folder_target: Rc<RefCell<Option<(i64, String)>>>,
    group_mode: Rc<Cell<GroupMode>>,
    group_date: Rc<Cell<GroupDate>>,
    group_ranges: Rc<RefCell<Vec<GroupRange>>>,
    last_scroll_y: Rc<Cell<f64>>,
    // Photo at the viewport top captured before a zoom resizes the tiles, so
    // the row reshape can restore the same viewport after the relayout.
    zoom_anchor: Rc<Cell<Option<i64>>>,
    folder_scroll_generation: Rc<Cell<u64>>,
    folder_anchor_photo: Cell<Option<i64>>,
    // True while a column change is still positioning the viewport. A second
    // change before it settles must reuse the original anchor instead of
    // reading GTK's transient (often zeroed) adjustment.
    folder_pending_reframe: Rc<Cell<bool>>,
    folder_reframe_photo: Rc<Cell<Option<i64>>>,
    // Coalesces rapid Ctrl+wheel zoom input: the visual reflow is deferred while
    // the user is still spinning so crossing several column boundaries triggers
    // one Folder row rebuild instead of one per notch.
    pending_zoom_width: Rc<Cell<Option<i32>>>,
    zoom_reflow_source: Rc<RefCell<Option<glib::SourceId>>>,
    // Set when no user-chosen thumbnail size exists: the first real layout
    // adopts the ~4-thumbnails-per-row default instead of a fixed pixel size.
    auto_default_zoom: Cell<bool>,
    // Letterbox whole photos (Contain) instead of cropping to the tile
    // (Cover), so portrait thumbnails show portrait, not a centre strip.
    fit_whole_photo: Rc<Cell<bool>>,
    on_zoom_changed: Rc<dyn Fn(i32)>,
}

impl Gallery {
    pub fn new(
        photos: &[Photo],
        initial_tile_width: i32,
        selected: impl Fn(Option<PhotoObject>) + 'static,
        activate: impl Fn(Vec<PhotoObject>, usize) + 'static,
        context_menu: impl Fn(PhotoObject, gtk::Widget, f64, f64) + 'static,
        unavailable: impl Fn(PhotoObject, gtk::Widget) + 'static,
        on_zoom_changed: impl Fn(i32) + 'static,
    ) -> Self {
        let selected: Rc<dyn Fn(Option<PhotoObject>)> = Rc::new(selected);
        let activate: Rc<dyn Fn(Vec<PhotoObject>, usize)> = Rc::new(activate);
        let context_menu: Rc<dyn Fn(PhotoObject, gtk::Widget, f64, f64)> = Rc::new(context_menu);
        let unavailable: Rc<dyn Fn(PhotoObject, gtk::Widget)> = Rc::new(unavailable);
        let on_zoom_changed: Rc<dyn Fn(i32)> = Rc::new(on_zoom_changed);
        let store = gio::ListStore::new::<PhotoObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let collage_selection_mode = Rc::new(Cell::new(false));
        let collage_selected_ids = Rc::new(RefCell::new(HashSet::new()));
        let current_columns = Rc::new(Cell::new(5u32));
        // Independent thumbnail width/height. Change DEFAULT_TILE_WIDTH and
        // DEFAULT_TILE_HEIGHT above to choose your preferred starting size.
        let tile_width = Rc::new(Cell::new(
            initial_tile_width.clamp(MIN_TILE_WIDTH, MAX_TILE_WIDTH),
        ));
        let tile_height = Rc::new(Cell::new(
            ((DEFAULT_TILE_HEIGHT as f64) * tile_width.get() as f64 / DEFAULT_TILE_WIDTH as f64)
                .round()
                .max(1.0) as i32,
        ));

        // Grouping is presented as a sticky heading outside the scrolled
        // GridView. This deliberately avoids nesting multiple GtkGridViews in
        // a GtkViewport, which broke row allocation and virtualization.
        let group_header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        group_header.set_hexpand(true);
        group_header.set_visible(false);
        group_header.set_margin_start(20);
        group_header.set_margin_end(20);
        group_header.set_margin_top(10);
        group_header.set_margin_bottom(4);
        group_header.add_css_class("group-heading-bar");

        let group_title = gtk::Label::new(None);
        group_title.set_xalign(0.0);
        group_title.add_css_class("section-heading");
        group_header.append(&group_title);

        let group_count = gtk::Label::new(None);
        group_count.set_xalign(0.0);
        group_count.add_css_class("dim-label");
        group_count.add_css_class("section-count");
        group_header.append(&group_count);

        let current_photos = Rc::new(RefCell::new(Vec::<PhotoObject>::new()));

        let factory = gtk::SignalListItemFactory::new();
        let tile_width_for_setup = tile_width.clone();
        let tile_height_for_setup = tile_height.clone();
        let unavailable_for_setup = unavailable.clone();
        let fit_whole_photo = Rc::new(Cell::new(false));
        let fit_whole_photo_for_setup = fit_whole_photo.clone();
        factory.connect_setup(move |_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };

            let frame = gtk::Overlay::new();
            frame.set_overflow(gtk::Overflow::Hidden);
            frame.add_css_class("photo-frame");
            frame.add_css_class("photo-tile");

            let picture = gtk::Picture::new();
            picture.set_content_fit(if fit_whole_photo_for_setup.get() {
                gtk::ContentFit::Contain
            } else {
                gtk::ContentFit::Cover
            });
            picture.set_can_shrink(true);
            picture.set_size_request(1, 1);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            picture.set_halign(gtk::Align::Fill);
            picture.set_valign(gtk::Align::Fill);
            picture.add_css_class("thumbnail");
            frame.set_child(Some(&picture));

            let placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
            placeholder.set_pixel_size(32);
            placeholder.add_css_class("dim-label");
            placeholder.set_visible(false);
            frame.add_overlay(&placeholder);

            let checkmark = gtk::Image::from_icon_name("object-select-symbolic");
            checkmark.set_pixel_size(18);
            checkmark.set_halign(gtk::Align::End);
            checkmark.set_valign(gtk::Align::Start);
            checkmark.set_margin_top(8);
            checkmark.set_margin_end(8);
            checkmark.add_css_class("selection-badge");
            frame.add_overlay(&checkmark);

            let favorite_badge = gtk::Image::from_icon_name("emote-love-symbolic");
            favorite_badge.set_pixel_size(18);
            favorite_badge.set_halign(gtk::Align::End);
            favorite_badge.set_valign(gtk::Align::End);
            favorite_badge.set_margin_bottom(8);
            favorite_badge.set_margin_end(8);
            favorite_badge.add_css_class("favorite-badge");
            favorite_badge.set_visible(false);
            frame.add_overlay(&favorite_badge);

            let edited_badge = gtk::Image::from_icon_name("document-edit-symbolic");
            edited_badge.set_pixel_size(18);
            edited_badge.set_halign(gtk::Align::Start);
            edited_badge.set_valign(gtk::Align::End);
            edited_badge.set_margin_bottom(8);
            edited_badge.set_margin_start(8);
            edited_badge.add_css_class("edited-badge");
            edited_badge.set_tooltip_text(Some("Edited"));
            edited_badge.set_visible(false);
            frame.add_overlay(&edited_badge);

            let unavailable_badge = gtk::Button::with_label("!");
            unavailable_badge.set_halign(gtk::Align::Start);
            unavailable_badge.set_valign(gtk::Align::Start);
            unavailable_badge.set_margin_top(8);
            unavailable_badge.set_margin_start(8);
            unavailable_badge.add_css_class("offline-badge");
            unavailable_badge.set_tooltip_text(Some("Original photo unavailable"));
            unavailable_badge.set_visible(false);
            frame.add_overlay(&unavailable_badge);

            let list_item_for_unavailable = list_item.clone();
            let unavailable_for_click = unavailable_for_setup.clone();
            let badge_for_click = unavailable_badge.clone();
            unavailable_badge.connect_clicked(move |_| {
                if let Some(photo) = list_item_for_unavailable
                    .item()
                    .and_downcast::<PhotoObject>()
                {
                    (unavailable_for_click)(photo, badge_for_click.clone().upcast());
                }
            });

            let tile = SquareTile::new(
                tile_width_for_setup.get(),
                tile_height_for_setup.get(),
                &frame,
            );
            tile.set_hexpand(true);
            tile.set_vexpand(false);
            tile.set_valign(gtk::Align::Start);
            list_item.set_child(Some(&tile));
        });

        factory.connect_bind(|_, object| {
            let Some(list_item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(photo) = list_item.item().and_downcast::<PhotoObject>() else {
                return;
            };
            let Some(tile) = list_item.child().and_downcast::<SquareTile>() else {
                return;
            };
            tile.imp()
                .photo_index
                .set(Some(list_item.position() as usize));
            tile.bind_photo(&photo);
        });

        let root = gtk::GridView::new(Some(selection.clone()), Some(factory));
        root.set_min_columns(5);
        root.set_max_columns(5);
        root.set_single_click_activate(false);
        root.set_enable_rubberband(true);
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.set_halign(gtk::Align::Fill);
        root.set_valign(gtk::Align::Fill);
        root.add_css_class("section-grid");

        // GridView children are recycled and the pointer may land on any
        // descendant of a tile. Use one stable controller on GridView, pick
        // the tile under the pointer, and only claim the event after a bound
        // photo has been identified. Select an unselected clicked photo so
        // context-menu actions and focus restoration target that thumbnail.
        // Right-clicking within an existing multi-selection preserves it.
        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let context_menu_for_grid = context_menu.clone();
        let root_for_context = root.clone();
        let selection_for_context = selection.clone();
        right_click.connect_pressed(move |gesture, _, x, y| {
            let Some(picked) = root_for_context.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(SquareTile::static_type())
                .and_downcast::<SquareTile>()
            else {
                return;
            };
            let Some(photo) = tile.imp().photo.borrow().as_ref().cloned() else {
                return;
            };
            let Some(position) = (0..selection_for_context.n_items()).find(|position| {
                selection_for_context
                    .item(*position)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|item| item.id() == photo.id())
            }) else {
                return;
            };
            let Some(frame) = tile.first_child().and_downcast::<gtk::Overlay>() else {
                return;
            };

            let frame_widget = frame.clone().upcast::<gtk::Widget>();
            let local = root_for_context
                .compute_point(
                    &frame_widget,
                    &gtk::graphene::Point::new(x as f32, y as f32),
                )
                .unwrap_or_else(|| gtk::graphene::Point::new(0.0, 0.0));

            gesture.set_state(gtk::EventSequenceState::Claimed);
            if !selection_for_context.is_selected(position) {
                selection_for_context.select_item(position, true);
            }
            root_for_context.grab_focus();
            (context_menu_for_grid)(photo, frame_widget, local.x() as f64, local.y() as f64);
        });
        root.add_controller(right_click);

        // Handle Add Photos clicks on the GridView itself, before the
        // built-in GridView selection controller sees them. This makes the
        // mode behave like a checklist: each click toggles one item and does
        // not collapse the other selected photos.
        let collage_click = gtk::GestureClick::new();
        collage_click.set_button(1);
        collage_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let mode = collage_selection_mode.clone();
        let selection_for_click = selection.clone();
        let collage_selected_ids_for_click = collage_selected_ids.clone();
        let root_for_click = root.clone();
        collage_click.connect_pressed(move |gesture, _, x, y| {
            if !mode.get() {
                return;
            }
            let Some(picked) = root_for_click.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(tile) = picked
                .ancestor(SquareTile::static_type())
                .and_downcast::<SquareTile>()
            else {
                return;
            };
            let Some(photo_id) = tile.imp().photo.borrow().as_ref().map(|photo| photo.id()) else {
                return;
            };
            let Some(position) = (0..selection_for_click.n_items()).find(|position| {
                selection_for_click
                    .item(*position)
                    .and_downcast::<PhotoObject>()
                    .is_some_and(|photo| photo.id() == photo_id)
            }) else {
                return;
            };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let mut selected_ids = collage_selected_ids_for_click.borrow_mut();
            if selected_ids.remove(&photo_id) {
                selection_for_click.unselect_item(position);
            } else {
                selected_ids.insert(photo_id);
                selection_for_click.select_item(position, false);
            }
        });
        root.add_controller(collage_click);

        let selected_for_signal = selected.clone();
        selection.connect_selection_changed(move |selection, _, _| {
            let selected = selection.selection();
            let photo = gtk::BitsetIter::init_first(&selected)
                .and_then(|(_, position)| selection.item(position))
                .and_downcast::<PhotoObject>();
            (selected_for_signal)(photo);
        });

        let current_photos_for_activate = current_photos.clone();
        let selection_for_activate = selection.clone();
        let activate_for_grid = activate.clone();
        root.connect_activate(move |_, position| {
            let Some(activated) = selection_for_activate
                .model()
                .and_then(|model| model.item(position))
                .and_downcast::<PhotoObject>()
            else {
                return;
            };
            let photos = current_photos_for_activate.borrow().clone();
            let index = photos
                .iter()
                .position(|photo| photo.id() == activated.id())
                .unwrap_or(position as usize);
            (activate_for_grid)(photos, index);
        });

        // Folder rendering is V2-only. The old chunked Folder ListView,
        // row cache, custom selection controller and thumbnail pump were
        // removed; Folder now has one implementation path.
        let selection_for_v2 = selection.clone();
        let store_for_v2 = store.clone();
        let v2_selection_guard = Rc::new(Cell::new(false));
        let guard_for_v2 = v2_selection_guard.clone();
        let v2 = Rc::new(crate::gallery_v2::GalleryV2::new(
            tile_width.get(),
            tile_height.get(),
            selected.clone(),
            activate.clone(),
            context_menu.clone(),
            unavailable.clone(),
            Rc::new(move |ids| {
                if guard_for_v2.replace(true) {
                    return;
                }
                selection_for_v2.unselect_all();
                if !ids.is_empty() {
                    let wanted = ids.iter().copied().collect::<HashSet<_>>();
                    for position in 0..store_for_v2.n_items() {
                        if store_for_v2
                            .item(position)
                            .and_downcast::<PhotoObject>()
                            .is_some_and(|photo| wanted.contains(&photo.id()))
                        {
                            selection_for_v2.select_item(position, false);
                        }
                    }
                }
                guard_for_v2.set(false);
            }),
        ));

        let v2_folder = Rc::new(crate::gallery_v2::GalleryV2Folder::new(
            tile_width.get(),
            tile_height.get(),
            selected.clone(),
            activate.clone(),
            context_menu.clone(),
            unavailable.clone(),
        ));

        // Transitional field alias for callers that only need a focus/scroll
        // root. It points at the V2 ListView; there is no second Folder widget.
        let folder_root = v2_folder.root.clone();
        let folder_rubberband = gtk::DrawingArea::new();
        folder_rubberband.set_can_target(false);
        folder_rubberband.set_visible(false);
        let folder_store = gio::ListStore::new::<glib::Object>();

        {
            let v2 = v2.clone();
            let guard = v2_selection_guard.clone();
            selection.connect_selection_changed(move |selection, _, _| {
                if guard.replace(true) {
                    return;
                }
                let bitset = selection.selection();
                let mut ids = Vec::new();
                if let Some((iter, first)) = gtk::BitsetIter::init_first(&bitset) {
                    if let Some(photo) = selection.item(first).and_downcast::<PhotoObject>() {
                        ids.push(photo.id());
                    }
                    for position in iter {
                        if let Some(photo) = selection.item(position).and_downcast::<PhotoObject>() {
                            ids.push(photo.id());
                        }
                    }
                }
                v2.set_selected_ids(&ids);
                guard.set(false);
            });
        }

        let gallery = Self {
            root,
            v2,
            v2_folder,
            folder_root,
            folder_rubberband,
            group_header,
            group_title,
            group_count,
            folder_store,
            folder_cache: Rc::new(RefCell::new(None)),
            folder_order: Rc::new(RefCell::new(Vec::new())),
            folder_catalog: Rc::new(RefCell::new(Vec::new())),
            folder_view_changed: Rc::new(RefCell::new(None)),
            selected,
            store,
            selection,
            collage_selection_mode,
            collage_selected_ids,
            current_columns,
            last_layout_width: Rc::new(Cell::new(0)),
            tile_width,
            tile_height,
            current_photos,
            replace_generation: Rc::new(Cell::new(0)),
            stream_building: Rc::new(Cell::new(false)),
            pending_folder_target: Rc::new(RefCell::new(None)),
            group_mode: Rc::new(Cell::new(GroupMode::None)),
            group_date: Rc::new(Cell::new(GroupDate::Taken)),
            group_ranges: Rc::new(RefCell::new(Vec::new())),
            last_scroll_y: Rc::new(Cell::new(0.0)),
            zoom_anchor: Rc::new(Cell::new(None)),
            folder_scroll_generation: Rc::new(Cell::new(0)),
            folder_anchor_photo: Cell::new(None),
            folder_pending_reframe: Rc::new(Cell::new(false)),
            folder_reframe_photo: Rc::new(Cell::new(None)),
            pending_zoom_width: Rc::new(Cell::new(None)),
            zoom_reflow_source: Rc::new(RefCell::new(None)),
            auto_default_zoom: Cell::new(false),
            fit_whole_photo,
            on_zoom_changed,
        };
        gallery.replace(photos);
        gallery
    }

    /// Column count a content width produces for the current tile size.
    /// The ceiling exists so absurdly narrow tiles cannot appear, not to
    /// limit wide monitors: on 2K/4K surfaces the default 300px tiles fill
    /// 9-11 columns, and capping below that leaves dead space on the right.
    fn columns_for_width(&self, width: i32) -> u32 {
        let available = (width - 48).max(200);
        ((available as f64) / (self.tile_width.get() as f64 + 30.0))
            .floor()
            .clamp(1.0, 12.0) as u32
    }

    pub fn update_width(&self, width: i32) {
        self.v2.update_width(width);
        self.v2_folder.update_width(width);
        self.update_layout(width, false);
    }

    fn update_layout(&self, width: i32, tile_size_changed: bool) {
        // First real allocation with no stored thumbnail preference: adopt
        // the ~4-thumbnails-per-row default for this surface width. Session
        // only - it becomes a preference if the user zooms manually.
        if self.auto_default_zoom.get() && width > 0 {
            self.auto_default_zoom.set(false);
            let target = zoom_level_for_four_columns(width);
            if target != self.tile_width.get() {
                self.apply_tile_size(target, false);
                return;
            }
        }
        let old_columns = self.current_columns.get();
        let old_width = self.last_layout_width.get();
        let columns = self.columns_for_width(width);
        if width == old_width && columns == old_columns && !tile_size_changed {
            return;
        }
        self.last_layout_width.set(width);
        let folder_mode = self.group_mode.get() == GroupMode::Folder;
        let folder_v2 = folder_mode && std::env::var_os("PIC_GALLERY_V2").is_some();
        if columns == old_columns {
            // Zooming within the same column count only changes tile geometry.
            // Replacing the Folder ListStore here used to invalidate every
            // realized row and cost ~0.8-1.1s for a 4.5k-photo library.
            if folder_mode && tile_size_changed {
                if folder_v2 {
                    // Folder V2 already received the exact same tile geometry
                    // through set_tile_size/update_width above. Do not run the
                    // legacy Folder anchor/row machinery on a hidden widget.
                    return;
                }
                // Tile size changed within the same columns: the rows keep
                // their photos but their heights change, so re-anchor the
                // viewport to the photo that was at the top.
                let anchor = self.take_reframe_anchor();
                update_folder_realized_rows(
                    self.folder_root.upcast_ref(),
                    self.tile_width.get(),
                    self.tile_height.get(),
                );
                self.folder_root.queue_resize();
                if let Some(anchor) = anchor {
                    self.scroll_folder_to_photo(anchor);
                }
            } else if !folder_mode {
                self.root.queue_resize();
                self.update_group_header_for_scroll(self.last_scroll_y.get());
            }
            // Width-only changes with the same columns need no vertical layout
            // work. GTK stretches the row boxes itself. Re-anchoring here made
            // every sidebar animation frame issue competing scroll requests.
            return;
        }

        let previous_columns = self.current_columns.replace(columns);
        if std::env::var_os("PICASA_TRACE").is_some() && previous_columns != columns { eprintln!("PIC_NAV current_columns_changed old={} new={}", previous_columns, columns); }
        self.root.set_min_columns(columns);
        self.root.set_max_columns(columns);
        self.root.queue_resize();
        if folder_mode {
            if folder_v2 {
                // V2 changes only GridView geometry; section/photo membership
                // remains persistent across zoom.
                return;
            }
            // Each model item is one visual photo line. A column change must
            // rebuild those lines to keep the layout gapless.
            let anchor = self.take_reframe_anchor();
            self.rebuild_folder_rows();
            if let Some(anchor) = anchor {
                self.scroll_folder_to_photo(anchor);
            }
        } else {
            self.update_group_header_for_scroll(self.last_scroll_y.get());
        }
    }

}
