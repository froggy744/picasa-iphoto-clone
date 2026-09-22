//! Direct userspace NFS through libnfs, no kernel or GVfs mount.
use crate::network_shares::Entry;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::sync::{Mutex, OnceLock};
use std::collections::HashMap;

type Callback = extern "C" fn(*mut c_void, *const c_char, c_uint) -> c_int;
unsafe extern "C" {
    fn pic_nfs_exports(host:*const c_char,cb:Callback,ctx:*mut c_void,error:*mut c_char,cap:usize)->c_int;
    fn pic_nfs_list(host:*const c_char, export_path:*const c_char, relative:*const c_char,
        cb:Callback,ctx:*mut c_void,error:*mut c_char,cap:usize)->c_int;
    fn pic_nfs_read(host:*const c_char, export_path:*const c_char, relative:*const c_char,
        out:*mut *mut u8,length:*mut usize,max_bytes:usize,error:*mut c_char,cap:usize)->c_int;
    fn pic_nfs_read_range(host:*const c_char,export_path:*const c_char,relative:*const c_char,offset:u64,requested:usize,out:*mut *mut u8,length:*mut usize,error:*mut c_char,cap:usize)->c_int;
    fn pic_nfs_stat(host:*const c_char, export_path:*const c_char, relative:*const c_char,
        size:*mut u64,mtime:*mut i64,is_dir:*mut c_int,error:*mut c_char,cap:usize)->c_int;
    fn pic_smb_free(data:*mut c_void);
}
fn err(buffer:&[c_char])->String{ unsafe{CStr::from_ptr(buffer.as_ptr())}.to_string_lossy().into_owned() }
fn encode(segment:&str)->String {
    let mut out=String::new();
    for &b in segment.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~()".contains(&b) {out.push(b as char)}
        else {out.push_str(&format!("%{b:02X}"))}
    }
    out
}
fn decode(segment:&str)->anyhow::Result<String>{
    let b=segment.as_bytes();let mut out=Vec::new();let mut pos=0;
    while pos<b.len() {
        if b[pos]==b'%' {
            anyhow::ensure!(pos+2<b.len(),"Invalid NFS URL percent escape");
            let hex=|c:u8|->anyhow::Result<u8>{match c{b'0'..=b'9'=>Ok(c-b'0'),b'a'..=b'f'=>Ok(c-b'a'+10),b'A'..=b'F'=>Ok(c-b'A'+10),_=>anyhow::bail!("Invalid URL escape")}};
            let n=hex(b[pos+1])?*16+hex(b[pos+2])?;
            anyhow::ensure!(n!=b'/' && n!=0,"Invalid escaped NFS path separator");
            out.push(n);pos+=3;
        }else{out.push(b[pos]);pos+=1;}
    }
    Ok(String::from_utf8(out)?)
}
fn parsed(uri:&str)->anyhow::Result<(String,String)> {
    let rest=uri.strip_prefix("nfs://").ok_or_else(||anyhow::anyhow!("Not NFS URL"))?;
    let (authority,path)=rest.split_once('/').unwrap_or((rest,""));
    anyhow::ensure!(!authority.is_empty(),"Missing NFS server address");
    // URI comes from GNOME Zeroconf; libnfs takes hostname separately, port from standard NFS settings.
    let host=authority.rsplit_once(':').map(|(h,port)| if port.chars().all(|c|c.is_ascii_digit()) {h} else {authority}).unwrap_or(authority);
    anyhow::ensure!(!host.is_empty() && !host.contains('@') && !host.contains('['),"Unsupported NFS host");
    let segments=path.split('/').filter(|s|!s.is_empty()).map(decode).collect::<anyhow::Result<Vec<_>>>()?;
    anyhow::ensure!(segments.iter().all(|s|s!="." && s!=".."),"Unsafe NFS path");
    Ok((host.to_owned(),format!("/{}",segments.join("/"))))
}
extern "C" fn names(context:*mut c_void, name:*const c_char, kind:c_uint)->c_int {
    if context.is_null() || name.is_null(){return -1;}
    let results=unsafe{&mut *(context as *mut Vec<(String,u32)>)};
    let name=unsafe{CStr::from_ptr(name)}.to_string_lossy().into_owned();
    results.push((name,kind));0
}
static EXPORTS:OnceLock<Mutex<HashMap<String,Vec<String>>>>=OnceLock::new();
fn exports(host:&str)->anyhow::Result<Vec<String>> {
    let cache=EXPORTS.get_or_init(||Mutex::new(HashMap::new()));
    if let Some(previous)=cache.lock().unwrap().get(host).cloned(){
        crate::network_shares::trace("PRIVATE_NFS",format!("export_cache_hit host={host} count={}",previous.len()));
        return Ok(previous)
    }
    let started=std::time::Instant::now();
    crate::network_shares::trace("PRIVATE_NFS",format!("export_discovery_start host={host}"));
    let mut results: Vec<(String,u32)> = Vec::new();let mut buffer=[0 as c_char;512];let chost=CString::new(host)?;
    let returned=unsafe{pic_nfs_exports(chost.as_ptr(),names,&mut results as *mut _ as *mut c_void,
        buffer.as_mut_ptr(),buffer.len())};
    if returned<0{anyhow::bail!("{}",err(&buffer))}
    let mut paths=results.into_iter().map(|(path,_)|path.trim_end_matches('/').to_owned()).collect::<Vec<_>>();
    paths.sort();paths.dedup();
    crate::network_shares::trace("PRIVATE_NFS",format!("export_discovery_done host={host} count={} elapsed_ms={} paths={paths:?}",paths.len(),started.elapsed().as_millis()));
    cache.lock().unwrap().insert(host.to_owned(),paths.clone());
    Ok(paths)
}
fn resolve(host:&str,path:&str)->anyhow::Result<(String,String)> {
    let all=exports(host)?;
    let chosen=all.into_iter().filter(|export|path==export || path.strip_prefix(export.as_str()).is_some_and(|tail|tail.starts_with('/')))
        .max_by_key(|export|export.len());
    let export=chosen.ok_or_else(||anyhow::anyhow!("NFS path {path} is not an exported directory on {host}"))?;
    let relative=path.strip_prefix(&export).unwrap_or("");
    Ok((export,if relative.is_empty(){"/".into()}else{relative.into()}))
}
fn children(uri:&str,results:Vec<(String,u32)>)->Vec<Entry>{
    let mut entries=results.into_iter().filter_map(|(name,kind)|{
        let is_dir=kind==7;
        if !is_dir && !(kind==8 && crate::image_format::supported(std::path::Path::new(&name))){return None}
        let uri=format!("{}/{}",uri.trim_end_matches('/'),encode(&name));
        Some(Entry{name,uri,is_dir,server:String::new()})
    }).collect::<Vec<_>>();
    entries.sort_by(|a,b|b.is_dir.cmp(&a.is_dir).then_with(||a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries
}
pub fn list(uri:&str)->anyhow::Result<Vec<Entry>>{
    let (host,path)=parsed(uri)?;
    let all=exports(&host)?;
    // Zeroconf advertises /mnt as a service root even though /mnt is NOT itself an export.
    if path=="/" || path=="/mnt" || path=="/exports" {
        let result=all.into_iter().map(|path|Entry{name:path.rsplit('/').next().unwrap_or(&path).to_owned(),
            uri:format!("nfs://{host}{path}"),is_dir:true,server:String::new()}).collect::<Vec<_>>();
        crate::network_shares::trace("PRIVATE_NFS",format!("exports_display host={host} advertised_path={path} count={}",result.len()));
        return Ok(result)
    }
    let (export,relative)=resolve(&host,&path)?;
    crate::network_shares::trace("PRIVATE_NFS",format!("list_start host={host} export={export} relative={relative}"));
    let mut results: Vec<(String,u32)> = Vec::new();let mut error=[0 as c_char;512];
    let h=CString::new(host)?;let e=CString::new(export)?;let r=CString::new(relative)?;
    let count=unsafe{pic_nfs_list(h.as_ptr(),e.as_ptr(),r.as_ptr(),names,
        &mut results as *mut _ as *mut c_void,error.as_mut_ptr(),error.len())};
    if count<0{anyhow::bail!("{}",err(&error))}
    let entries=children(uri,results);
    crate::network_shares::trace("PRIVATE_NFS",format!("list_done raw={count} displayed={}",entries.len()));
    Ok(entries)
}
pub fn read(uri:&str)->anyhow::Result<Vec<u8>>{
    const LIMIT:usize=128*1024*1024;
    let (host,path)=parsed(uri)?;let (export,relative)=resolve(&host,&path)?;
    let (h,e,r)=(CString::new(host)?,CString::new(export)?,CString::new(relative)?);
    let mut buffer=[0 as c_char;512];let mut bytes: *mut u8=std::ptr::null_mut();let mut len=0usize;
    crate::network_shares::trace("PRIVATE_NFS",format!("read_start uri={uri} max_bytes={LIMIT}"));
    let start=std::time::Instant::now();
    let status=unsafe{pic_nfs_read(h.as_ptr(),e.as_ptr(),r.as_ptr(),&mut bytes,&mut len,LIMIT,buffer.as_mut_ptr(),buffer.len())};
    if status<0 {
        let detail=err(&buffer);
        crate::network_shares::trace("PRIVATE_NFS",format!("read_failed uri={uri} elapsed_ms={} error={detail}",start.elapsed().as_millis()));
        anyhow::bail!("{detail}")
    }
    let result=if len==0{Vec::new()}else{unsafe{std::slice::from_raw_parts(bytes,len).to_vec()}};
    unsafe{pic_smb_free(bytes as *mut c_void)};
    crate::network_shares::trace("PRIVATE_NFS",format!("read_done bytes={} elapsed_ms={} uri={uri}",result.len(),start.elapsed().as_millis()));
    Ok(result)
}
pub fn read_range(uri:&str,offset:u64,requested:usize)->anyhow::Result<Vec<u8>>{
    anyhow::ensure!(requested<=100*1024*1024,"NFS range exceeds preview safety limit");
    let(host,path)=parsed(uri)?;let(export,relative)=resolve(&host,&path)?;let(h,e,r)=(CString::new(host)?,CString::new(export)?,CString::new(relative)?);
    let mut error=[0 as c_char;512];let mut bytes=std::ptr::null_mut();let mut len=0usize;
    if unsafe{pic_nfs_read_range(h.as_ptr(),e.as_ptr(),r.as_ptr(),offset,requested,&mut bytes,&mut len,error.as_mut_ptr(),error.len())}<0 {anyhow::bail!("{}",err(&error));}
    let result=if len==0{Vec::new()}else{unsafe{std::slice::from_raw_parts(bytes,len).to_vec()}};unsafe{pic_smb_free(bytes as *mut c_void)};
    crate::network_shares::trace("PRIVATE_NFS",format!("range offset={offset} requested={requested} received={}",result.len()));Ok(result)
}
#[cfg(test)]mod tests{use super::*;
    #[test]fn uri_encoding(){assert_eq!(parsed("nfs://DietPi.local:2049/mnt/4TBP/Some%20Photos").unwrap(),("DietPi.local".into(),"/mnt/4TBP/Some Photos".into()));}
    #[test]fn reject_parent(){assert!(parsed("nfs://dietpi.local/mnt/4TBP/../etc").is_err());}
}

pub fn stat(uri: &str) -> anyhow::Result<crate::network_shares::Metadata> {
    let (host,path)=parsed(uri)?;
    // The discovery pseudo-root is a virtual directory, not a mountable export.
    if path=="/" || path=="/mnt" || path=="/exports" {
        return Ok(crate::network_shares::Metadata{size:0,mtime:None,is_dir:true});
    }
    let (export,relative)=resolve(&host,&path)?;
    let (h,e,r)=(CString::new(host)?,CString::new(export)?,CString::new(relative)?);
    let mut size=0u64;let mut mtime=0i64;let mut dir=0i32;
    let mut buffer=[0 as c_char;512];
    let status=unsafe{pic_nfs_stat(h.as_ptr(),e.as_ptr(),r.as_ptr(),&mut size,&mut mtime,&mut dir,buffer.as_mut_ptr(),buffer.len())};
    if status<0 {anyhow::bail!("{}",err(&buffer));}
    Ok(crate::network_shares::Metadata{size,mtime:Some(mtime),is_dir:dir!=0})
}
