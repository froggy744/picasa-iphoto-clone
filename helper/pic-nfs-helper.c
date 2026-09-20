/*
 * PIC private NFSv4 helper.  Copyright (c) 2026. MIT.
 * Read-only libnfs access through stdio; no mount(2), GVfs or FUSE.
 * Intended for a root-owned executable with CAP_NET_BIND_SERVICE ONLY.
 * Root-owned /etc/pic-nfs-helper.conf allowlists server IPv4/export prefixes.
 * Protocol: request [op:1][host len:u32be][host][path len:u32be][path].
 * Response: E [error len:u32be][error], or O followed by:
 *   S: [is_dir:1][size:u64be][mtime:u64be]
 *   L: repeated [name len:u32be][name][is_dir:1][size:u64be][mtime:u64be], 0 len ends.
 *   R: repeated [chunk len:u32be][bytes], 0 len ends; 0xffffffff len means
 *      [error len:u32be][error] after a partial transfer.
 */
#define _POSIX_C_SOURCE 200809L
#include <nfsc/libnfs.h>
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#define CONF "/etc/pic-nfs-helper.conf"
#define MAX_HOST 253U
#define MAX_PATH 4096U
#define MAX_ENTRIES 100000U
#define MAX_FILE (512ULL * 1024 * 1024)
#define MAX_RULES 64

struct rule { char ip[INET_ADDRSTRLEN]; char prefix[MAX_PATH + 1]; };
static struct rule rules[MAX_RULES];
static size_t rule_count;
static struct nfs_context *nfs;
static char mounted_host[INET_ADDRSTRLEN];

static int read_all(void *dst, size_t len) {
    unsigned char *out = dst;
    while (len) {
        ssize_t count = read(STDIN_FILENO, out, len);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) return 0;
        out += count; len -= (size_t)count;
    }
    return 1;
}
static int write_all(const void *src, size_t len) {
    const unsigned char *p = src;
    while (len) {
        ssize_t count = write(STDOUT_FILENO, p, len);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) return 0;
        p += count; len -= (size_t)count;
    }
    return 1;
}
static int put_u32(uint32_t n) {
    uint32_t net = htonl(n);
    return write_all(&net, sizeof(net));
}
static int get_u32(uint32_t *n) {
    uint32_t net;
    if (!read_all(&net, sizeof(net))) return 0;
    *n = ntohl(net);
    return 1;
}
static int put_u64(uint64_t n) {
    unsigned char b[8];
    for (int i = 7; i >= 0; --i) { b[i] = (unsigned char)n; n >>= 8; }
    return write_all(b, 8);
}
static int fail(const char *message) {
    size_t len = strlen(message);
    if (len > 1024) len = 1024;
    return write_all("E", 1) && put_u32((uint32_t)len) && write_all(message, len);
}
static int lib_fail(const char *name, int rc) {
    char message[1200];
    const char *detail = nfs ? nfs_get_error(nfs) : "nfs context unavailable";
    snprintf(message, sizeof(message), "%s rc=%d libnfs=%s", name, rc,
             (detail && *detail) ? detail : "<empty>");
    return fail(message);
}
static int valid_prefix(const char *p) {
    if (p[0] != '/' || p[1] == 0 || strlen(p) > MAX_PATH) return 0;
    /* Forbid dot components, double slashes and backslashes in allowlist. */
    for (const char *s = p; *s; ++s) {
        if (*s == '\\' || (*s == '/' && s[1] == '/')) return 0;
        if ((*s == '.' && (s == p + 1 || s[-1] == '/') &&
             (s[1] == '/' || !s[1] || (s[1] == '.' && (s[2] == '/' || !s[2]))))) return 0;
    }
    return 1;
}
static int valid_path(const char *p) {
    return valid_prefix(p);
}
static int load_rules(void) {
    /* Prevent a user from substituting a symlink or user-modifiable policy. */
    int fd = open(CONF, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) { fprintf(stderr, "PIC NFS: cannot open %s: %s\n", CONF, strerror(errno)); return 0; }
    struct stat st;
    if (fstat(fd, &st) != 0 || !S_ISREG(st.st_mode) || st.st_uid != 0 || (st.st_mode & 022)) {
        fprintf(stderr, "PIC NFS: config must be root-owned and not group/other writable\n");
        close(fd); return 0;
    }
    FILE *f = fdopen(fd, "r");
    if (!f) { close(fd); return 0; }
    char line[8192];
    while (fgets(line, sizeof(line), f)) {
        char ip[INET_ADDRSTRLEN], prefix[MAX_PATH + 1], extra[2];
        char *s = line;
        while (*s == ' ' || *s == '\t') ++s;
        if (*s == '#' || *s == '\n' || *s == 0) continue;
        if (sscanf(s, "%15s %4096s %1s", ip, prefix, extra) != 2 ||
            rule_count == MAX_RULES || !valid_prefix(prefix)) { rule_count = 0; break; }
        struct in_addr address;
        if (inet_pton(AF_INET, ip, &address) != 1 ||
            !inet_ntop(AF_INET, &address, rules[rule_count].ip, INET_ADDRSTRLEN)) {
            rule_count = 0; break;
        }
        strcpy(rules[rule_count].prefix, prefix);
        ++rule_count;
    }
    int bad = ferror(f) || !feof(f);
    fclose(f);
    if (bad || !rule_count) {
        fprintf(stderr, "PIC NFS: invalid or empty policy in %s\n", CONF);
        return 0;
    }
    return 1;
}
static int path_allowed(const char *ip, const char *path) {
    for (size_t i = 0; i < rule_count; ++i) {
        size_t n = strlen(rules[i].prefix);
        if (!strcmp(ip, rules[i].ip) && !strncmp(path, rules[i].prefix, n) &&
            (!path[n] || path[n] == '/')) return 1;
    }
    return 0;
}
static int resolve_allowed(const char *host, const char *path, char *out) {
    struct addrinfo hints = {0}, *addresses = NULL;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(host, NULL, &hints, &addresses)) return 0;
    int allowed = 0;
    for (struct addrinfo *item = addresses; item; item = item->ai_next) {
        char ip[INET_ADDRSTRLEN];
        struct sockaddr_in *a = (struct sockaddr_in *)item->ai_addr;
        if (inet_ntop(AF_INET, &a->sin_addr, ip, sizeof(ip)) && path_allowed(ip, path)) {
            strcpy(out, ip); allowed = 1; break;
        }
    }
    freeaddrinfo(addresses);
    return allowed;
}
static int prepare_nfs(const char *ip) {
    if (nfs && !strcmp(mounted_host, ip)) return 0;
    if (nfs) { nfs_destroy_context(nfs); nfs = NULL; mounted_host[0] = 0; }
    nfs = nfs_init_context();
    if (!nfs) return -1;
    nfs_set_autoreconnect(nfs, 2);
    nfs_set_retrans(nfs, 2);
    int rc = nfs_set_version(nfs, 4);
    if (rc != 0) return rc;
    rc = nfs_mount(nfs, ip, "/");
    if (rc == 0) strcpy(mounted_host, ip);
    return rc;
}
static int respond_stat(const char *path) {
    struct nfs_stat_64 st = {0};
    int rc = nfs_stat64(nfs, path, &st);
    if (rc < 0) return lib_fail("nfs_stat64", rc);
    unsigned char dir = (st.nfs_mode & S_IFMT) == S_IFDIR;
    return write_all("O", 1) && write_all(&dir, 1) &&
           put_u64(st.nfs_size) && put_u64(st.nfs_mtime);
}
static int respond_list(const char *path) {
    struct nfsdir *dir = NULL;
    int rc = nfs_opendir(nfs, path, &dir);
    if (rc < 0) return lib_fail("nfs_opendir", rc);
    if (!write_all("O", 1)) { nfs_closedir(nfs, dir); return 0; }
    uint32_t count = 0;
    struct nfsdirent *ent;
    while ((ent = nfs_readdir(nfs, dir))) {
        if (!ent->name || !strcmp(ent->name, ".") || !strcmp(ent->name, "..")) continue;
        size_t len = strlen(ent->name);
        if (++count > MAX_ENTRIES || !len || len > 4096) {
            nfs_closedir(nfs, dir);
            const char *message = "NFS directory exceeds response limit";
            return put_u32(UINT32_MAX) && put_u32((uint32_t)strlen(message)) &&
                   write_all(message, strlen(message));
        }
        unsigned char is_dir = ent->type == 2;
        if (!put_u32((uint32_t)len) || !write_all(ent->name, len) ||
            !write_all(&is_dir, 1) || !put_u64(ent->size) ||
            !put_u64((uint64_t)ent->mtime.tv_sec)) {
            nfs_closedir(nfs, dir); return 0;
        }
    }
    nfs_closedir(nfs, dir);
    return put_u32(0);
}
static int respond_read(const char *path) {
    struct nfs_stat_64 st = {0};
    int rc = nfs_stat64(nfs, path, &st);
    if (rc < 0) return lib_fail("nfs_stat64 before read", rc);
    if ((st.nfs_mode & S_IFMT) != S_IFREG) return fail("NFS read requires a regular file");
    if (st.nfs_size > MAX_FILE) return fail("NFS file exceeds 512 MiB limit");
    struct nfsfh *fh = NULL;
    rc = nfs_open(nfs, path, O_RDONLY, &fh);
    if (rc < 0) return lib_fail("nfs_open(O_RDONLY)", rc);
    if (!write_all("O", 1)) { nfs_close(nfs, fh); return 0; }
    unsigned char buf[64 * 1024];
    uint64_t total = 0;
    while (1) {
        rc = nfs_read(nfs, fh, buf, sizeof(buf));
        if (rc <= 0) break;
        total += (unsigned)rc;
        if (total > MAX_FILE) {
            nfs_close(nfs, fh);
            const char *message = "NFS file exceeds 512 MiB limit";
            return put_u32(UINT32_MAX) && put_u32((uint32_t)strlen(message)) &&
                   write_all(message, strlen(message));
        }
        if (!put_u32((uint32_t)rc) || !write_all(buf, (size_t)rc)) {
            nfs_close(nfs, fh); return 0;
        }
    }
    if (rc < 0) {
        char message[1200];
        const char *detail = nfs_get_error(nfs);
        snprintf(message, sizeof(message), "nfs_read rc=%d libnfs=%s", rc,
                 (detail && *detail) ? detail : "<empty>");
        nfs_close(nfs, fh);
        size_t len = strlen(message);
        return put_u32(UINT32_MAX) && put_u32((uint32_t)len) && write_all(message, len);
    }
    nfs_close(nfs, fh);
    return put_u32(0);
}
int main(void) {
    signal(SIGPIPE, SIG_IGN);
    if (!load_rules()) return 2;
    while (1) {
        unsigned char op;
        uint32_t host_len, path_len;
        char host[MAX_HOST + 1], path[MAX_PATH + 1], ip[INET_ADDRSTRLEN];
        if (!read_all(&op, 1)) break;
        if (!get_u32(&host_len) || !host_len || host_len > MAX_HOST) break;
        if (!read_all(host, host_len)) break;
        host[host_len] = 0;
        if (!get_u32(&path_len) || !path_len || path_len > MAX_PATH) break;
        if (!read_all(path, path_len)) break;
        path[path_len] = 0;
        if (memchr(host, 0, host_len) || memchr(path, 0, path_len) ||
            !valid_path(path) || (op != 'S' && op != 'L' && op != 'R')) {
            if (!fail("Invalid helper request")) break;
            continue;
        }
        if (!resolve_allowed(host, path, ip)) {
            if (!fail("NFS server/export not permitted by root-owned helper policy")) break;
            continue;
        }
        int rc = prepare_nfs(ip);
        if (rc) {
            if (!lib_fail("NFSv4 mount", rc)) break;
            continue;
        }
        int ok = (op == 'S') ? respond_stat(path) :
                 (op == 'L') ? respond_list(path) : respond_read(path);
        if (!ok) break;
    }
    if (nfs) nfs_destroy_context(nfs);
    return 0;
}
