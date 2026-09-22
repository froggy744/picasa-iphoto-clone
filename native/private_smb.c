/* Samba libsmbclient: no GIO mount, no Nautilus mount, no shell commands. */
#include <libsmbclient.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <pthread.h>
#include <unistd.h>
#include <pwd.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <fcntl.h>
#include <netdb.h>

static int trace_enabled(void);

/* Default credentials when a server does not prompt for a login. Mirror what
 * smbclient -N and GNOME's accepted "cancel" do: the current OS account with
 * an empty password. Windows file servers commonly deny the literal "guest"
 * account while accepting an anonymous session under the caller's local
 * identity, so the naive guest fallback produced spurious "access denied". */
static void guest_auth(const char *server, const char *share, char *workgroup, int wglen,
                       char *username, int unlen, char *password, int pwlen) {
    (void)server; (void)share; (void)workgroup; (void)wglen;
    if (unlen > 0) {
        struct passwd *pw = getpwuid(getuid());
        const char *fallback = "guest";
        snprintf(username, (size_t)unlen, "%s", pw && pw->pw_name ? pw->pw_name : fallback);
    }
    if (pwlen > 0) password[0] = '\0';
    if (trace_enabled()) fprintf(stderr, "PIC_SMB_AUTH user=%s\n", username);
}

/* The legacy smbc_* API owns one process-wide client context. Keep calls
 * serialized because the context and its connection cache are not safe for
 * concurrent use; do not reinitialize it for each image read. */
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int initialized = 0;
static int trace_enabled(void) {
    const char *value = getenv("PICASA_TRACE");
    return value && *value;
}

static int init_smb(char *error, size_t cap) {
    if (!initialized) {
        if (trace_enabled()) fprintf(stderr, "PIC_SMB_CONNECT create_start\n");
        if (smbc_init(guest_auth, 0) != 0) {
            snprintf(error, cap, "smbc_init: %s", strerror(errno)); return -1;
        }
        initialized = 1;
        if (trace_enabled()) fprintf(stderr, "PIC_SMB_CONNECT create_ok\n");
    } else if (trace_enabled()) {
        fprintf(stderr, "PIC_SMB_CONNECT reuse\n");
    }
    return 0;
}
static void fail(char *error, size_t cap, const char *operation) {
    snprintf(error, cap, "%s: %s (errno=%d)", operation, strerror(errno), errno);
}
typedef int (*entry_callback)(void *context, const char *name, unsigned int kind);
int pic_smb_list(const char *uri, entry_callback cb, void *context, char *error, size_t cap) {
    pthread_mutex_lock(&lock);
    if (init_smb(error, cap)) { pthread_mutex_unlock(&lock); return -1; }
    int fd = smbc_opendir(uri);
    if (fd < 0) { fail(error, cap, "smbc_opendir"); pthread_mutex_unlock(&lock); return -1; }
    struct smbc_dirent *ent;
    int count=0;
    errno = 0;
    while ((ent=smbc_readdir(fd)) != NULL) {
        if (strcmp(ent->name,".")==0 || strcmp(ent->name,"..")==0) continue;
        if (cb(context, ent->name, ent->smbc_type) != 0) break;
        count++;
        errno=0;
    }
    int saved_errno=errno;
    smbc_closedir(fd);
    if (saved_errno) { errno=saved_errno; fail(error,cap,"smbc_readdir"); pthread_mutex_unlock(&lock); return -1; }
    pthread_mutex_unlock(&lock);
    return count;
}

/* Automatic SMB/NFS server discovery on a subnet (e.g. "10.0.0"), or the
 * local subnet when prefix is empty/NULL. One batched non-blocking connect
 * pass for ports 445 and 2049; each reachable host's IP, kind (3 = SMB,
 * 7 = NFS) and best-effort reverse-DNS PC name reach the callback. Does not
 * mount or authenticate, so it is safe to run from the picker thread. */
typedef int (*scan_host_callback)(void *context, const char *ip,
                                  unsigned int kind, const char *hostname);
int pic_smb_scan_hosts(const char *prefix, scan_host_callback cb, void *context,
                       char *error, size_t cap) {
    char base[24] = {0};
    if (prefix && prefix[0]) {
        snprintf(base, sizeof base, "%s", prefix);
        size_t len = strlen(base);
        if (len && base[len - 1] != '.') {
            if (len < sizeof base - 1) { base[len] = '.'; base[len + 1] = '\0'; }
        }
    } else {
        struct ifaddrs *ifaddr = NULL;
        if (getifaddrs(&ifaddr) != 0) {
            snprintf(error, cap, "getifaddrs: %s", strerror(errno));
            return -1;
        }
        for (struct ifaddrs *ifa = ifaddr; ifa; ifa = ifa->ifa_next) {
            if (!ifa->ifa_addr || ifa->ifa_addr->sa_family != AF_INET) continue;
            if (ifa->ifa_flags & IFF_LOOPBACK) continue;
            unsigned char *octet = (unsigned char *)&((struct sockaddr_in *)ifa->ifa_addr)->sin_addr;
            if (octet[0] == 169 && octet[1] == 254) continue;
            snprintf(base, sizeof base, "%u.%u.%u.", octet[0], octet[1], octet[2]);
            break;
        }
        freeifaddrs(ifaddr);
        if (!base[0]) {
            snprintf(error, cap, "Could not detect a local subnet");
            return -1;
        }
    }
    if (trace_enabled()) fprintf(stderr, "PIC_SMB_SCAN start base=%s\n", base);

    int fds[254 * 2];
    struct pollfd pfd[254 * 2];
    int socket_port[254 * 2];
    int count = 0;
    for (int i = 1; i <= 254; i++) {
        char ip[16];
        snprintf(ip, sizeof ip, "%s%d", base, i);
        for (int p = 0; p < 2; p++) {
            int port = p == 0 ? 445 : 2049;
            int fd = socket(AF_INET, SOCK_STREAM, 0);
            if (fd < 0) continue;
            int flags = fcntl(fd, F_GETFL, 0);
            fcntl(fd, F_SETFL, flags | O_NONBLOCK);
            struct sockaddr_in sa;
            memset(&sa, 0, sizeof sa);
            sa.sin_family = AF_INET;
            sa.sin_port = htons((uint16_t)port);
            if (inet_pton(AF_INET, ip, &sa.sin_addr) != 1) { close(fd); continue; }
            if (connect(fd, (struct sockaddr *)&sa, sizeof sa) != 0 && errno != EINPROGRESS) {
                close(fd); continue;
            }
            fds[count] = fd;
            pfd[count].fd = fd;
            pfd[count].events = POLLOUT;
            pfd[count].revents = 0;
            socket_port[count] = port;
            count++;
        }
    }

    char smb_up[255] = {0}, nfs_up[255] = {0};
    int ready = poll(pfd, (nfds_t)count, 1500);
    if (ready < 0 && errno != EINTR) {
        snprintf(error, cap, "poll: %s", strerror(errno));
    } else if (ready > 0) {
        for (int n = 0; n < count; n++) {
            if (!(pfd[n].revents & (POLLOUT | POLLERR))) continue;
            int sock_error = 0;
            socklen_t sl = sizeof sock_error;
            if (getsockopt(fds[n], SOL_SOCKET, SO_ERROR, &sock_error, &sl) != 0 || sock_error != 0)
                continue;
            char ip[16];
            /* Recover host index from the socket's peer address. */
            struct sockaddr_in peer;
            socklen_t plen = sizeof peer;
            if (getpeername(fds[n], (struct sockaddr *)&peer, &plen) != 0) continue;
            unsigned char *octet = (unsigned char *)&peer.sin_addr;
            snprintf(ip, sizeof ip, "%u.%u.%u.%u", octet[0], octet[1], octet[2], octet[3]);
            int index = octet[3];
            if (index >= 1 && index <= 254) {
                if (socket_port[n] == 445) smb_up[index] = 1;
                else nfs_up[index] = 1;
            }
            (void)ip;
        }
    }
    for (int n = 0; n < count; n++) close(fds[n]);

    int found = 0;
    for (int index = 1; index <= 254; index++) {
        if (!smb_up[index] && !nfs_up[index]) continue;
        char ip[16];
        snprintf(ip, sizeof ip, "%s%d", base, index);
        /* Best-effort PC name from reverse DNS (works for DHCP-registered
         * Windows hosts and mDNS-resolving .local/.lan names). */
        char hostname[NI_MAXHOST] = "";
        struct sockaddr_in peer;
        memset(&peer, 0, sizeof peer);
        peer.sin_family = AF_INET;
        peer.sin_port = 0;
        if (inet_pton(AF_INET, ip, &peer.sin_addr) == 1) {
            socklen_t peer_len = sizeof peer;
            if (getnameinfo((struct sockaddr *)&peer, peer_len, hostname,
                            sizeof hostname, NULL, 0, NI_NAMEREQD) != 0) {
                hostname[0] = '\0';
            }
        }
        /* Report one entry per open service: a host with both SMB and NFS
         * advertises shares AND exports. */
        if (smb_up[index]) {
            if (cb(context, ip, 3, hostname[0] ? hostname : ip) != 0) break;
            found++;
        }
        if (nfs_up[index]) {
            if (cb(context, ip, 7, hostname[0] ? hostname : ip) != 0) break;
            found++;
        }
    }
    if (trace_enabled()) fprintf(stderr, "PIC_SMB_SCAN done hosts=%d base=%s\n", found, base);
    return found;
}
/* C-allocated bytes; Rust must release them with pic_smb_free. */
int pic_smb_read(const char *uri, unsigned char **out, size_t *length, size_t max_bytes,
                 char *error, size_t cap) {
    *out=NULL; *length=0;
    pthread_mutex_lock(&lock);
    if (init_smb(error,cap)) { pthread_mutex_unlock(&lock); return -1; }
    if (trace_enabled()) fprintf(stderr, "PIC_SMB_READ start\n");
    int fd=smbc_open(uri,O_RDONLY,0);
    if (fd < 0) { fail(error,cap,"smbc_open"); pthread_mutex_unlock(&lock); return -1; }
    size_t allocated=64*1024, used=0;
    unsigned char *bytes=malloc(allocated);
    if (!bytes) { snprintf(error,cap,"Out of memory"); smbc_close(fd); pthread_mutex_unlock(&lock); return -1; }
    int failed=0;
    for (;;) {
        if (used==allocated) {
            if (allocated>=max_bytes) { snprintf(error,cap,"File exceeds maximum thumbnail input size"); failed=1; break; }
            size_t next=allocated*2; if (next>max_bytes) next=max_bytes;
            unsigned char *b=realloc(bytes,next);
            if (!b) { snprintf(error,cap,"Out of memory"); failed=1; break; }
            bytes=b; allocated=next;
        }
        ssize_t got=smbc_read(fd,bytes+used,allocated-used);
        if (got<0) { fail(error,cap,"smbc_read"); failed=1; break; }
        if (got==0) break;
        used+=(size_t)got;
    }
    smbc_close(fd);
    pthread_mutex_unlock(&lock);
    if (failed) { free(bytes); return -1; }
    *out=bytes; *length=used;
    if (trace_enabled()) fprintf(stderr, "PIC_SMB_READ done bytes=%zu\n", used);
    return 0;
}
void pic_smb_free(void *bytes) { free(bytes); }

int pic_smb_read_range(const char *uri, uint64_t offset, size_t requested,
                       unsigned char **out, size_t *length, char *error, size_t cap) {
    *out=NULL; *length=0;
    pthread_mutex_lock(&lock);
    if (init_smb(error,cap)) { pthread_mutex_unlock(&lock); return -1; }
    int fd=smbc_open(uri,O_RDONLY,0);
    if (fd < 0) { fail(error,cap,"smbc_open"); pthread_mutex_unlock(&lock); return -1; }
    if (smbc_lseek(fd,(off_t)offset,SEEK_SET) < 0) { fail(error,cap,"smbc_lseek"); smbc_close(fd); pthread_mutex_unlock(&lock); return -1; }
    unsigned char *bytes=malloc(requested ? requested : 1);
    if (!bytes) { snprintf(error,cap,"Out of memory"); smbc_close(fd); pthread_mutex_unlock(&lock); return -1; }
    size_t used=0;
    while (used < requested) {
        ssize_t got=smbc_read(fd,bytes+used,requested-used);
        if (got < 0) { fail(error,cap,"smbc_read"); free(bytes); smbc_close(fd); pthread_mutex_unlock(&lock); return -1; }
        if (got == 0) break;
        used+=(size_t)got;
    }
    smbc_close(fd); pthread_mutex_unlock(&lock);
    *out=bytes; *length=used;
    if (trace_enabled()) fprintf(stderr,"PIC_SMB_RANGE offset=%llu requested=%zu received=%zu\n",(unsigned long long)offset,requested,used);
    return 0;
}

int pic_smb_stat(const char *uri, uint64_t *size, int64_t *mtime,
                 int *is_dir, char *error, size_t cap) {
    pthread_mutex_lock(&lock);
    if (init_smb(error,cap)) {pthread_mutex_unlock(&lock);return -1;}
    struct stat st={0};
    int result=smbc_stat(uri,&st);
    if(result) fail(error,cap,"smbc_stat");
    else { *size=(uint64_t)st.st_size; *mtime=(int64_t)st.st_mtime;
           *is_dir=S_ISDIR(st.st_mode); }
    pthread_mutex_unlock(&lock);
    return result? -1:0;
}
