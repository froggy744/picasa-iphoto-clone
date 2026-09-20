/*
 * PIC portable, unprivileged, read-only NFSv4 client (MIT).
 * libnfs is an external library: ship and comply with its own licenses.
 * This process has the same OS rights as PIC; it is NEVER a setuid/setcap binary.
 * NFS servers that require a reserved source port need an OS-admin provisioned
 * client (e.g. PIC's existing Linux restricted helper) or a native NFS mount.
 *
 * Wire protocol is intentionally compatible with the existing Fedora helper:
 * Request: [S|L|R:1][host-length:u32be][host][path-length:u32be][path]
 * Response: 'E' [message-length:u32be][message], or 'O' followed by
 * S: [directory:1][size:u64be][mtime:u64be]
 * L: [name-length:u32be][name][directory:1][size:u64be][mtime:u64be]..., 0
 * R: [length:u32be][bytes]..., 0 (0xffffffff marks partial-read error).
 */
#define _POSIX_C_SOURCE 200809L
#include <nfsc/libnfs.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <sys/stat.h>
#ifdef _WIN32
#include <io.h>
#include <windows.h>
#ifndef S_IFMT
#define S_IFMT _S_IFMT
#endif
#ifndef S_IFDIR
#define S_IFDIR _S_IFDIR
#endif
#ifndef S_IFREG
#define S_IFREG _S_IFREG
#endif
#endif

#define MAX_HOST 253U
#define MAX_PATH 4096U
#define MAX_ENTRIES 100000U
#define MAX_FILE (512ULL * 1024ULL * 1024ULL)
#define BLOCK_SIZE (64U * 1024U)

static struct nfs_context *nfs;
static char mounted_host[MAX_HOST + 1];

static int read_all(void *dst, size_t n) {
    return n == 0 || fread(dst, 1, n, stdin) == n;
}
static int write_all(const void *src, size_t n) {
    return n == 0 || fwrite(src, 1, n, stdout) == n;
}
static int get_u32(uint32_t *n) {
    unsigned char b[4];
    if (!read_all(b, 4)) return 0;
    *n = ((uint32_t)b[0] << 24) | ((uint32_t)b[1] << 16)
       | ((uint32_t)b[2] << 8) | (uint32_t)b[3];
    return 1;
}
static int put_u32(uint32_t n) {
    unsigned char b[4] = {(unsigned char)(n >> 24), (unsigned char)(n >> 16),
                          (unsigned char)(n >> 8), (unsigned char)n};
    return write_all(b, 4);
}
static int put_u64(uint64_t n) {
    unsigned char b[8];
    for (int i = 7; i >= 0; --i) { b[i] = (unsigned char)n; n >>= 8; }
    return write_all(b, 8);
}
static int fail(const char *message) {
    size_t len = strlen(message);
    if (len > 1024) len = 1024;
    return write_all("E", 1) && put_u32((uint32_t)len)
        && write_all(message, len) && fflush(stdout) == 0;
}
static int lib_fail(const char *operation, int rc) {
    char message[1200];
    const char *detail = nfs ? nfs_get_error(nfs) : "NFS context not initialized";
    snprintf(message, sizeof(message), "%s failed (%d): %s. If permission is denied on a secure NFS export, use an administrator-configured NFS client; the bundled PIC helper does not acquire elevated rights.",
             operation, rc, (detail && *detail) ? detail : "unknown error");
    return fail(message);
}
/* Reject traversal and ambiguous representations before any library access. */
static int valid_path(const char *path) {
    size_t n = strlen(path);
    if (!n || n > MAX_PATH || path[0] != '/') return 0;
    if (n == 1) return 1; /* NFSv4 pseudo-root */
    for (size_t i = 1; i < n; ++i) {
        if (path[i] == '\\') return 0;
        if (path[i] == '/' && path[i-1] == '/') return 0;
    }
    size_t pos = 1;
    while (pos < n) {
        size_t end = pos;
        while (end < n && path[end] != '/') ++end;
        size_t len = end - pos;
        if ((len == 1 && path[pos] == '.') ||
            (len == 2 && path[pos] == '.' && path[pos+1] == '.')) return 0;
        pos = end + 1;
    }
    return 1;
}
static int valid_host(const char *host) {
    size_t len = strlen(host);
    if (!len || len > MAX_HOST) return 0;
    for (const unsigned char *p = (const unsigned char *)host; *p; ++p) {
        if (!( (*p >= 'a' && *p <= 'z') || (*p >= 'A' && *p <= 'Z') ||
               (*p >= '0' && *p <= '9') || *p == '.' || *p == '-' || *p == ':' )) return 0;
    }
    return 1;
}
static int prepare_nfs(const char *host) {
    if (nfs && strcmp(host, mounted_host) == 0) return 0;
    if (nfs) { nfs_destroy_context(nfs); nfs = NULL; mounted_host[0] = '\0'; }
    nfs = nfs_init_context();
    if (!nfs) return -1;
    nfs_set_autoreconnect(nfs, 2);
    nfs_set_retrans(nfs, 2);
    int rc = nfs_set_version(nfs, 4);
    if (rc != 0) return rc;
    rc = nfs_mount(nfs, host, "/");
    if (rc == 0) strcpy(mounted_host, host);
    return rc;
}
static int respond_stat(const char *path) {
    struct nfs_stat_64 st = {0};
    int rc = nfs_stat64(nfs, path, &st);
    if (rc < 0) return lib_fail("nfs_stat64", rc);
    unsigned char dir = (st.nfs_mode & S_IFMT) == S_IFDIR;
    return write_all("O", 1) && write_all(&dir, 1)
        && put_u64(st.nfs_size) && put_u64(st.nfs_mtime)
        && fflush(stdout) == 0;
}
static int respond_list(const char *path) {
    struct nfsdir *dir = NULL;
    int rc = nfs_opendir(nfs, path, &dir);
    if (rc < 0) return lib_fail("nfs_opendir", rc);
    if (!write_all("O", 1)) { nfs_closedir(nfs, dir); return 0; }
    uint32_t count = 0;
    struct nfsdirent *ent;
    while ((ent = nfs_readdir(nfs, dir)) != NULL) {
        if (!ent->name || !strcmp(ent->name, ".") || !strcmp(ent->name, "..")) continue;
        size_t len = strlen(ent->name);
        if (++count > MAX_ENTRIES || !len || len > MAX_PATH) {
            nfs_closedir(nfs, dir);
            const char *message = "NFS directory exceeds response limit";
            return put_u32(UINT32_MAX) && put_u32((uint32_t)strlen(message))
                && write_all(message, strlen(message)) && fflush(stdout) == 0;
        }
        unsigned char is_dir = ent->type == 2;
        if (!put_u32((uint32_t)len) || !write_all(ent->name, len)
            || !write_all(&is_dir, 1) || !put_u64(ent->size)
            || !put_u64((uint64_t)ent->mtime.tv_sec)) {
            nfs_closedir(nfs, dir); return 0;
        }
    }
    nfs_closedir(nfs, dir);
    return put_u32(0) && fflush(stdout) == 0;
}
static int respond_read(const char *path) {
    struct nfs_stat_64 st = {0};
    int rc = nfs_stat64(nfs, path, &st);
    if (rc < 0) return lib_fail("nfs_stat64", rc);
    if ((st.nfs_mode & S_IFMT) != S_IFREG) return fail("NFS read requires a regular file");
    if (st.nfs_size > MAX_FILE) return fail("NFS file exceeds the 512 MiB transport limit");
    struct nfsfh *fh = NULL;
    rc = nfs_open(nfs, path, O_RDONLY, &fh);
    if (rc < 0) return lib_fail("nfs_open", rc);
    if (!write_all("O", 1)) { nfs_close(nfs, fh); return 0; }
    unsigned char buf[BLOCK_SIZE];
    uint64_t total = 0;
    for (;;) {
        rc = nfs_read(nfs, fh, buf, sizeof(buf));
        if (rc <= 0) break;
        total += (unsigned)rc;
        if (total > MAX_FILE || !put_u32((uint32_t)rc)
            || !write_all(buf, (size_t)rc)) {
            nfs_close(nfs, fh); return 0;
        }
    }
    if (rc < 0) {
        char message[1200];
        const char *detail = nfs_get_error(nfs);
        snprintf(message, sizeof(message), "nfs_read failed (%d): %s", rc,
                 (detail && *detail) ? detail : "unknown error");
        nfs_close(nfs, fh);
        size_t len = strlen(message);
        if (len > 1024) len = 1024;
        return put_u32(UINT32_MAX) && put_u32((uint32_t)len)
            && write_all(message, len) && fflush(stdout) == 0;
    }
    nfs_close(nfs, fh);
    return put_u32(0) && fflush(stdout) == 0;
}
int main(void) {
#ifdef _WIN32
    /* Rust reads raw binary frames. Text-mode CRLF conversion corrupts JPEGs. */
    if (_setmode(_fileno(stdin), _O_BINARY) < 0 ||
        _setmode(_fileno(stdout), _O_BINARY) < 0) return 2;
#endif
    /* Binary stdio is buffered for speed and flushed after every response. */
    unsigned char op;
    uint32_t host_len, path_len;
    char host[MAX_HOST + 1], path[MAX_PATH + 1];
    while (read_all(&op, 1)) {
        if (!get_u32(&host_len) || !host_len || host_len > MAX_HOST) break;
        if (!read_all(host, host_len)) break;
        host[host_len] = '\0';
        if (!get_u32(&path_len) || !path_len || path_len > MAX_PATH) break;
        if (!read_all(path, path_len)) break;
        path[path_len] = '\0';
        if (memchr(host, 0, host_len) || memchr(path, 0, path_len)
            || !valid_host(host) || !valid_path(path)
            || (op != 'S' && op != 'L' && op != 'R')) {
            if (!fail("Invalid NFS request")) break;
            continue;
        }
        int rc = prepare_nfs(host);
        if (rc != 0) {
            if (!lib_fail("nfs_mount", rc)) break;
            continue;
        }
        int ok = op == 'S' ? respond_stat(path)
               : op == 'L' ? respond_list(path) : respond_read(path);
        if (!ok) break;
    }
    if (nfs) nfs_destroy_context(nfs);
    return ferror(stdin) || ferror(stdout) ? 1 : 0;
}
