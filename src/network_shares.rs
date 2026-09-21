//! Direct, read-only userspace SMB + NFS. No GIO mount or kernel mounts.
use anyhow::{Context, Result};
use gio::prelude::*;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub uri: String,
    pub is_dir: bool,
}
#[derive(Clone, Debug)]
pub struct Metadata {pub size: u64, pub mtime: Option<i64>, pub is_dir: bool}

pub fn private(uri: &str) -> bool { uri.starts_with("nfs://") || uri.starts_with("smb://") }
pub fn trace(area: &str, message: impl std::fmt::Display) {
    if std::env::var_os("PICASA_TRACE").is_some() {
        eprintln!("PIC_NETWORK tid={:?} {area} {message}", std::thread::current().id());
    }
}

pub fn list(uri: &str) -> Result<Vec<Entry>> {
    if uri.starts_with("nfs://") {return crate::private_nfs::list(uri)}
    if uri.starts_with("smb://") {return crate::private_smb::list(uri)}
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}
pub fn read(uri: &str) -> Result<Vec<u8>> {
    if uri.starts_with("nfs://") {return crate::private_nfs::read(uri)}
    if uri.starts_with("smb://") {return crate::private_smb::read(uri)}
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}
/// Bounded offset read for embedded remote RAW previews.
pub fn read_range(uri:&str,offset:u64,length:usize)->Result<Vec<u8>> {
    if uri.starts_with("nfs://") {return crate::private_nfs::read_range(uri,offset,length)}
    if uri.starts_with("smb://") {return crate::private_smb::read_range(uri,offset,length)}
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}
pub fn stat(uri: &str) -> Result<Metadata> {
    if uri.starts_with("nfs://") {return crate::private_nfs::stat(uri)}
    if uri.starts_with("smb://") {return crate::private_smb::stat(uri)}
    anyhow::bail!("Not a direct SMB/NFS URI: {uri}")
}
/// Uses GNOME's network:/// only to discover advertised services. Never opens
/// smb:// or nfs:// through GIO: the private libraries handle those paths.
pub fn discover() -> Result<Vec<Entry>> {
    let root=gio::File::for_uri("network:///");
    let iterator=root.enumerate_children("standard::display-name,standard::target-uri,standard::type",gio::FileQueryInfoFlags::NONE,gio::Cancellable::NONE)
       .context("Cannot discover local network services")?;
    let mut entries=Vec::new();
    while let Some(info)=iterator.next_file(gio::Cancellable::NONE)? {
        let uri=info.attribute_string("standard::target-uri")
            .map(|v|v.to_string()).unwrap_or_else(|| iterator.child(&info).uri().to_string());
        if private(&uri) {entries.push(Entry{name:info.display_name().to_string(),uri,is_dir:true});}
    }
    entries.sort_by(|a,b|a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries.dedup_by(|a,b| a.uri==b.uri);
    Ok(entries)
}

pub fn info(uri: &str) -> Result<gio::FileInfo> {
    let meta=stat(uri)?;
    let info=gio::FileInfo::new();
    let name=gio::File::for_uri(uri).basename().unwrap_or_default();
    info.set_name(&name);
    info.set_file_type(if meta.is_dir {gio::FileType::Directory}else{gio::FileType::Regular});
    info.set_size(i64::try_from(meta.size).unwrap_or(i64::MAX));
    if let Some(mtime)=meta.mtime {if mtime>=0 {info.set_attribute_uint64("time::modified",mtime as u64)}}
    Ok(info)
}
