//! Portable NFSv4 client transport: one wire protocol on Linux, Windows, macOS.
//! The normal helper is packaged beside PIC, without mounts or elevated rights.
//! An OPTIONAL Linux privileged helper, installed by an administrator, remains
//! supported for NFS exports that refuse non-reserved source ports.
//!
//! The C helper is a separate process and reads only. It uses the existing
//! libnfs protocol (S=stat, L=list, R=read) unchanged.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

#[cfg(target_os = "linux")]
const LINUX_ADMIN_HELPER: &str = "/usr/local/libexec/pic-nfs-helper";
const OP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
static HELPER: OnceLock<Mutex<Option<Helper>>> = OnceLock::new();

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
    path: String,
}

#[cfg(windows)]
const HELPER_NAME: &str = "pic-nfs-helper.exe";
#[cfg(not(windows))]
const HELPER_NAME: &str = "pic-nfs-helper";

fn helper_candidates() -> Vec<PathBuf> {
    // The override is deliberate for development and distribution testing.
    // Do not invoke the shell or interpolate command strings.
    if let Some(path) = std::env::var_os("PIC_NFS_HELPER_PATH") {
        return vec![PathBuf::from(path)];
    }
    let mut candidates = Vec::new();
    // Preserve existing secure-export installations; never replace or chmod
    // their root-owned, capability-limited helper during a normal PIC update.
    #[cfg(target_os = "linux")]
    candidates.push(PathBuf::from(LINUX_ADMIN_HELPER));

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("libexec").join(HELPER_NAME));
            candidates.push(dir.join(HELPER_NAME));
            // macOS .app/Contents/MacOS/pic-rs -> Contents/Helpers/
            #[cfg(target_os = "macos")]
            if let Some(contents) = dir.parent() {
                candidates.push(contents.join("Helpers").join(HELPER_NAME));
                candidates.push(contents.join("Resources").join("libexec").join(HELPER_NAME));
            }
            // AppImage: /tmp/.mount_*/usr/bin/pic-rs -> usr/libexec/
            #[cfg(target_os = "linux")]
            if let Some(usr) = dir.parent() {
                candidates.push(usr.join("libexec").join(HELPER_NAME));
                candidates.push(usr.join("lib").join("pic-rs").join(HELPER_NAME));
            }
        }
    }
    candidates
}

fn helper_executable(path: &std::path::Path) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    if !meta.is_file() { return false; }
    #[cfg(unix)]
    { meta.permissions().mode() & 0o111 != 0 }
    #[cfg(not(unix))]
    { true }
}

fn helper_path() -> Result<PathBuf, String> {
    helper_candidates()
        .into_iter()
        .find(|p| helper_executable(p))
        .ok_or_else(|| format!(
            "NFS client was not found in the PIC application bundle. Expected {} under libexec; reinstall the complete PIC package (PIC_NFS_HELPER_PATH overrides for developers).",
            HELPER_NAME
        ))
}

pub fn direct_available() -> bool {
    std::env::var_os("PIC_TEST_NO_NFSCLIENT").is_none() && helper_path().is_ok()
}

fn decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("Invalid NFS URI percent encoding".into());
            }
            let chunk = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|e| e.to_string())?;
            out.push(u8::from_str_radix(chunk, 16).map_err(|e| e.to_string())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

fn target(uri: &str) -> Result<Target, String> {
    let rest = uri.strip_prefix("nfs://").ok_or("not an NFS URI")?;
    let (authority, path) = rest.split_once('/').ok_or("NFS URI has no path")?;
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']').ok_or("Invalid IPv6 NFS authority")?;
        if !suffix.is_empty() && suffix != ":2049" {
            return Err("Only the standard NFSv4 port 2049 is supported".into());
        }
        host
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if port == "2049" { host } else {
            return Err("Only the standard NFSv4 port 2049 is supported".into());
        }
    } else {
        authority
    };
    if host.is_empty() || host.len() > 253 || host.as_bytes().contains(&0) {
        return Err("Invalid NFS hostname".into());
    }
    let path = decode(path)?;
    // An empty URI path denotes the NFSv4 pseudo-root.
    if path.len() > 4095 || path.as_bytes().contains(&0)
        || path.split('/').any(|part| part == "." || part == "..") {
        return Err("Invalid NFS path".into());
    }
    // A privileged Linux helper enforces a root-owned allowlist; the bundled
    // ordinary-user helper relies on the server's NFS export permissions.
    Ok(Target { host: host.to_owned(), path: format!("/{path}") })
}

struct Helper {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Helper {
    fn spawn() -> Result<Self, String> {
        let path = helper_path()?;
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("NFS helper '{}' cannot start: {e}; reinstall the complete PIC package", path.display()))?;
        let stdin = child.stdin.take().ok_or("NFS helper stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("NFS helper stdout unavailable")?;
        Ok(Self { child, stdin, stdout })
    }

    fn begin(&mut self, op: u8, target: &Target) -> Result<(), Failure> {
        let host = target.host.as_bytes();
        let path = target.path.as_bytes();
        self.stdin.write_all(&[op])?;
        self.stdin.write_all(&(host.len() as u32).to_be_bytes())?;
        self.stdin.write_all(host)?;
        self.stdin.write_all(&(path.len() as u32).to_be_bytes())?;
        self.stdin.write_all(path)?;
        self.stdin.flush()?;
        let mut status = [0u8];
        self.stdout.read_exact(&mut status)?;
        match status[0] {
            b'O' => Ok(()),
            b'E' => {
                let size = read_u32(&mut self.stdout)? as usize;
                if size > 1024 { return Err(Failure::Protocol("NFS helper error too large".into())); }
                let mut message = vec![0; size];
                self.stdout.read_exact(&mut message)?;
                Err(Failure::Remote(String::from_utf8_lossy(&message).into_owned()))
            }
            _ => Err(Failure::Protocol("Invalid NFS helper status".into())),
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
enum Failure {
    Remote(String),
    Protocol(String),
    Io(io::Error),
}

impl From<io::Error> for Failure {
    fn from(value: io::Error) -> Self { Self::Io(value) }
}

fn read_u32(reader: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

fn invoke<T>(target: Target, operation: u8, decode: impl FnOnce(&mut ChildStdout) -> Result<T, Failure>) -> Result<T, String> {
    let lock = HELPER.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().map_err(|_| "NFS helper lock poisoned")?;
    if guard.is_none() { *guard = Some(Helper::spawn()?); }
    let result = (|| {
        let helper = guard.as_mut().ok_or(Failure::Protocol("No NFS helper".into()))?;
        helper.begin(operation, &target)?;
        decode(&mut helper.stdout)
    })();
    match result {
        Ok(value) => Ok(value),
        Err(Failure::Remote(message)) => Err(message),
        Err(Failure::Io(error)) => {
            *guard = None;
            Err(format!("NFS helper connection lost: {error}"))
        }
        Err(Failure::Protocol(message)) => {
            *guard = None;
            Err(format!("NFS helper protocol error: {message}"))
        }
    }
}

fn run<T: Send + 'static>(op: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("pic-nfs-request".into())
        .spawn(move || { let _ = sender.send(op()); })
        .map_err(|e| e.to_string())?;
    receiver.recv_timeout(OP_TIMEOUT)
        .map_err(|_| "NFS helper request timed out".to_owned())?
}

pub fn stat(uri: &str) -> Result<Metadata, String> {
    let target = target(uri)?;
    let result = run(move || invoke(target, b'S', |out| {
        let mut dir = [0u8];
        out.read_exact(&mut dir)?;
        let size = read_u64(out)?;
        let mtime = read_u64(out)?;
        Ok(Metadata { is_dir: dir[0] != 0, size, mtime: mtime as i64 })
    }));
    match &result {
        Ok(meta) => crate::source::net_trace(format!("nfs_helper_stat_ok uri={uri} size={}", meta.size)),
        Err(err) => crate::source::net_trace(format!("nfs_helper_stat_failed uri={uri} error={err}")),
    }
    result
}

pub fn list_dir(uri: &str) -> Result<Vec<Entry>, String> {
    let target = target(uri)?;
    let result = run(move || invoke(target, b'L', |out| {
        let mut entries = Vec::new();
        loop {
            let len = read_u32(out)? as usize;
            if len == 0 { break; }
            if len == u32::MAX as usize {
                let size = read_u32(out)? as usize;
                if size > 1200 { return Err(Failure::Protocol("Invalid NFS directory error".into())); }
                let mut message = vec![0; size];
                out.read_exact(&mut message)?;
                return Err(Failure::Remote(String::from_utf8_lossy(&message).into_owned()));
            }
            if len > 4096 || entries.len() >= MAX_ENTRIES {
                return Err(Failure::Protocol("Invalid NFS directory response".into()));
            }
            let mut name = vec![0u8; len];
            out.read_exact(&mut name)?;
            let mut is_dir = [0u8];
            out.read_exact(&mut is_dir)?;
            let size = read_u64(out)?;
            let mtime = read_u64(out)?;
            entries.push(Entry {
                name: String::from_utf8_lossy(&name).into_owned(),
                is_dir: is_dir[0] != 0,
                size,
                mtime: mtime as i64,
            });
        }
        Ok(entries)
    }));
    match &result {
        Ok(entries) => crate::source::net_trace(format!("nfs_helper_list_ok uri={uri} count={}", entries.len())),
        Err(err) => crate::source::net_trace(format!("nfs_helper_list_failed uri={uri} error={err}")),
    }
    result
}

pub fn read_file(uri: &str) -> Result<Vec<u8>, String> {
    let target = target(uri)?;
    let result = run(move || invoke(target, b'R', |out| {
        let mut bytes = Vec::new();
        loop {
            let len = read_u32(out)?;
            if len == 0 { break; }
            if len == u32::MAX {
                let size = read_u32(out)? as usize;
                if size > 1200 { return Err(Failure::Protocol("Invalid NFS read error".into())); }
                let mut message = vec![0; size];
                out.read_exact(&mut message)?;
                return Err(Failure::Remote(String::from_utf8_lossy(&message).into_owned()));
            }
            let size = len as usize;
            if size > 64 * 1024 || bytes.len().saturating_add(size) > MAX_BYTES {
                return Err(Failure::Protocol("NFS file exceeds transport limit".into()));
            }
            let start = bytes.len();
            bytes.resize(start + size, 0);
            out.read_exact(&mut bytes[start..])?;
        }
        Ok(bytes)
    }));
    match &result {
        Ok(bytes) => crate::source::net_trace(format!("nfs_helper_read_ok uri={uri} bytes={}", bytes.len())),
        Err(err) => crate::source::net_trace(format!("nfs_helper_read_failed uri={uri} error={err}")),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_and_path() {
        let result = target("nfs://DietPi.local:2049/mnt/4TBP/Other/Tat%20Sing/a.jpg").unwrap();
        assert_eq!(result.host, "DietPi.local");
        assert_eq!(result.path, "/mnt/4TBP/Other/Tat Sing/a.jpg");
    }

    #[test]
    fn permits_pseudo_root() {
        let result = target("nfs://DietPi.local/").unwrap();
        assert_eq!(result.path, "/");
    }

    #[test]
    fn rejects_traversal_and_unexpected_ports() {
        assert!(target("nfs://DietPi.local/mnt/%2E%2E/private").is_err());
        assert!(target("nfs://DietPi.local:1234/export").is_err());
    }

    #[test]
    fn rejects_invalid_encoding() {
        assert!(target("nfs://DietPi.local/mnt/4TBP/foo%Q1.jpg").is_err());
    }

    #[test]
    #[ignore = "requires packaged helper and a reachable NFS export"]
    fn live_exact_jpeg_read() {
        let file = std::env::var("PIC_NFS_LIVE_FILE").expect("set PIC_NFS_LIVE_FILE to an exact JPEG URI");
        let metadata = stat(&file).expect("JPEG stat failed");
        assert!(!metadata.is_dir);
        let bytes = read_file(&file).expect("JPEG read failed; do not mark NFS fixed");
        assert!(bytes.starts_with(&[0xff, 0xd8, 0xff]), "JPEG signature missing");
    }
}
