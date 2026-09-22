//! Minimal direct-share picker for clean main's existing Import Folder action.
//! Discovery alone uses network:///; all SMB/NFS listings are direct userspace.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use gtk4 as gtk;
use gtk::prelude::*;
use crate::network_shares::{self, Entry};

pub fn open(parent: &gtk::Window, on_import: Rc<dyn Fn(String)>) {
    let dialog=gtk::Dialog::with_buttons(
        Some("Browse Network Photos"), Some(parent), gtk::DialogFlags::MODAL,
        &[("Cancel",gtk::ResponseType::Cancel),("Import selected folder",gtk::ResponseType::Accept)]
    );
    dialog.set_default_size(760,520);
    dialog.set_resizable(true);
    dialog.set_default_response(gtk::ResponseType::Accept);
    let content=dialog.content_area();
    content.set_spacing(8);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    let heading=gtk::Label::new(Some("Select a folder on an SMB or NFS share. Original files stay on the server."));
    heading.set_xalign(0.0);
    heading.set_wrap(true);
    content.append(&heading);
    let toolbar=gtk::Box::new(gtk::Orientation::Horizontal,6);
    let discovery=gtk::Button::with_label("Find Network Shares");
    let scan=gtk::Button::with_label("Scan Subnet");
    let up=gtk::Button::with_label("Up");
    let address=gtk::Entry::new();
    address.set_hexpand(true);
    address.set_placeholder_text(Some("smb://server/share/Photos, nfs://server/export, or just server"));
    let go=gtk::Button::with_label("Open URL");
    toolbar.append(&discovery);
    toolbar.append(&scan);
    toolbar.append(&up);
    toolbar.append(&address);
    toolbar.append(&go);
    content.append(&toolbar);
    let location=gtk::Label::new(Some("Find Network Shares"));
    location.set_xalign(0.0);
    location.set_selectable(true);
    location.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    content.append(&location);
    let hint=gtk::Label::new(Some("Scan Subnet probes the local network for SMB/NFS servers, including ones mDNS/WSD does not advertise."));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.set_opacity(0.7);
    content.append(&hint);
    let scroll=gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let rows=gtk::ListBox::new();
    rows.set_selection_mode(gtk::SelectionMode::None);
    rows.set_activate_on_single_click(false);
    scroll.set_child(Some(&rows));
    content.append(&scroll);
    let status=gtk::Label::new(Some("Discovering servers…"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    content.append(&status);

    let current=Rc::new(RefCell::new(String::from("network:///")));
    let entries=Rc::new(RefCell::new(Vec::<Entry>::new()));
    let generation=Rc::new(Cell::new(0u64));
    type Listed=(u64,String,Result<Vec<Entry>,String>);
    let (sender,receiver)=mpsc::channel::<Listed>();

    let current_for_poll=current.clone();
    let entries_for_poll=entries.clone();
    let generation_for_poll=generation.clone();
    let rows_for_poll=rows.clone();
    let status_for_poll=status.clone();
    let location_for_poll=location.clone();
    let address_for_poll=address.clone();
    let generation_for_scan=generation.clone();
    let status_for_scan=status.clone();
    let sender_for_scan=sender.clone();
    let dialog_for_poll=dialog.downgrade();
    glib::timeout_add_local(Duration::from_millis(30),move || {
        if dialog_for_poll.upgrade().is_none() {return glib::ControlFlow::Break}
        while let Ok((id,uri,result))=receiver.try_recv(){
            if id!=generation_for_poll.get() {continue}
            match result {
                Ok(new_entries)=>{
                    *current_for_poll.borrow_mut()=uri.clone();
                    let scan_results=uri=="scan:///";
                    address_for_poll.set_text(if uri=="network:///" || scan_results {""}else{&uri});
                    location_for_poll.set_text(if uri=="network:///" {"Network shares"}else if scan_results {"Scan results: shares found across servers"}else{&uri});
                    status_for_poll.set_text(&if scan_results {
                        format!("{} shares found by probing the subnet — double-click a folder, or Up to return",new_entries.len())
                    } else {
                        format!("{} folders and photos found — double-click a folder to open it",new_entries.len())
                    });
                    *entries_for_poll.borrow_mut()=new_entries;
                    while let Some(child)=rows_for_poll.first_child(){rows_for_poll.remove(&child)}
                    for entry in entries_for_poll.borrow().iter(){
                        let row=gtk::ListBoxRow::new();
                        let label=gtk::Label::new(Some(&format!("{} {}",if entry.is_dir {"📁"}else{"🖼"},entry.name)));
                        label.set_xalign(0.0);
                        label.set_margin_top(5);
                        label.set_margin_bottom(5);
                        label.set_margin_start(8);
                        row.set_child(Some(&label));
                        row.set_activatable(entry.is_dir);
                        rows_for_poll.append(&row);
                    }
                }
                Err(error)=>status_for_poll.set_text(&format!("Could not open {uri}: {error}")),
            }
        }
        glib::ControlFlow::Continue
    });

    let load:Rc<dyn Fn(String)>=Rc::new(move |uri:String|{
        let uri=if uri=="network:///"{
            uri
        }else{
            match network_shares::normalize_input(&uri){
                Some(normalized)=>normalized,
                None=>{
                    status.set_text("That is not a network address. Try smb://server/share or a bare hostname like Ella.local.");
                    return;
                }
            }
        };
        if uri!="network:///" && !network_shares::private(&uri){return}
        let next=generation.get().wrapping_add(1);
        generation.set(next);
        status.set_text("Reading network folder…");
        let tx=sender.clone();
        std::thread::spawn(move || {
            let result=if uri=="network:///"{network_shares::discover()}else{network_shares::list(&uri)};
            let _=tx.send((next,uri,result.map_err(|error|error.to_string())));
        });
    });
    let load_for_rows=load.clone();
    let entries_for_rows=entries.clone();
    rows.connect_row_activated(move |_list,row|{
        if let Some(entry)=entries_for_rows.borrow().get(row.index() as usize) {
            if entry.is_dir{load_for_rows(entry.uri.clone());}
        }
    });
    let load_for_discovery=load.clone();
    discovery.connect_clicked(move |_|load_for_discovery("network:///".to_string()));
    scan.connect_clicked(move |_|{
        let next=generation_for_scan.get().wrapping_add(1);
        generation_for_scan.set(next);
        status_for_scan.set_text("Probing the local subnet for SMB/NFS servers…");
        let tx=sender_for_scan.clone();
        std::thread::spawn(move || {
            let result=network_shares::scan_subnet().map_err(|error|error.to_string());
            let _=tx.send((next,String::from("scan:///"),result));
        });
    });
    let load_for_go=load.clone();
    let address_for_go=address.clone();
    go.connect_clicked(move |_|load_for_go(address_for_go.text().trim().to_string()));
    let load_for_enter=load.clone();
    address.connect_activate(move |entry|load_for_enter(entry.text().trim().to_string()));
    let load_for_up=load.clone();
    let current_for_up=current.clone();
    up.connect_clicked(move |_|{
        let here=current_for_up.borrow().clone();
        if here=="network:///" || here=="scan:///"{return}
        let before=here.trim_end_matches('/');
        let parent=before.rsplit_once('/').map(|(p,_)|p.to_string()).unwrap_or_default();
        if parent=="smb:/" || parent=="nfs:/" || parent.is_empty() {
            load_for_up("network:///".to_string());
        }else {load_for_up(parent);}
    });
    let current_for_submit=current.clone();
    dialog.connect_response(move |dlg,reply|{
        if reply==gtk::ResponseType::Accept {
            let uri=current_for_submit.borrow().clone();
            if uri!="network:///" && network_shares::private(&uri) {on_import(uri);}
        }
        dlg.close();
    });
    dialog.present();
    load("network:///".to_string());
}
