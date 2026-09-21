//! Direct SMB access using Samba libsmbclient; does not mount GVfs.
use crate::network_shares::Entry;
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};

unsafe extern "C" {
    fn pic_smb_list(uri: *const c_char, cb: extern "C" fn(*mut c_void,*const c_char,c_uint)->c_int,
        ctx: *mut c_void, error: *mut c_char, capacity: usize) -> c_int;
    fn pic_smb_read(uri: *const c_char, out: *mut *mut u8, length: *mut usize,
        max_bytes: usize, error: *mut c_char, capacity: usize) -> c_int;
    fn pic_smb_read_range(uri:*const c_char,offset:u64,requested:usize,out:*mut *mut u8,length:*mut usize,error:*mut c_char,capacity:usize)->c_int;
    fn pic_smb_stat(uri:*const c_char,size:*mut u64,mtime:*mut i64,
        is_dir:*mut c_int,error:*mut c_char,cap:usize)->c_int;
    fn pic_smb_free(bytes: *mut c_void);
}
fn c_error(buf: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}
fn encoded_segment(name: &str) -> String {
    let mut result=String::new();
    for &byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~()".contains(&byte) { result.push(byte as char); }
        else { result.push_str(&format!("%{byte:02X}")); }
    }
    result
}
extern "C" fn receive_entry(context:*mut c_void, name:*const c_char, kind:c_uint) -> c_int {
    if context.is_null() || name.is_null() { return -1; }
    let (parent,entries) = unsafe { &mut *(context as *mut (String,Vec<Entry>)) };
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
    // Samba: 3=share, 7=directory, 8=file; 9=symlink.
    let is_dir=matches!(kind,3|7);
    if !(is_dir || kind==8 && crate::image_format::supported(std::path::Path::new(&name))) { return 0; }
    let uri=format!("{}/{}",parent.trim_end_matches('/'),encoded_segment(&name));
    entries.push(Entry {name,uri,is_dir});
    0
}
pub fn list(uri:&str)->anyhow::Result<Vec<Entry>> {
    let uri = CString::new(uri)?;
    let mut context=(uri.to_string_lossy().into_owned(),Vec::<Entry>::new());
    let mut error=[0 as c_char;512];
    crate::network_shares::trace("PRIVATE_SMB",format!("list_start uri={}",context.0));
    let count=unsafe {pic_smb_list(uri.as_ptr(),receive_entry,
        &mut context as *mut _ as *mut c_void,error.as_mut_ptr(),error.len())};
    if count<0 {anyhow::bail!("{}",c_error(&error));}
    context.1.sort_by(|a,b|b.is_dir.cmp(&a.is_dir).then_with(||a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    crate::network_shares::trace("PRIVATE_SMB",format!("list_done count={count} displayed={}",context.1.len()));
    Ok(context.1)
}
pub fn read(uri:&str)->anyhow::Result<Vec<u8>> {
    const LIMIT:usize=128*1024*1024;
    let uri=CString::new(uri)?;
    let mut data: *mut u8=std::ptr::null_mut();
    let mut length=0usize;
    let mut error=[0 as c_char;512];
    let result=unsafe {pic_smb_read(uri.as_ptr(),&mut data,&mut length,LIMIT,error.as_mut_ptr(),error.len())};
    if result<0 {anyhow::bail!("{}",c_error(&error));}
    let bytes=unsafe {std::slice::from_raw_parts(data,length).to_vec()};
    unsafe {pic_smb_free(data as *mut c_void)};
    Ok(bytes)
}
pub fn read_range(uri:&str,offset:u64,requested:usize)->anyhow::Result<Vec<u8>> {
    anyhow::ensure!(requested<=100*1024*1024,"SMB range exceeds preview safety limit");
    let uri=CString::new(uri)?;let mut data=std::ptr::null_mut();let mut length=0usize;let mut error=[0 as c_char;512];
    if unsafe{pic_smb_read_range(uri.as_ptr(),offset,requested,&mut data,&mut length,error.as_mut_ptr(),error.len())}<0 {anyhow::bail!("{}",c_error(&error));}
    let bytes=if length==0{Vec::new()}else{unsafe{std::slice::from_raw_parts(data,length).to_vec()}};
    unsafe{pic_smb_free(data as *mut c_void)};
    crate::network_shares::trace("PRIVATE_SMB",format!("range offset={offset} requested={requested} received={}",bytes.len()));Ok(bytes)
}
#[cfg(test)] mod tests { use super::*; #[test] fn safe_segment() {
    assert_eq!(encoded_segment("Home Movies"),"Home%20Movies");
    assert_eq!(encoded_segment("a#b"),"a%23b");
} }

pub fn stat(uri:&str)->anyhow::Result<crate::network_shares::Metadata>{
    let uri=CString::new(uri)?;
    let mut size=0u64;let mut mtime=0i64;let mut dir=0i32;
    let mut buffer=[0 as c_char;512];
    let status=unsafe{pic_smb_stat(uri.as_ptr(),&mut size,&mut mtime,&mut dir,buffer.as_mut_ptr(),buffer.len())};
    if status<0 {anyhow::bail!("{}",c_error(&buffer))}
    Ok(crate::network_shares::Metadata{size,mtime:Some(mtime),is_dir:dir!=0})
}
