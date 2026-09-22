//! Direct SMB access through libsmbclient, loaded at runtime with dlopen.
//!
//! This is the foundation of the "REDESIGN SHARES MOUNT" transport: PIC reads
//! SMB servers directly, so the automatic reconnect path never registers a
//! gvfs mount and nothing appears in Nautilus. Existing desktop mounts are
//! never used or modified by this transport.
//!
//! Design notes:
//! - The library is loaded from `libsmbclient.so.0` (runtime Samba library);
//!   no `-devel` package is needed to build. If it is missing, every operation
//!   fails with `SmbTransportError::Unavailable` and the caller reports the
//!   share as offline - there is deliberately NO gvfs fallback.
//! - One SMBCCTX per operation, created and destroyed on a dedicated worker
//!   thread (SMBCCTX is not thread-safe). Credentials for the share are bound
//!   to that thread so the auth callback can supply them; nothing global
//!   leaks between shares.
//! - `SMB_SINGLE_FLIGHT` serializes all transport operations process-wide and
//!   `SMB_WORKER_IN_FLIGHT` guarantees that a timed-out operation cannot
//!   overlap a new one: a timeout is never merely hidden, the worker must
//!   actually finish (bounded by the Samba context timeout) before further
//!   SMB work is allowed.
//! - Credentials stay in process memory and Secret Service, never in SQLite.
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Result of listing a directory.
#[derive(Debug, Clone)]
pub struct SmbEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Result of stat-ing a path.
#[derive(Debug, Clone)]
pub struct SmbFileMeta {
    pub size: u64,
    pub is_dir: bool,
    pub mtime: Option<i64>,
    /// Server-root probes only: the shares the server advertises.
    pub shares: Vec<String>,
}

impl SmbFileMeta {
    fn file(size: u64, is_dir: bool, mtime: Option<i64>) -> Self {
        Self {
            size,
            is_dir,
            mtime,
            shares: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum SmbTransportError {
    /// The URI is not an SMB URI.
    NotSmb,
    /// The URI has no share component; a server root is not readable over SMB.
    NoShare,
    /// libsmbclient.so.0 is not available on this system.
    Unavailable(String),
    /// The operation exceeded the caller's timeout.
    Timeout,
    /// Every credential in the ladder was rejected by the server. The share
    /// needs an interactive login (Retry Connection).
    AuthRequired(String),
    /// Any other failure, carrying the message.
    Failed(String),
}

impl std::fmt::Display for SmbTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSmb => write!(f, "not an smb:// URI"),
            Self::NoShare => write!(f, "the URI has no share name"),
            Self::Unavailable(detail) => {
                write!(f, "libsmbclient is not available: {detail}")
            }
            Self::Timeout => write!(f, "the SMB operation timed out"),
            Self::AuthRequired(detail) => {
                write!(f, "authentication is required for {detail}")
            }
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// A parsed `smb://host[:port][/share[/path]]` URI with percent-escapes
/// decoded. The stored library URIs keep their exact form; only the transport
/// decodes them for Samba.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbTarget {
    pub host: String,
    pub share: String,
    pub path: String,
}

pub fn parse_smb_uri(uri: &str) -> Option<SmbTarget> {
    let rest = uri.strip_prefix("smb://")?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        None => (rest, ""),
    };
    // Drop userinfo (`user:pass@`) if present; credentials are supplied
    // through the auth callback, never through the URL.
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    // Strip a numeric :port; SMB is 445-only and smbc URLs do not take ports.
    let host = if authority.starts_with('[') {
        authority.to_string()
    } else {
        match authority.rsplit_once(':') {
            Some((host, port))
                if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                host.to_string()
            }
            _ => authority.to_string(),
        }
    };
    if host.is_empty() {
        return None;
    }
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    // A share-less URI (server root) parses fine; operations reject it with
    // `NoShare` because smbc cannot address files without a share.
    let share = segments.next().and_then(decode_segment).unwrap_or_default();
    let decoded_path: Vec<String> = segments
        .map(|segment| decode_segment(segment).unwrap_or_default())
        .filter(|segment| !segment.is_empty())
        .collect();
    Some(SmbTarget {
        host,
        share,
        path: decoded_path.join("/"),
    })
}

fn decode_segment(segment: &str) -> Option<String> {
    glib::uri_unescape_string(segment, None::<&str>)
        .map(|decoded| decoded.to_string())
        .or_else(|| Some(segment.to_string()))
}

/// Percent-encode a single path segment for storage in PIC's canonical
/// `smb://` URIs (same alphabet gvfs/gio uses).
pub fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Rewrite legacy gvfs-FUSE photo paths into canonical `smb://` URIs.
///
/// Photographs imported before the direct transport were stored as
/// `/run/user/UID/gvfs/smb-share:server=HOST,share=SHARE/path...`. Those
/// paths only resolve while a gvfs mount exists - the transport mounts
/// nothing, so reads of such records failed instantly (Fedora v17: 19
/// lightbox read failures, every one on a legacy FUSE path). Normalizing at
/// the read/probe boundary restores access without touching library
/// records. Non-FUSE references (smb://, nfs://, local paths) pass through
/// unchanged; source::normalize_import_reference handles NFS FUSE paths.
pub fn normalize_smb_reference(reference: &str) -> String {
    if reference.starts_with("smb://") {
        return reference.to_string();
    }
    let Some(marker) = reference.find("/gvfs/smb-share:") else {
        return reference.to_string();
    };
    let options_and_path = &reference[marker + "/gvfs/smb-share:".len()..];
    let (options, tail) = match options_and_path.split_once('/') {
        Some((options, tail)) => (options, tail),
        None => (options_and_path, ""),
    };
    let mut server: Option<&str> = None;
    let mut share: Option<&str> = None;
    for option in options.split(',') {
        if let Some(host) = option.strip_prefix("server=") {
            server = Some(host);
        } else if let Some(share_name) = option.strip_prefix("share=") {
            share = Some(share_name);
        }
    }
    let (Some(server), Some(share)) = (server, share) else {
        return reference.to_string();
    };
    // Drop a numeric :port (SMB is 445-only; smbc URLs take no port) and any
    // userinfo-style components Samba may have included.
    let server = match server.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            host
        }
        _ => server,
    };
    if server.is_empty() || share.is_empty() {
        return reference.to_string();
    }
    let mut url = format!(
        "smb://{server}/{}",
        percent_encode_segment(&unescape_gvfs(share))
    );
    for segment in tail.split('/').filter(|segment| !segment.is_empty()) {
        url.push('/');
        url.push_str(&percent_encode_segment(&unescape_gvfs(segment)));
    }
    url
}

/// gvfs percent-encodes option values (e.g. `prefix=%2Fexports%2FWork`);
/// decode so the value can be re-encoded per segment.
fn unescape_gvfs(value: &str) -> String {
    glib::uri_unescape_string(value, None::<&str>)
        .map(|decoded| decoded.to_string())
        .unwrap_or_else(|| value.to_string())
}

impl SmbTarget {
    /// smbc URL for a path inside the share. An empty path is the share root.
    fn smbc_url(&self) -> String {
        let mut url = format!("smb://{}/{}", self.host, self.share);
        if !self.path.is_empty() {
            url.push('/');
            url.push_str(&self.path);
        } else {
            url.push('/');
        }
        url
    }
}

// ---------------------------------------------------------------------------
// FFI: types mirroring libsmbclient.h (verified against pavao-sys 0.2.16).
// ---------------------------------------------------------------------------

type SmbCtx = *mut libc::c_void;
type SmbFileHandle = *mut libc::c_void;

/// `struct smbc_dirent` as of Samba 4.x (mirrors pavao-sys 0.2.16): the
/// name is an inline array standing in for the variable-length tail, and
/// there is no `size` member - file sizes come from `stat`.
#[repr(C)]
#[derive(Clone, Copy)]
struct SmbDirent {
    smbc_type: c_uint,
    dirlen: c_uint,
    commentlen: c_uint,
    comment: *mut c_char,
    namelen: c_uint,
    name: [c_char; 1024],
}

const SMBC_TYPE_FILE_SHARE: c_uint = 3;
const SMBC_TYPE_DIR: c_uint = 7;
const SMBC_TYPE_FILE: c_uint = 8;

type AuthDataWithContextFn = extern "C" fn(
    ctx: SmbCtx,
    srv: *const c_char,
    shr: *const c_char,
    wg: *mut c_char,
    wglen: c_int,
    un: *mut c_char,
    unlen: c_int,
    pw: *mut c_char,
    pwlen: c_int,
);
type OpenDirFn = extern "C" fn(ctx: SmbCtx, fname: *const c_char) -> SmbFileHandle;
type ReadDirFn = extern "C" fn(ctx: SmbCtx, dir: SmbFileHandle) -> *mut SmbDirent;
type CloseDirFn = extern "C" fn(ctx: SmbCtx, dir: SmbFileHandle) -> c_int;
type OpenFn = extern "C" fn(
    ctx: SmbCtx,
    fname: *const c_char,
    flags: c_int,
    mode: libc::mode_t,
) -> SmbFileHandle;
type ReadFn =
    extern "C" fn(ctx: SmbCtx, file: SmbFileHandle, buf: *mut libc::c_void, count: usize) -> isize;
type CloseFn = extern "C" fn(ctx: SmbCtx, file: SmbFileHandle) -> c_int;
type StatFn = extern "C" fn(ctx: SmbCtx, url: *const c_char, st: *mut libc::stat) -> c_int;

struct SmbApi {
    new_context: unsafe extern "C" fn() -> SmbCtx,
    init_context: unsafe extern "C" fn(SmbCtx) -> SmbCtx,
    free_context: unsafe extern "C" fn(SmbCtx, c_int),
    set_auth_data: unsafe extern "C" fn(SmbCtx, Option<AuthDataWithContextFn>),
    set_timeout: unsafe extern "C" fn(SmbCtx, c_int),
    set_user: unsafe extern "C" fn(SmbCtx, *const c_char),
    // `smbc_getFunction*` are GETTERS: they take the context and return the
    // callable function pointer for that context. They are resolved once per
    // client after `smbc_init_context`.
    get_opendir: unsafe extern "C" fn(SmbCtx) -> Option<OpenDirFn>,
    get_readdir: unsafe extern "C" fn(SmbCtx) -> Option<ReadDirFn>,
    get_closedir: unsafe extern "C" fn(SmbCtx) -> Option<CloseDirFn>,
    get_open: unsafe extern "C" fn(SmbCtx) -> Option<OpenFn>,
    get_read: unsafe extern "C" fn(SmbCtx) -> Option<ReadFn>,
    get_close: unsafe extern "C" fn(SmbCtx) -> Option<CloseFn>,
    get_stat: unsafe extern "C" fn(SmbCtx) -> Option<StatFn>,
}

/// Credentials handed to the auth callback. Bound per worker thread.
#[derive(Debug, Clone)]
pub struct SmbCredentials {
    pub user: String,
    pub password: String,
}

/// Internal operation failure with its cause classification: `auth` failures
/// are worth retrying with different credentials, everything else is not.
#[derive(Debug)]
struct SmbOpError {
    message: String,
    auth: bool,
}

impl SmbOpError {
    fn of(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            auth: last_call_was_auth_failure(),
        }
    }

    fn cancelled() -> Self {
        Self {
            message: String::from("cancelled"),
            auth: false,
        }
    }
}

thread_local! {
    static THREAD_CREDENTIALS: RefCell<Option<SmbCredentials>> = const { RefCell::new(None) };
}

/// Credentials for the operation running on the CURRENT thread; the auth
/// callback reads them when the server asks.
extern "C" fn auth_callback(
    _ctx: SmbCtx,
    _server: *const c_char,
    _share: *const c_char,
    workgroup: *mut c_char,
    workgroup_len: c_int,
    username: *mut c_char,
    username_len: c_int,
    password: *mut c_char,
    password_len: c_int,
) {
    let credentials = THREAD_CREDENTIALS
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| os_default_credentials());
    unsafe {
        fill_buffer("WORKGROUP", workgroup, workgroup_len);
        fill_buffer(&credentials.user, username, username_len);
        fill_buffer(&credentials.password, password, password_len);
    }
}

/// Write `value` NUL-terminated into `buffer` without exceeding `capacity`.
unsafe fn fill_buffer(value: &str, buffer: *mut c_char, capacity: c_int) {
    if buffer.is_null() || capacity <= 0 {
        return;
    }
    let bytes = value.as_bytes();
    let writable = (capacity as usize).saturating_sub(1);
    let count = bytes.len().min(writable);
    std::ptr::copy_nonoverlapping(bytes.as_ptr().cast::<c_char>(), buffer, count);
    buffer.add(count).write(0);
}

// ---------------------------------------------------------------------------
// Library loading (dlopen) and the per-operation client.
// ---------------------------------------------------------------------------

static API: OnceLock<Result<SmbApi, String>> = OnceLock::new();

fn api() -> Result<&'static SmbApi, SmbTransportError> {
    #[cfg(test)]
    if std::env::var_os("PIC_TEST_NO_SMBCLIENT").is_some() {
        return Err(SmbTransportError::Unavailable(
            "disabled in isolated test process".into(),
        ));
    }
    let loaded = API.get_or_init(|| unsafe { load_smbclient() });
    loaded
        .as_ref()
        .map_err(|detail| SmbTransportError::Unavailable(detail.clone()))
}

unsafe fn load_smbclient() -> Result<SmbApi, String> {
    let handle = libc::dlopen(
        b"libsmbclient.so.0\0".as_ptr().cast::<c_char>(),
        libc::RTLD_NOW | libc::RTLD_LOCAL,
    );
    if handle.is_null() {
        return Err(String::from(
            "libsmbclient.so.0 could not be loaded (install the samba client library)",
        ));
    }
    let symbol = |name: &'static [u8]| -> *mut libc::c_void {
        libc::dlsym(handle, name.as_ptr().cast::<c_char>())
    };
    let load = |name: &'static [u8]| -> Result<*mut libc::c_void, String> {
        let pointer = symbol(name);
        if pointer.is_null() {
            return Err(format!(
                "symbol {} missing in libsmbclient",
                String::from_utf8_lossy(name)
            ));
        }
        Ok(pointer)
    };
    let transmute_fn = |name: &'static [u8]| -> Result<*mut libc::c_void, String> { load(name) };
    macro_rules! require {
        ($name:expr) => {
            transmute_fn($name)?
        };
    }
    Ok(SmbApi {
        new_context: std::mem::transmute(require!(b"smbc_new_context\0")),
        init_context: std::mem::transmute(require!(b"smbc_init_context\0")),
        free_context: std::mem::transmute(require!(b"smbc_free_context\0")),
        set_auth_data: std::mem::transmute(require!(b"smbc_setFunctionAuthDataWithContext\0")),
        set_timeout: std::mem::transmute(require!(b"smbc_setTimeout\0")),
        set_user: std::mem::transmute(require!(b"smbc_setUser\0")),
        get_opendir: std::mem::transmute(require!(b"smbc_getFunctionOpendir\0")),
        get_readdir: std::mem::transmute(require!(b"smbc_getFunctionReaddir\0")),
        get_closedir: std::mem::transmute(require!(b"smbc_getFunctionClosedir\0")),
        get_open: std::mem::transmute(require!(b"smbc_getFunctionOpen\0")),
        get_read: std::mem::transmute(require!(b"smbc_getFunctionRead\0")),
        get_close: std::mem::transmute(require!(b"smbc_getFunctionClose\0")),
        get_stat: std::mem::transmute(require!(b"smbc_getFunctionStat\0")),
    })
}

/// A context plus the callables resolved from it, valid for the current
/// worker thread only.
struct SmbClient {
    ctx: SmbCtx,
    opendir: OpenDirFn,
    readdir: ReadDirFn,
    closedir: CloseDirFn,
    open: OpenFn,
    read: ReadFn,
    close: CloseFn,
    stat: StatFn,
}

impl SmbClient {
    /// Samba context timeout in milliseconds (smbc_setTimeout takes
    /// milliseconds - the same value smb.conf's `timeout` uses).
    const CONTEXT_TIMEOUT_MS: c_int = 10_000;

    fn new(credentials: &SmbCredentials) -> Result<Self, String> {
        let api = api().map_err(|error| error.to_string())?;
        crate::source::net_trace("smb_client_new_start");
        unsafe {
            let ctx = (api.new_context)();
            if ctx.is_null() {
                return Err(String::from("smbc_new_context failed"));
            }
            crate::source::net_trace("smb_client_context_created");
            THREAD_CREDENTIALS.with(|slot| {
                *slot.borrow_mut() = Some(credentials.clone());
            });
            (api.set_auth_data)(ctx, Some(auth_callback));
            (api.set_timeout)(ctx, Self::CONTEXT_TIMEOUT_MS);
            if !credentials.user.is_empty() {
                let user = CString::new(credentials.user.as_str())
                    .map_err(|_| String::from("invalid username"))?;
                (api.set_user)(ctx, user.as_ptr());
            }
            crate::source::net_trace("smb_client_init_start");
            let initialized = (api.init_context)(ctx);
            crate::source::net_trace("smb_client_init_done");
            if initialized.is_null() {
                (api.free_context)(ctx, 1);
                return Err(String::from("smbc_init_context failed"));
            }
            // Resolve the real callables from the initialized context. Each
            // getter returns None only when the context was not initialized.
            // On a resolution failure the context is freed before returning.
            let mut unfreed = Some(initialized);
            macro_rules! resolve_fn {
                ($getter:expr, $name:literal) => {
                    match ($getter)(initialized) {
                        Some(function) => function,
                        None => {
                            if let Some(ctx) = unfreed.take() {
                                (api.free_context)(ctx, 1);
                            }
                            return Err(format!("libsmbclient context is missing {}", $name));
                        }
                    }
                };
            }
            Ok(Self {
                ctx: initialized,
                opendir: resolve_fn!(api.get_opendir, "opendir"),
                readdir: resolve_fn!(api.get_readdir, "readdir"),
                closedir: resolve_fn!(api.get_closedir, "closedir"),
                open: resolve_fn!(api.get_open, "open"),
                read: resolve_fn!(api.get_read, "read"),
                close: resolve_fn!(api.get_close, "close"),
                stat: resolve_fn!(api.get_stat, "stat"),
            })
        }
    }

    fn opendir(&self, url: &str, cancel: &AtomicBool) -> Result<Vec<SmbEntry>, SmbOpError> {
        let c_url = CString::new(url).map_err(|_| SmbOpError::of("invalid SMB path"))?;
        crate::source::net_trace(format!("smb_opendir_call url={url}"));
        unsafe {
            if cancel.load(Ordering::SeqCst) {
                return Err(SmbOpError::cancelled());
            }
            let dir = (self.opendir)(self.ctx, c_url.as_ptr());
            crate::source::net_trace(format!(
                "smb_opendir_return url={url} ok={}",
                !dir.is_null()
            ));
            if dir.is_null() {
                return Err(SmbOpError::of(format!("cannot open smb directory: {url}")));
            }
            // Always close the handle, including cancellation and read errors.
            let result = (|| {
                let mut entries: Vec<SmbEntry> = Vec::new();
                let mut iterations: usize = 0;
                loop {
                    iterations += 1;
                    if iterations > 10_000 {
                        return Err(SmbOpError::of(format!("smb readdir runaway: {url}")));
                    }
                    if cancel.load(Ordering::SeqCst) {
                        return Err(SmbOpError::cancelled());
                    }
                    // NULL means either EOF or failure. Only EOF is a complete
                    // listing that the scanner may use to reconcile deletions.
                    *libc::__errno_location() = 0;
                    let entry = (self.readdir)(self.ctx, dir);
                    if entry.is_null() {
                        if *libc::__errno_location() != 0 {
                            return Err(SmbOpError::of(format!(
                                "cannot read smb directory: {url}"
                            )));
                        }
                        break;
                    }
                    let entry = &*entry;
                    if !matches!(
                        entry.smbc_type,
                        SMBC_TYPE_FILE_SHARE | SMBC_TYPE_DIR | SMBC_TYPE_FILE
                    ) {
                        continue;
                    }
                    let name = CStr::from_ptr(entry.name.as_ptr())
                        .to_string_lossy()
                        .into_owned();
                    if name.is_empty() || name == "." || name == ".." {
                        continue;
                    }
                    entries.push(SmbEntry {
                        name,
                        is_dir: matches!(entry.smbc_type, SMBC_TYPE_FILE_SHARE | SMBC_TYPE_DIR),
                        // Directory listings do not carry sizes in Samba 4.x;
                        // sizes come from `stat`.
                        size: 0,
                    });
                }
                Ok(entries)
            })();
            (self.closedir)(self.ctx, dir);
            result
        }
    }

    fn stat(&self, url: &str, cancel: &AtomicBool) -> Result<SmbFileMeta, SmbOpError> {
        let c_url = CString::new(url).map_err(|_| SmbOpError::of("invalid SMB path"))?;
        unsafe {
            if cancel.load(Ordering::SeqCst) {
                return Err(SmbOpError::cancelled());
            }
            let mut st: libc::stat = std::mem::zeroed();
            let result = (self.stat)(self.ctx, c_url.as_ptr(), &mut st);
            if result != 0 {
                return Err(SmbOpError::of(format!("cannot stat: {url}")));
            }
            Ok(SmbFileMeta::file(
                st.st_size.max(0) as u64,
                (st.st_mode & libc::S_IFMT) == libc::S_IFDIR,
                Some(st.st_mtime as i64),
            ))
        }
    }

    /// Read at most `max_bytes` from a file; `usize::MAX` reads it whole.
    fn read_prefix(
        &self,
        url: &str,
        max_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, SmbOpError> {
        let c_url = CString::new(url).map_err(|_| SmbOpError::of("invalid SMB path"))?;
        unsafe {
            if cancel.load(Ordering::SeqCst) {
                return Err(SmbOpError::cancelled());
            }
            let file = (self.open)(self.ctx, c_url.as_ptr(), libc::O_RDONLY, 0);
            if file.is_null() {
                return Err(SmbOpError::of(format!("cannot open smb file: {url}")));
            }
            let mut data: Vec<u8> = Vec::with_capacity(max_bytes.min(64 * 1024));
            let mut buffer = [0u8; 32 * 1024];
            while data.len() < max_bytes {
                if cancel.load(Ordering::SeqCst) {
                    (self.close)(self.ctx, file);
                    return Err(SmbOpError::cancelled());
                }
                let want = (max_bytes - data.len()).min(buffer.len());
                let got = (self.read)(self.ctx, file, buffer.as_mut_ptr().cast(), want);
                if got < 0 {
                    (self.close)(self.ctx, file);
                    return Err(SmbOpError::of(format!("smb read failed: {url}")));
                }
                if got == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..got as usize]);
            }
            (self.close)(self.ctx, file);
            Ok(data)
        }
    }
}

impl Drop for SmbClient {
    /// Free the Samba context (closing its cached connections and files).
    /// Without this, every operation leaks a full SMBCCTX with its sockets
    /// and krb5 state, and long sessions die with "Too many open files".
    /// Runs on the creating worker thread, as required by SMBCCTX.
    fn drop(&mut self) {
        if let Ok(api) = api() {
            unsafe { (api.free_context)(self.ctx, 1) };
            crate::source::net_trace("smb_client_context_freed");
        }
    }
}

// ---------------------------------------------------------------------------
// Per-server flight isolation, bounded execution, credential ladder.
// ---------------------------------------------------------------------------

/// Execution lane for an SMB operation. USER lane covers everything the user
/// is waiting on right now (opening photos, folder navigation, scans);
/// BACKGROUND lane covers availability probes and startup checks. Background
/// work queues behind user work: a photograph is never stuck behind a batch
/// of probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmbLane {
    User,
    Background,
}

/// Per-server serialization. SMBCCTX is not safe under concurrency, so all
/// operations for one host queue on one slot. The slot is priority-aware:
/// when it frees, a waiting USER operation goes before waiting BACKGROUND
/// operations. A running C call cannot be interrupted, but it is bounded by
/// the Samba context timeout (smbc_setTimeout), so worst-case user wait is
/// one in-flight operation, never the whole background queue.
struct HostFlight {
    state: Mutex<FlightState>,
    cv: std::sync::Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FlightState {
    active: Option<SmbLane>,
    waiting_users: usize,
    waiting_background: usize,
}

static HOST_FLIGHTS: OnceLock<
    Mutex<std::collections::HashMap<String, std::sync::Arc<HostFlight>>>,
> = OnceLock::new();

fn host_flight(host: &str) -> std::sync::Arc<HostFlight> {
    let key = host.to_ascii_lowercase();
    let map = HOST_FLIGHTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut map = match map.lock() {
        Ok(map) => map,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.entry(key)
        .or_insert_with(|| {
            std::sync::Arc::new(HostFlight {
                state: Mutex::new(FlightState {
                    active: None,
                    waiting_users: 0,
                    waiting_background: 0,
                }),
                cv: std::sync::Condvar::new(),
            })
        })
        .clone()
}

impl HostFlight {
    fn lock_lane(&self, lane: SmbLane) -> FlightGuard {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match lane {
            SmbLane::User => {
                while state.active.is_some() {
                    state = self
                        .cv
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                state.active = Some(SmbLane::User);
            }
            SmbLane::Background => {
                state.waiting_background += 1;
                while state.active.is_some() || state.waiting_users > 0 {
                    state = self
                        .cv
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                state.waiting_background -= 1;
                state.active = Some(SmbLane::Background);
            }
        }
        FlightGuard { flight: self, lane }
    }

    fn unlock(&self, lane: SmbLane) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = None;
        self.cv.notify_all();
        let _ = lane;
    }
}

struct FlightGuard<'a> {
    flight: &'a HostFlight,
    lane: SmbLane,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        self.flight.unlock(self.lane);
    }
}

/// The current OS account with an empty password: the fallback identity when a
/// server asks for a login and no credentials were supplied. This mirrors
/// `smbclient -N` and GNOME's accepted "cancel" on the auth prompt. Windows
/// file servers commonly deny the literal "guest" account and empty-user
/// sessions while accepting an anonymous session under the caller's own local
/// identity, so libsmbclient's classic guest mapping produced spurious
/// "access denied".
fn os_default_credentials() -> SmbCredentials {
    SmbCredentials {
        user: os_username(),
        password: String::new(),
    }
}

/// Resolve the current account name. `getpwuid` is authoritative; `$USER`
/// backs it up on minimal runtimes.
fn os_username() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned();
        }
    }
    std::env::var("USER")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|_| String::from("guest"))
}

/// Anonymous/guest fallback: the OS account with an empty password (see
/// `os_default_credentials`).
fn guest_credentials() -> SmbCredentials {
    os_default_credentials()
}

/// True when the last failed libsmbclient call set errno to an access-denied
/// style error (wrong username/password), as opposed to unreachable server,
/// missing share, and so on.
fn last_call_was_auth_failure() -> bool {
    let error = std::io::Error::last_os_error();
    matches!(error.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM))
}

/// A single connection attempt's failure, split by cause: `auth` failures are
/// worth retrying with other credentials; everything else (offline, missing
/// share) is not.
#[derive(Debug)]
struct SmbAttemptError {
    kind: SmbTransportError,
    auth: bool,
}

impl From<SmbTransportError> for SmbAttemptError {
    fn from(kind: SmbTransportError) -> Self {
        let auth = matches!(kind, SmbTransportError::AuthRequired(_));
        Self { kind, auth }
    }
}

/// Credential ladder: an explicit override (freshly typed credentials) is
/// used alone; otherwise saved credentials first, then guest access for this
/// exact server+share. A failed login never touches the stored entry.
fn credential_ladder(
    target: &SmbTarget,
    override_creds: Option<&SmbCredentials>,
) -> Vec<SmbCredentials> {
    if let Some(credentials) = override_creds {
        return vec![credentials.clone()];
    }
    let mut candidates = Vec::new();
    if let Some(stored) = stored_smb_credentials(target) {
        candidates.push(stored);
    }
    candidates.push(guest_credentials());
    candidates
}

fn run_bounded<T, Operation>(
    lane: SmbLane,
    trace_label: &'static str,
    target: &SmbTarget,
    timeout: Duration,
    credentials: SmbCredentials,
    operation: Operation,
) -> Result<T, SmbAttemptError>
where
    T: Send + 'static,
    Operation: FnOnce(&SmbClient, &AtomicBool) -> Result<T, SmbOpError> + Send + 'static,
{
    let flight = host_flight(&target.host);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let cancel_for_worker = cancel.clone();
    let (sender, receiver) = mpsc::channel();
    let target = target.clone();
    let spawn = std::thread::Builder::new()
        .name(String::from("pic-smb-transport"))
        .spawn(move || {
            // Queue on the host slot (background work yields to user work).
            // An abandoned caller (timeout arm below) marks its attempt
            // cancelled, and the worker aborts as soon as it reaches the
            // front of the queue instead of starting new Samba work.
            let flight_guard = flight.lock_lane(lane);
            if cancel_for_worker.load(Ordering::SeqCst) {
                drop(flight_guard);
                let _ = sender.send(Err(SmbAttemptError {
                    kind: SmbTransportError::Failed(String::from("cancelled")),
                    auth: false,
                }));
                return;
            }
            let outcome = SmbClient::new(&credentials)
                .map_err(|message| SmbOpError {
                    message,
                    auth: false,
                })
                .and_then(|client| {
                    let started = Instant::now();
                    let result = operation(&client, &cancel_for_worker);
                    crate::source::net_trace(format!(
                        "smb_transport {trace_label} ms={:.1}",
                        started.elapsed().as_secs_f64() * 1000.0
                    ));
                    result
                });
            drop(flight_guard);
            let cancelled = cancel_for_worker.load(Ordering::SeqCst);
            let outcome = outcome.map_err(|error| {
                if cancelled && !error.auth {
                    // The caller already gave up; report its timeout.
                    SmbAttemptError {
                        kind: SmbTransportError::Timeout,
                        auth: false,
                    }
                } else {
                    SmbAttemptError {
                        kind: SmbTransportError::Failed(error.message),
                        auth: error.auth,
                    }
                }
            });
            let _ = sender.send(outcome);
        });
    if let Err(spawn_error) = spawn {
        return Err(SmbAttemptError {
            kind: SmbTransportError::Failed(format!("could not start SMB worker: {spawn_error}")),
            auth: false,
        });
    }
    let outcome = match receiver.recv_timeout(timeout) {
        Ok(outcome) => outcome,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Abandon the attempt. A queued worker aborts the moment it
            // reaches the front of the queue; a RUNNING worker cannot be
            // interrupted mid-C-call, but it is bounded by the Samba context
            // timeout (smbc_setTimeout) and terminates on its own.
            cancel.store(true, Ordering::SeqCst);
            crate::source::net_trace(format!(
                "smb_transport_timeout label={trace_label} host={} timeout_ms={}",
                target.host,
                timeout.as_millis()
            ));
            return Err(SmbAttemptError {
                kind: SmbTransportError::Timeout,
                auth: false,
            });
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(SmbAttemptError {
                kind: SmbTransportError::Failed(String::from("SMB worker terminated unexpectedly")),
                auth: false,
            });
        }
    };
    outcome
}

/// Run `operation` for `target` trying the credential ladder in order. Auth
/// failures advance the ladder; any other failure stops it.
fn run_with_ladder<T, Operation>(
    lane: SmbLane,
    trace_label: &'static str,
    target: &SmbTarget,
    timeout: Duration,
    override_creds: Option<&SmbCredentials>,
    operation: Operation,
) -> Result<T, SmbTransportError>
where
    T: Send + 'static,
    Operation: Fn(&SmbClient, &AtomicBool) -> Result<T, SmbOpError> + Clone + Send + Sync + 'static,
{
    // Fail closed before accessing credentials when the private library is absent.
    api()?;
    let candidates = credential_ladder(target, override_creds);
    let mut last: Option<SmbAttemptError> = None;
    for credentials in candidates {
        match run_bounded(
            lane,
            trace_label,
            target,
            timeout,
            credentials,
            operation.clone(),
        ) {
            Ok(value) => return Ok(value),
            Err(attempt) => {
                let auth = attempt.auth;
                crate::source::net_trace(format!(
                    "smb_transport_failed label={trace_label} host={} auth={} error={}",
                    target.host, auth, attempt.kind
                ));
                last = Some(attempt);
                if !auth {
                    break;
                }
            }
        }
    }
    Err(last
        .map(|attempt| attempt.kind)
        .unwrap_or(SmbTransportError::Failed(String::from(
            "no SMB credentials were tried",
        ))))
}

// ---------------------------------------------------------------------------
// Public transport operations.
// ---------------------------------------------------------------------------

/// Samba's own I/O timeout for the context (bounded C-side); the caller-side
/// `timeout` below additionally bounds how long the UI's worker waits.
pub const SMB_OP_TIMEOUT: Duration = Duration::from_secs(12);
/// Whole-file reads (full-resolution photos can be tens of MB) get a longer
/// caller-side bound. The Samba per-response timeout still aborts dead
/// connections; this bound only ends a stall.
pub const SMB_READ_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Secret Service credential storage (Milestone 2). Passwords live ONLY in
// the keyring - never in SQLite, never in trace logs, never in source files.
// ---------------------------------------------------------------------------

const KEYRING_SERVICE: &str = "pic-rs";

/// Keyring entry identity for one exact server+share. Host case is
/// normalized; the share stays as addressed (shares on one host stay
/// distinct).
fn credential_key(target: &SmbTarget) -> String {
    format!(
        "smb://{}/{}",
        target.host.to_ascii_lowercase(),
        target.share
    )
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredSecret {
    username: String,
    password: String,
}

/// Read the stored credentials for a share from the Secret Service. Returns
/// None when nothing is stored (or no keyring service is reachable - e.g.
/// tests without a session bus), never on a wrong password.
fn stored_smb_credentials(target: &SmbTarget) -> Option<SmbCredentials> {
    read_smb_credentials(target).or_else(|| {
        if target.share.is_empty() {
            return None;
        }
        let server = parse_smb_uri(&format!("smb://{}/", target.host))?;
        read_smb_credentials(&server)
    })
}

static SESSION_CREDENTIALS: OnceLock<Mutex<HashMap<String, SmbCredentials>>> = OnceLock::new();

fn read_smb_credentials(target: &SmbTarget) -> Option<SmbCredentials> {
    let key = credential_key(target);
    if let Some(credentials) = SESSION_CREDENTIALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&key)
    {
        return Some(credentials.clone());
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key).ok()?;
    let secret = entry.get_password().ok()?;
    let parsed: StoredSecret = serde_json::from_str(&secret).ok()?;
    if parsed.username.is_empty() {
        // An empty username duplicates the guest attempt; skip it.
        return None;
    }
    crate::source::net_trace(format!("smb_credentials_found share={key}"));
    Some(SmbCredentials {
        user: parsed.username,
        password: parsed.password,
    })
}

/// Store (or overwrite) the credentials for a share in the Secret Service.
pub fn store_smb_credentials(uri: &str, username: &str, password: &str) -> Result<(), String> {
    let target = parse_smb_uri(uri).ok_or_else(|| String::from("not an smb:// URI"))?;
    let key = credential_key(&target);
    SESSION_CREDENTIALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(
            key.clone(),
            SmbCredentials {
                user: username.to_string(),
                password: password.to_string(),
            },
        );
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|error| format!("keyring unavailable: {error}"))?;
    let secret = serde_json::to_string(&StoredSecret {
        username: username.to_string(),
        password: password.to_string(),
    })
    .map_err(|error| error.to_string())?;
    entry
        .set_password(&secret)
        .map_err(|error| format!("could not store credentials: {error}"))?;
    crate::source::net_trace(format!("smb_credentials_stored share={key}"));
    Ok(())
}

/// Remove stored credentials for a share (e.g. after a rejected password).
pub fn delete_smb_credentials(uri: &str) -> Result<(), String> {
    let target = parse_smb_uri(uri).ok_or_else(|| String::from("not an smb:// URI"))?;
    let key = credential_key(&target);
    SESSION_CREDENTIALS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&key);
    let entry = keyring::Entry::new(KEYRING_SERVICE, &key)
        .map_err(|error| format!("keyring unavailable: {error}"))?;
    match entry.delete_credential() {
        Ok(()) => {
            crate::source::net_trace(format!("smb_credentials_deleted share={key}"));
            Ok(())
        }
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("could not delete credentials: {error}")),
    }
}

// ---------------------------------------------------------------------------
// Public transport operations (saved credentials, then guest access;
// auth failures advance the ladder, everything else stops it).
// ---------------------------------------------------------------------------

/// True when the direct libsmbclient transport is usable in this environment.
/// Missing libraries mean offline; callers must never select a desktop backend.
pub fn direct_available() -> bool {
    api().is_ok()
}

/// List a directory inside an SMB share.
pub fn list_dir(uri: &str) -> Result<Vec<SmbEntry>, SmbTransportError> {
    let target = parse_smb_uri(uri).ok_or(SmbTransportError::NotSmb)?;
    if target.share.is_empty() {
        return Err(SmbTransportError::NoShare);
    }
    let url = target.smbc_url();
    crate::source::net_trace(format!("smb_list_start url={url}"));
    run_with_ladder(
        SmbLane::User,
        "list_dir",
        &target,
        SMB_OP_TIMEOUT,
        None,
        move |client, cancel| client.opendir(&url, cancel),
    )
}

/// List the shares a server exposes (`smb://host/`) with the same private
/// credential ladder as every other operation.
pub fn list_shares(uri: &str) -> Result<Vec<String>, SmbTransportError> {
    let target = parse_smb_uri(uri).ok_or(SmbTransportError::NotSmb)?;
    let url = format!("smb://{}/", target.host);
    crate::source::net_trace(format!("smb_list_shares url={url}"));
    run_with_ladder(
        SmbLane::User,
        "list_shares",
        &target,
        SMB_OP_TIMEOUT,
        None,
        move |client, cancel| {
            client.opendir(&url, cancel).map(|entries| {
                entries
                    .into_iter()
                    .filter(|entry| entry.is_dir)
                    .map(|entry| entry.name)
                    .collect()
            })
        },
    )
}

/// Stat a path inside an SMB share on the BACKGROUND lane (availability
/// probes). Scanner and user-driven callers use [`stat_in_lane`].
pub fn stat(uri: &str) -> Result<SmbFileMeta, SmbTransportError> {
    stat_in_lane(uri, SmbLane::Background)
}

/// Stat a path on an explicit lane.
pub fn stat_in_lane(uri: &str, lane: SmbLane) -> Result<SmbFileMeta, SmbTransportError> {
    let target = parse_smb_uri(uri).ok_or(SmbTransportError::NotSmb)?;
    if target.share.is_empty() {
        // A registered SERVER root has no share to stat; availability is
        // proven by listing the server's shares over the credential ladder.
        let host = target.host.clone();
        let url = format!("smb://{host}/");
        return run_with_ladder(
            lane,
            "stat_server_root",
            &target,
            SMB_OP_TIMEOUT,
            None,
            move |client, cancel| {
                client.opendir(&url, cancel).map(|entries| SmbFileMeta {
                    size: 0,
                    is_dir: true,
                    mtime: None,
                    shares: entries
                        .iter()
                        .filter(|entry| entry.is_dir)
                        .map(|entry| entry.name.clone())
                        .collect::<Vec<String>>(),
                })
            },
        )
        .map(|meta| SmbFileMeta {
            size: 0,
            is_dir: true,
            mtime: None,
            shares: meta.shares,
        });
    }
    let url = target.smbc_url();
    crate::source::net_trace(format!(
        "smb_stat_start url={url} lane={}",
        match lane {
            SmbLane::User => "user",
            SmbLane::Background => "background",
        }
    ));
    run_with_ladder(
        lane,
        "stat",
        &target,
        SMB_OP_TIMEOUT,
        None,
        move |client, cancel| client.stat(&url, cancel),
    )
}

/// Read a whole file inside an SMB share.
pub fn read_file(uri: &str) -> Result<Vec<u8>, SmbTransportError> {
    let target = parse_smb_uri(uri).ok_or(SmbTransportError::NotSmb)?;
    if target.share.is_empty() {
        return Err(SmbTransportError::NoShare);
    }
    let url = target.smbc_url();
    crate::source::net_trace(format!("smb_read_start url={url} whole-file"));
    run_with_ladder(
        SmbLane::User,
        "read_file",
        &target,
        SMB_READ_TIMEOUT,
        None,
        move |client, cancel| client.read_prefix(&url, usize::MAX, cancel),
    )
}

/// Read at most `max_bytes` of a file inside an SMB share.
pub fn read_prefix(uri: &str, max_bytes: usize) -> Result<Vec<u8>, SmbTransportError> {
    let target = parse_smb_uri(uri).ok_or(SmbTransportError::NotSmb)?;
    if target.share.is_empty() {
        return Err(SmbTransportError::NoShare);
    }
    let url = target.smbc_url();
    crate::source::net_trace(format!("smb_read_start url={url} max_bytes={max_bytes}"));
    run_with_ladder(
        SmbLane::User,
        "read_prefix",
        &target,
        SMB_OP_TIMEOUT,
        None,
        move |client, cancel| client.read_prefix(&url, max_bytes, cancel),
    )
}

/// Reachability + credential check with explicit credentials (used by the
/// interactive Retry Connection after the user typed them). `Ok` means the
/// credentials work; `AuthRequired` means the server rejected them.
pub fn check_available_with(
    uri: &str,
    username: &str,
    password: &str,
) -> Result<SmbFileMeta, SmbTransportError> {
    let target = parse_smb_uri(uri).ok_or(SmbTransportError::NotSmb)?;
    let server_root = target.share.is_empty();
    let url = if server_root {
        format!("smb://{}/", target.host)
    } else {
        target.smbc_url()
    };
    let override_creds = SmbCredentials {
        user: username.to_string(),
        password: password.to_string(),
    };
    crate::source::net_trace(format!(
        "smb_check_start url={url} user={}",
        if username.is_empty() {
            "guest"
        } else {
            username
        }
    ));
    run_with_ladder(
        SmbLane::User,
        "check_available",
        &target,
        SMB_OP_TIMEOUT,
        Some(&override_creds),
        move |client, cancel| {
            if server_root {
                client.opendir(&url, cancel).map(|entries| SmbFileMeta {
                    size: 0,
                    is_dir: true,
                    mtime: None,
                    shares: entries
                        .into_iter()
                        .filter(|entry| entry.is_dir)
                        .map(|entry| entry.name)
                        .collect(),
                })
            } else {
                client.stat(&url, cancel)
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_credentials_precede_guest_and_explicit_credentials_are_exclusive() {
        let target = parse_smb_uri("smb://pic-credential-test.invalid/photos").unwrap();
        let key = credential_key(&target);
        SESSION_CREDENTIALS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(
                key.clone(),
                SmbCredentials {
                    user: "saved-user".into(),
                    password: "test-only".into(),
                },
            );
        let candidates = credential_ladder(&target, None);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].user, "saved-user");
        assert!(candidates[1].user.is_empty());
        let typed = SmbCredentials {
            user: "typed-user".into(),
            password: "test-only".into(),
        };
        let candidates = credential_ladder(&target, Some(&typed));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].user, "typed-user");
        SESSION_CREDENTIALS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .remove(&key);
    }

    #[test]
    fn parses_stored_library_uris() {
        // Host with port, percent-encoded path segments: stored URIs must
        // decode exactly and keep shares distinct.
        let target = parse_smb_uri("smb://DietPi.local:445/4TBS/Sport").expect("parses");
        assert_eq!(target.host, "DietPi.local");
        assert_eq!(target.share, "4TBS");
        assert_eq!(target.path, "Sport");

        let target = parse_smb_uri("smb://dietpi.local/4tbs/sg1/Extras-Stargate%20Atlantis")
            .expect("parses");
        assert_eq!(target.host, "dietpi.local");
        assert_eq!(target.share, "4tbs");
        assert_eq!(target.path, "sg1/Extras-Stargate Atlantis");

        let target = parse_smb_uri("smb://nas/Photos%202026/file.jpg").expect("parses");
        assert_eq!(target.share, "Photos 2026");
        assert_eq!(target.path, "file.jpg");

        // Trailing slash: share root.
        let target = parse_smb_uri("smb://DietPi.local:445/").expect("parses");
        assert_eq!(target.host, "DietPi.local");
        assert_eq!(target.share, "");
        assert_eq!(target.path, "");

        // Userinfo in the URI is ignored (credentials never come from URLs).
        let target = parse_smb_uri("smb://user:secret@nas/share/file.jpg").expect("parses");
        assert_eq!(target.host, "nas");
        assert_eq!(target.share, "share");
        assert_eq!(target.path, "file.jpg");
    }

    #[test]
    fn normalizes_legacy_gvfs_fuse_paths() {
        // The exact failing records from Fedora v17: photos imported before
        // the redesign carry gvfs-FUSE paths that die with the mount.
        let normalized = normalize_smb_reference(
            "/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbp/Other/Christian Other/Father of Lights (2012)/folder.jpg",
        );
        assert_eq!(
            normalized,
            "smb://dietpi.local/4tbp/Other/Christian%20Other/Father%20of%20Lights%20%282012%29/folder.jpg"
        );

        let normalized = normalize_smb_reference(
            "/run/user/1000/gvfs/smb-share:server=dietpi.local,share=4tbs/sg1/file.JPG",
        );
        assert_eq!(normalized, "smb://dietpi.local/4tbs/sg1/file.JPG");

        // Other option orders and extra options still parse; ports drop.
        let normalized = normalize_smb_reference(
            "/run/user/1000/gvfs/smb-share:share=Work,server=10.0.0.1:445/sub/dir/pic.png",
        );
        assert_eq!(normalized, "smb://10.0.0.1/Work/sub/dir/pic.png");

        // Case of the stored path is preserved (only the scheme is built).
        let normalized = normalize_smb_reference(
            "/run/user/1000/gvfs/smb-share:server=DietPi.local,share=4TBS/Photos/IMG_0001.JPG",
        );
        assert_eq!(normalized, "smb://DietPi.local/4TBS/Photos/IMG_0001.JPG");

        // Pass-throughs: canonical URIs, NFS FUSE paths, plain local paths.
        assert_eq!(
            normalize_smb_reference("smb://dietpi.local/4tbs/x.jpg"),
            "smb://dietpi.local/4tbs/x.jpg"
        );
        assert_eq!(
            normalize_smb_reference(
                "/run/user/1000/gvfs/nfs:host=DietPi.local,prefix=%2Fexports%2FWork/a.png"
            ),
            "/run/user/1000/gvfs/nfs:host=DietPi.local,prefix=%2Fexports%2FWork/a.png"
        );
        assert_eq!(
            normalize_smb_reference("/home/peet/Pictures/a.jpg"),
            "/home/peet/Pictures/a.jpg"
        );
        // Malformed gvfs option strings pass through untouched.
        assert_eq!(
            normalize_smb_reference("/run/user/1000/gvfs/smb-share:bogus/a.jpg"),
            "/run/user/1000/gvfs/smb-share:bogus/a.jpg"
        );
    }

    #[test]
    fn rejects_non_smb_and_empty_authorities() {
        assert!(parse_smb_uri("nfs://host/export").is_none());
        assert!(parse_smb_uri("file:///home/peet").is_none());
        assert!(parse_smb_uri("/mnt/4TBP").is_none());
        assert!(parse_smb_uri("smb://").is_none());
        assert!(parse_smb_uri("smb://:445/share").is_none());
    }

    #[test]
    fn smbc_urls_are_built_for_the_share_root_and_subpaths() {
        let target = parse_smb_uri("smb://dietpi.local/4tbs").expect("parses");
        assert_eq!(target.smbc_url(), "smb://dietpi.local/4tbs/");
        let target = parse_smb_uri("smb://dietpi.local/4tbs/sg1/Extras-Stargate%20Atlantis")
            .expect("parses");
        assert_eq!(
            target.smbc_url(),
            "smb://dietpi.local/4tbs/sg1/Extras-Stargate Atlantis"
        );
    }

    // -- Live tests against the DietPi guest share (no gvfs mount involved).
    // Run with: cargo test smb_live -- --ignored --test-threads=1

    const LIVE_SHARE_URI: &str = "smb://dietpi.local/4tbs/";

    #[test]
    #[ignore = "requires the LAN SMB server and a GNOME session; use isolated XDG_CACHE_HOME"]
    fn private_network_mount_list_smoke() {
        fn mounts() -> Vec<String> {
            let output = std::process::Command::new("gio")
                .args(["mount", "-l"])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut mounts: Vec<_> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.split_once(" -> ").map(|(_, uri)| uri.to_string()))
                .collect();
            mounts.sort();
            mounts
        }
        let baseline = mounts();
        let unchanged =
            |step: &str| assert_eq!(mounts(), baseline, "desktop mounts changed after {step}");
        for _ in crate::source::discover_network_locations() {}
        unchanged("startup server discovery");
        let server = "smb://dietpi.local/";
        let shares = list_shares(server).expect("direct anonymous enumeration");
        assert!(
            shares
                .iter()
                .any(|share| share.eq_ignore_ascii_case("4tbs")),
            "the known accessible share must survive entry decoding: {shares:?}"
        );
        unchanged("anonymous share enumeration");
        check_available_with(server, "", "").expect("direct server authentication");
        unchanged("server authentication");
        assert!(crate::source::probe_source_available(LIVE_SHARE_URI));
        unchanged("startup availability");
        let mut pending = std::collections::VecDeque::from([(LIVE_SHARE_URI.to_string(), 0)]);
        let mut photo = None;
        for _ in 0..64 {
            let Some((directory, depth)) = pending.pop_front() else {
                break;
            };
            for entry in list_dir(&directory).expect("private folder browsing") {
                let uri = format!(
                    "{}/{}",
                    directory.trim_end_matches('/'),
                    percent_encode_segment(&entry.name)
                );
                if entry.is_dir && depth < 4 {
                    pending.push_back((uri, depth + 1));
                } else if !entry.is_dir && entry.name.to_ascii_lowercase().ends_with(".jpg") {
                    let meta = stat(&uri).unwrap();
                    if meta.size > 0 && meta.size < 16 * 1024 * 1024 {
                        photo = Some((uri, meta));
                        break;
                    }
                }
            }
            if photo.is_some() {
                break;
            }
        }
        unchanged("folder browsing");
        let (uri, meta) = photo.expect("a small JPEG on the live share");
        assert!(!crate::source::read(&uri).unwrap().is_empty());
        unchanged("photo read");
        let thumbnail = crate::thumbnail::create(&uri, meta.mtime, Some(meta.size as i64)).unwrap();
        assert!(thumbnail.is_file());
        unchanged("thumbnail generation");
        let image = crate::thumbnail::decode_for_viewer(&uri, 1920, 1080).unwrap();
        assert!(image.width() > 0 && image.height() > 0);
        unchanged("viewer decode and materialization");
        assert!(!crate::source::probe_source_available(
            "smb://127.0.0.1/PIC-nonexistent-share/"
        ));
        unchanged("unavailable share probe");
        assert!(crate::source::probe_source_available(LIVE_SHARE_URI));
        unchanged("retry live share");
        assert!(crate::source::read("nfs://localhost/photos/a.jpg")
            .unwrap_err()
            .to_string()
            .contains("NFS is unavailable"));
        unchanged("NFS failure");
        crate::source::private_transport_tests::missing_smb_library_fails_closed_across_restarts();
        unchanged("missing SMB library and process restarts");
    }

    #[test]
    #[ignore = "requires the DietPi SMB server on the LAN"]
    fn smb_live_lists_guest_share() {
        let entries = list_dir(LIVE_SHARE_URI).expect("guest listing works");
        crate::source::net_trace(format!("smb_live_list count={}", entries.len()));
        assert!(!entries.is_empty(), "the guest share should list entries");
        assert!(entries.iter().all(|entry| !entry.name.is_empty()));
        assert!(
            entries.iter().any(|entry| entry.is_dir) || entries.iter().any(|entry| entry.size > 0)
        );
    }

    /// Walk down from the share root and return the first regular file
    /// smaller than `max_size` (skips the multi-GB videos that share the NAS
    /// with the photos).
    fn find_small_file(max_size: u64) -> String {
        let mut queue: Vec<String> = vec![LIVE_SHARE_URI.to_string()];
        let mut levels = 0;
        while let Some(dir_uri) = queue.pop() {
            if levels > 3 {
                panic!("no file found within 3 levels of {LIVE_SHARE_URI}");
            }
            levels += 1;
            for entry in list_dir(&dir_uri).expect("guest listing works") {
                if entry.is_dir {
                    queue.push(format!("{}{}/", dir_uri, entry.name));
                    continue;
                }
                let file_uri = format!("{}{}", dir_uri, entry.name);
                if let Ok(meta) = stat(&file_uri) {
                    if meta.size <= max_size && meta.size > 0 {
                        return file_uri;
                    }
                }
            }
        }
        panic!("no suitable file found under {LIVE_SHARE_URI}");
    }

    #[test]
    #[ignore = "requires the DietPi SMB server on the LAN"]
    fn smb_live_reads_file_prefix() {
        let file_uri = find_small_file(32 * 1024 * 1024);
        let bytes = read_prefix(&file_uri, 4096).expect("guest read works");
        assert!(!bytes.is_empty());
        assert!(bytes.len() <= 4096);
        crate::source::net_trace(format!(
            "smb_live_read uri={file_uri} bytes={}",
            bytes.len()
        ));
    }

    #[test]
    #[ignore = "requires the DietPi SMB server on the LAN"]
    fn smb_live_stats_share_root() {
        let meta = stat(LIVE_SHARE_URI).expect("guest stat works");
        assert!(meta.is_dir);
    }

    #[test]
    #[ignore = "requires the DietPi SMB server on the LAN"]
    fn smb_live_source_read_integration() {
        // End-to-end: source::read must dispatch smb:// URIs to this
        // transport (this is the integration point used by scanning,
        // thumbnails, RAW materialization, lightbox and export).
        let file_uri = find_small_file(32 * 1024 * 1024);
        let bytes = crate::source::read(&file_uri)
            .unwrap_or_else(|error| panic!("source::read failed for {file_uri}: {error}"));
        assert!(!bytes.is_empty());
        crate::source::net_trace(format!(
            "smb_source_read_ok uri={file_uri} bytes={}",
            bytes.len()
        ));
    }
}
