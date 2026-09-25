fn schedule_scroll_restore(
    adjustment: Option<gtk::Adjustment>,
    scroll_y: f64,
    replace_generation: Rc<Cell<u64>>,
    generation: u64,
) {
    glib::idle_add_local_once(move || {
        if replace_generation.get() != generation {
            return;
        }
        if let Some(adjustment) = adjustment {
            let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
            adjustment.set_value(scroll_y.clamp(adjustment.lower(), upper));
        }
    });
}

fn collect_tiles(widget: &gtk::Widget, tiles: &mut Vec<SquareTile>) {
    if let Some(tile) = widget.downcast_ref::<SquareTile>() {
        tiles.push(tile.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        collect_tiles(&current, tiles);
        child = current.next_sibling();
    }
}
