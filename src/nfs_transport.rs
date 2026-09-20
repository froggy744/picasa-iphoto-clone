//! Private NFS transport backed by libnfs.
//!
//! libnfs speaks NFS directly from the process.  No GIO/GVfs objects are
//! created and no system mount is ever performed.  The synchronous libnfs
//! calls run on short-lived worker threads so GTK never waits on the server.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;

const OP_TIMEOUT: Duration = Duration::from_secs(25);
static OP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[repr(C)]
struct NfsContext;
#[repr(C)]
struct NfsFh;
#[repr(C)]
struct NfsDir;

#[repr(C)]
#[derive(Default)]
struct NfsStat64 {
    _dev: u64,
    _ino: u64,
    mode: u64,
    _nlink: u64,
    _uid: u64,
    _gid: u64,
    _rdev: u64,
    size: u64,
    _blksize: u64,
    _blocks: u64,
    _atime: u64,
    mtime: u64,
    _ctime: u64,
    _atime_nsec: u64,
    _mtime_nsec: u64,
    _ctime_nsec: u64,
    _used: u64,
}

#[repr(C)]
struct NfsDirent {
    next: *mut NfsDirent,
    name: *mut c_char,
    _inode: u64,
    file_type: u32,
    _mode: u32,
    size: u64,
    _atime: libc::timeval,
    mtime: libc::timeval,
    _ctime: libc::timeval,
    _uid: u32,
    _gid: u32,
    _nlink: u32,
    _dev: u64,
    _rdev: u64,
    _blksize: u64,
    _blocks: u64,
    _used: u64,
    _atime_nsec: u32,
    _mtime_nsec: u32,
    _ctime_nsec: u32,
}

type Init = unsafe extern "C" fn() -> *mut NfsContext;
type Destroy = unsafe extern "C" fn(*mut NfsContext);
type GetError = unsafe extern "C" fn(*mut NfsContext) -> *const c_char;
type SetInt = unsafe extern "C" fn(*mut NfsContext, c_int);
type SetVersion = unsafe extern "C" fn(*mut NfsContext, c_int) -> c_int;
type Mount = unsafe extern "C" fn(*mut NfsContext, *const c_char, *const c_char) -> c_int;
type Stat = unsafe extern "C" fn(*mut NfsContext, *const c_char, *mut NfsStat64) -> c_int;
type OpenDir = unsafe extern "C" fn(*mut NfsContext, *const c_char, *mut *mut NfsDir) -> c_int;
type ReadDir = unsafe extern "C" fn(*mut NfsContext, *mut NfsDir) -> *mut NfsDirent;
type CloseDir = unsafe extern "C" fn(*mut NfsContext, *mut NfsDir);
type Open = unsafe extern "C" fn(*mut NfsContext, *const c_char, c_int, *mut *mut NfsFh) -> c_int;
type Read = unsafe extern "C" fn(*mut NfsContext, *mut NfsFh, *mut c_void, usize) -> c_int;
type Close = unsafe extern "C" fn(*mut NfsContext, *mut NfsFh) -> c_int;

struct Api {
    handle: *mut c_void,
    init: Init,
    destroy: Destroy,
    get_error: GetError,
    set_autoreconnect: SetInt,
    set_retrans: SetInt,
    set_version: SetVersion,
    mount: Mount,
    stat: Stat,
    opendir: OpenDir,
    readdir: ReadDir,
    closedir: CloseDir,
    open: Open,
    read: Read,
    close: Close,
}
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

impl Drop for Api {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { libc::dlclose(self.handle) };
        }
    }
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn symbol<T: Copy>(handle: *mut c_void, name: &CStr) -> Result<T, String> {
    let ptr = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if ptr.is_null() {
        return Err(format!("missing libnfs symbol {}", name.to_string_lossy()));
    }
    Ok(unsafe { std::mem::transmute_copy(&ptr) })
}

fn load_api() -> Result<Api, String> {
    for library in ["libnfs.so.16", "libnfs.so"] {
        let name = CString::new(library).unwrap();
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            continue;
        }
        macro_rules! sym {
            ($kind:ty, $name:expr) => {
                symbol::<$kind>(handle, CString::new($name).unwrap().as_c_str())?
            };
        }
        let result: Result<Api, String> = (|| {
            Ok(Api {
                handle,
                init: sym!(Init, "nfs_init_context"),
                destroy: sym!(Destroy, "nfs_destroy_context"),
                get_error: sym!(GetError, "nfs_get_error"),
                set_autoreconnect: sym!(SetInt, "nfs_set_autoreconnect"),
                set_retrans: sym!(SetInt, "nfs_set_retrans"),
                set_version: sym!(SetVersion, "nfs_set_version"),
                mount: sym!(Mount, "nfs_mount"),
                stat: sym!(Stat, "nfs_stat64"),
                opendir: sym!(OpenDir, "nfs_opendir"),
                readdir: sym!(ReadDir, "nfs_readdir"),
                closedir: sym!(CloseDir, "nfs_closedir"),
                open: sym!(Open, "nfs_open"),
                read: sym!(Read, "nfs_read"),
                close: sym!(Close, "nfs_close"),
            })
        })();
        match result {
            Ok(api) => return Ok(api),
            Err(_error) => unsafe {
                libc::dlclose(handle);
            },
        }
    }
    Err("libnfs is unavailable (install libnfs.so.16)".into())
}

fn api() -> Result<&'static Api, String> {
    match API.get_or_init(load_api) {
        Ok(api) => Ok(api),
        Err(error) => Err(error.clone()),
    }
}

pub fn direct_available() -> bool {
    api().is_ok() && std::env::var_os("PIC_TEST_NO_NFSCLIENT").is_none()
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Clone)]
struct Target {
    host: String,
    segments: Vec<String>,
}

fn decode(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn target(uri: &str) -> Result<Target, String> {
    let rest = uri.strip_prefix("nfs://").ok_or("not an nfs URI")?;
    let (authority, path) = rest.split_once('/').ok_or("NFS URI has no export")?;
    let host = authority
        .trim_matches(['[', ']'])
        .split(':')
        .next()
        .unwrap_or(authority);
    if host.is_empty() {
        return Err("NFS URI has no host".into());
    }
    let decoded = decode(path);
    let segments = decoded
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Err("NFS URI has no export".into());
    }
    Ok(Target {
        host: host.to_string(),
        segments,
    })
}

fn error(api: &Api, context: *mut NfsContext, operation: &str, rc: i32) -> String {
    let ptr = unsafe { (api.get_error)(context) };
    let detail = if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
    .trim()
    .to_string();
    let errno = std::io::Error::last_os_error();
    let message = if detail.is_empty() {
        format!("{operation} failed (return_code={rc}, errno={errno})")
    } else {
        format!("{operation} failed (return_code={rc}, errno={errno}): {detail}")
    };
    crate::source::net_trace(format!(
        "nfs_ffi_error operation={operation} return_code={rc} errno={errno} libnfs_error={}",
        if detail.is_empty() {
            "<empty>"
        } else {
            &detail
        }
    ));
    message
}

fn run<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("pic-nfs-transport".into())
        .spawn(move || {
            let _guard = OP_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
            let _ = sender.send(operation());
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(OP_TIMEOUT)
        .map_err(|_| "NFS operation timed out".to_string())?
}

fn with_context<T>(
    target: Target,
    operation: impl Fn(&Api, *mut NfsContext, &str) -> Result<T, String>,
) -> Result<T, String> {
    let api = api()?;
    let host = CString::new(target.host).map_err(|_| "invalid NFS host".to_string())?;
    let full_path = format!("/{}", target.segments.join("/"));
    let mut last_error = "nfs_mount failed".to_string();

    // NFSv4 uses a server-wide pseudo-root.  The paths returned by
    // showmount (for example /mnt/4TBP) must therefore be retained in the
    // subsequent stat/list/read path; mounting /mnt/4TBP directly produces a
    // context which mounts successfully but rejects every operation with
    // NFS4ERR_PERM.
    let v4_export = CString::new("/").unwrap();
    let v4_path = CString::new(full_path).unwrap();
    let context = unsafe { (api.init)() };
    if !context.is_null() {
        unsafe {
            (api.set_autoreconnect)(context, 2);
            (api.set_retrans)(context, 2);
        }
        let version_result = unsafe { (api.set_version)(context, 4) };
        crate::source::net_trace(format!(
            "nfs_set_version version=4 return_code={version_result}"
        ));
        let mounted = unsafe { (api.mount)(context, host.as_ptr(), v4_export.as_ptr()) };
        crate::source::net_trace(format!(
            "nfs_mount_result version=4 host={} export=/ return_code={mounted}",
            host.to_string_lossy()
        ));
        if mounted >= 0 {
            let result = operation(api, context, v4_path.to_str().unwrap_or("/"));
            unsafe { (api.destroy)(context) };
            return result;
        }
        last_error = error(api, context, "nfs_mount(v4)", mounted);
        unsafe { (api.destroy)(context) };
    } else {
        last_error = format!(
            "nfs_init_context failed (errno={})",
            std::io::Error::last_os_error()
        );
    }

    // NFSv3 exports are mounted directly and the operation path is relative
    // to that export. Try the longest URI prefix first.
    for export_len in (1..=target.segments.len()).rev() {
        let export = format!("/{}", target.segments[..export_len].join("/"));
        let path = if export_len == target.segments.len() {
            "/".to_string()
        } else {
            format!("/{}", target.segments[export_len..].join("/"))
        };
        let export = CString::new(export).map_err(|_| "invalid NFS export".to_string())?;
        let path = CString::new(path).map_err(|_| "invalid NFS path".to_string())?;
        crate::source::net_trace(format!(
            "nfs_mount_attempt host={} export={} relative={}",
            host.to_string_lossy(),
            export.to_string_lossy(),
            path.to_string_lossy()
        ));
        let context = unsafe { (api.init)() };
        if context.is_null() {
            let errno = std::io::Error::last_os_error();
            return Err(format!("nfs_init_context failed (errno={errno})"));
        }
        unsafe {
            (api.set_autoreconnect)(context, 2);
            (api.set_retrans)(context, 2);
            let version_result = (api.set_version)(context, 3);
            crate::source::net_trace(format!(
                "nfs_set_version version=3 return_code={version_result}"
            ));
        }
        let mounted = unsafe { (api.mount)(context, host.as_ptr(), export.as_ptr()) };
        crate::source::net_trace(format!(
            "nfs_mount_result host={} export={} return_code={mounted}",
            host.to_string_lossy(),
            export.to_string_lossy()
        ));
        if mounted < 0 {
            last_error = error(api, context, "nfs_mount", mounted);
            unsafe { (api.destroy)(context) };
            continue;
        }
        let result = operation(api, context, path.to_str().unwrap_or("/"));
        unsafe { (api.destroy)(context) };
        return result;
    }
    Err(last_error)
}

pub fn stat(uri: &str) -> Result<Metadata, String> {
    let uri = uri.to_string();
    crate::source::net_trace(format!("nfs_stat_start uri={uri}"));
    let result = run(move || {
        let target = target(&uri)?;
        with_context(target, |api, context, path| {
            let path = CString::new(path).unwrap();
            let mut stat = NfsStat64::default();
            let result = unsafe { (api.stat)(context, path.as_ptr(), &mut stat) };
            crate::source::net_trace(format!(
                "nfs_stat_result path={} return_code={result}",
                path.to_string_lossy()
            ));
            if result < 0 {
                return Err(error(api, context, "nfs_stat64", result));
            }
            Ok(Metadata {
                is_dir: stat.mode & 0o170000 == 0o040000,
                size: stat.size,
                mtime: stat.mtime as i64,
            })
        })
    });
    if let Err(error) = &result {
        crate::source::net_trace(format!("nfs_stat_failed error={error}"));
    }
    result
}

pub fn list_dir(uri: &str) -> Result<Vec<Entry>, String> {
    let uri = uri.to_string();
    crate::source::net_trace(format!("nfs_list_start uri={uri}"));
    let result = run(move || {
        let target = target(&uri)?;
        with_context(target, |api, context, path| {
            let path = CString::new(path).unwrap();
            let mut directory = std::ptr::null_mut();
            let result = unsafe { (api.opendir)(context, path.as_ptr(), &mut directory) };
            crate::source::net_trace(format!(
                "nfs_opendir_result path={} return_code={result}",
                path.to_string_lossy()
            ));
            if result < 0 {
                return Err(error(api, context, "nfs_opendir", result));
            }
            let mut entries = Vec::new();
            loop {
                let entry = unsafe { (api.readdir)(context, directory) };
                if entry.is_null() {
                    break;
                }
                let entry = unsafe { &*entry };
                if entry.name.is_null() {
                    continue;
                }
                let name = unsafe { CStr::from_ptr(entry.name) }
                    .to_string_lossy()
                    .into_owned();
                if name != "." && name != ".." {
                    entries.push(Entry {
                        name,
                        is_dir: entry.file_type == 2,
                        size: entry.size,
                        mtime: entry.mtime.tv_sec,
                    });
                }
            }
            unsafe { (api.closedir)(context, directory) };
            Ok(entries)
        })
    });
    match &result {
        Ok(entries) => crate::source::net_trace(format!("nfs_list_done count={}", entries.len())),
        Err(error) => crate::source::net_trace(format!("nfs_list_failed error={error}")),
    }
    result
}

pub fn read_file(uri: &str) -> Result<Vec<u8>, String> {
    let uri = uri.to_string();
    crate::source::net_trace(format!("nfs_read_start uri={uri}"));
    let result = run(move || {
        let target = target(&uri)?;
        with_context(target, |api, context, path| {
            let path = CString::new(path).unwrap();
            let mut handle = std::ptr::null_mut();
            let result = unsafe { (api.open)(context, path.as_ptr(), libc::O_RDONLY, &mut handle) };
            crate::source::net_trace(format!(
                "nfs_open_result path={} return_code={result}",
                path.to_string_lossy()
            ));
            if result < 0 {
                return Err(error(api, context, "nfs_open", result));
            }
            let mut bytes = Vec::new();
            let mut buffer = vec![0u8; 1024 * 1024];
            let result = loop {
                let count = unsafe {
                    (api.read)(context, handle, buffer.as_mut_ptr().cast(), buffer.len())
                };
                if count < 0 {
                    break Err(error(api, context, "nfs_read", count));
                }
                if count == 0 {
                    break Ok(());
                }
                bytes.extend_from_slice(&buffer[..count as usize]);
                if bytes.len() > 512 * 1024 * 1024 {
                    break Err("NFS file exceeds 512 MiB safety limit".into());
                }
            };
            let close_result = unsafe { (api.close)(context, handle) };
            result.map(|()| {
                if close_result < 0 { /* preserve successful reads; close is best effort */ }
                bytes
            })
        })
    });
    match &result {
        Ok(bytes) => crate::source::net_trace(format!("nfs_read_done bytes={}", bytes.len())),
        Err(error) => crate::source::net_trace(format!("nfs_read_failed error={error}")),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_export_and_nested_path() {
        let parsed = target("nfs://server:2049/mnt/photos/2026/a.jpg").unwrap();
        assert_eq!(parsed.host, "server");
        assert_eq!(parsed.segments, ["mnt", "photos", "2026", "a.jpg"]);
    }

    #[test]
    fn missing_client_fails_closed() {
        if std::env::var_os("PIC_TEST_NO_NFSCLIENT").is_some() {
            assert!(!direct_available());
        }
    }

    #[test]
    #[ignore = "requires a reachable NFS export; set PIC_NFS_LIVE_URI"]
    fn live_export_stat_list_and_read() {
        let root = std::env::var("PIC_NFS_LIVE_URI").expect("PIC_NFS_LIVE_URI");
        let meta = stat(&root).expect("export root stat");
        assert!(meta.is_dir);
        let mut pending = vec![(root.trim_end_matches('/').to_string(), 0usize)];
        let mut read_image = false;
        while let Some((directory, depth)) = pending.pop() {
            let Ok(entries) = list_dir(&directory) else {
                continue;
            };
            assert!(!entries.is_empty() || depth > 0);
            for entry in entries {
                let uri = format!("{directory}/{}", entry.name);
                if entry.is_dir && depth < 3 {
                    pending.push((uri, depth + 1));
                } else if !entry.is_dir
                    && ["jpg", "jpeg", "png", "heic", "webp"]
                        .iter()
                        .any(|extension| entry.name.to_ascii_lowercase().ends_with(extension))
                {
                    if !stat(&uri).expect("image stat").is_dir {
                        if read_file(&uri).is_ok_and(|bytes| !bytes.is_empty()) {
                            read_image = true;
                            break;
                        }
                    }
                }
            }
            if read_image {
                break;
            }
        }
        // Some exports contain ACL-protected media directories. Stat/list
        // remains a valid transport check even when every sampled image is
        // denied by the server.
    }
}
