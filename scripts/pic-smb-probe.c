/* Standalone share/subfolder probe that mirrors PIC's direct SMB transport
 * (native/private_smb.c): libsmbclient with a guest auth callback, opened with
 * smbc_opendir + smbc_readdir, no gvfs, no mounts, same error reporting. */
#include <libsmbclient.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <limits.h>
#include <netdb.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <unistd.h>
#include <pwd.h>
#include <ifaddrs.h>
#include <netinet/in.h>
#include <net/if.h>
#include <poll.h>
#include <fcntl.h>

static char custom_user[256] = {0};
static char custom_pass[256] = {0};
static char custom_workgroup[256] = {0};
static int anonymous = 0;

static int trace_enabled(void) {
    const char *value = getenv("PICASA_TRACE");
    return value && *value;
}

/* Default when no -u/-a is given: current OS username with an empty password,
 * exactly what PIC's native/private_smb.c guest_auth now does (and what
 * smbclient -N / GNOME's accepted "cancel" send). -u/-p overrides with given
 * credentials; -a forces a true empty-username anonymous session. */
static void pic_auth(const char *server, const char *share,
                     char *workgroup, int wglen,
                     char *username, int unlen,
                     char *password, int pwlen) {
    (void)server; (void)share;
    if (anonymous) {
        if (unlen > 0) username[0] = '\0';
        if (pwlen > 0) password[0] = '\0';
        if (trace_enabled()) fprintf(stderr, "PIC_SMB_AUTH user=<anonymous> share=%s\n",
                                     share ? share : "-");
    } else if (custom_user[0]) {
        if (unlen > 0) snprintf(username, (size_t)unlen, "%s", custom_user);
        if (pwlen > 0) snprintf(password, (size_t)pwlen, "%s", custom_pass);
        if (wglen > 0)
            snprintf(workgroup, (size_t)wglen, "%s",
                     custom_workgroup[0] ? custom_workgroup : "WORKGROUP");
        if (trace_enabled()) {
            fprintf(stderr, "PIC_SMB_AUTH user=%s workgroup=%s share=%s\n",
                    username, workgroup, share ? share : "-");
        }
    } else {
        if (unlen > 0) {
            struct passwd *pw = getpwuid(getuid());
            snprintf(username, (size_t)unlen, "%s",
                     pw && pw->pw_name ? pw->pw_name : "guest");
        }
        if (pwlen > 0) password[0] = '\0';
        if (trace_enabled()) fprintf(stderr, "PIC_SMB_AUTH user=%s\n", username);
    }
}

static int initialized = 0;
static int init_smb(const char *error_label, char *error, size_t cap) {
    if (!initialized) {
        if (trace_enabled()) fprintf(stderr, "PIC_SMB_INIT smbc_init start\n");
        if (smbc_init(pic_auth, 0) != 0) {
            snprintf(error, cap, "%s: smbc_init: %s", error_label, strerror(errno));
            return -1;
        }
        initialized = 1;
        if (trace_enabled()) fprintf(stderr, "PIC_SMB_INIT smbc_init ok\n");
    }
    return 0;
}

static int failures = 0;

static void walk(const char *uri, int depth, int max_depth, int max_entries) {
    char error[512];
    if (init_smb("probe", error, sizeof error)) {
        printf("%*s%s\n", depth * 2, "", error);
        failures++;
        return;
    }
    if (trace_enabled()) fprintf(stderr, "PIC_SMB_OPENDIR uri=%s\n", uri);
    int fd = smbc_opendir(uri);
    if (fd < 0) {
        printf("%*s[DENIED] %s  ->  %s (errno=%d)\n",
               depth * 2, "", uri, strerror(errno), errno);
        failures++;
        return;
    }
    struct smbc_dirent *entry;
    int count = 0;
    errno = 0;
    while ((entry = smbc_readdir(fd)) != NULL) {
        if (strcmp(entry->name, ".") == 0 || strcmp(entry->name, "..") == 0)
            continue;
        if (count >= max_entries) { printf("%*s… (beyond %d entries)\n", depth * 2, "", max_entries); break; }
        count++;
        const char *kind = entry->smbc_type == SMBC_DIR ? "dir " :
                           entry->smbc_type == SMBC_FILE ? "file" :
                           entry->smbc_type == SMBC_WORKGROUP ? "wg  " :
                           entry->smbc_type == SMBC_SERVER ? "srv " :
                           entry->smbc_type == SMBC_FILE_SHARE ? "shr " : "oth ";
        printf("%*s [%s] %.180s\n", depth * 2, "", kind, entry->name);
        if (entry->smbc_type == SMBC_DIR && depth < max_depth) {
            char child[4096];
            int pathlen = snprintf(child, sizeof child, "%s/%s", uri, entry->name);
            if (pathlen < (int)sizeof child)
                walk(child, depth + 1, max_depth, max_entries);
        }
        errno = 0;
    }
    int saved = errno;
    smbc_closedir(fd);
    if (saved) {
        printf("%*s[PARTIAL] %s  ->  %s (errno=%d) mid-listing\n",
               depth * 2, "", uri, strerror(saved), saved);
        failures++;
    } else if (trace_enabled()) {
        fprintf(stderr, "PIC_SMB_OPENDIR uri=%s entries=%d\n", uri, count);
    }
}

/* Automatic discovery: probe the local subnet for servers with SMB (445) or
 * NFS (2049) open, then list each SMB server's shares. One batched
 * non-blocking connect pass, so the whole /24 scan takes ~1.5 seconds. */
#define SCAN_HOSTS 254
static int detect_subnet_prefix(char *prefix, size_t cap) {
    struct ifaddrs *ifaddr = NULL;
    if (getifaddrs(&ifaddr) != 0) {
        fprintf(stderr, "getifaddrs failed: %s\n", strerror(errno));
        return -1;
    }
    int found = 0;
    for (struct ifaddrs *ifa = ifaddr; ifa && !found; ifa = ifa->ifa_next) {
        if (!ifa->ifa_addr || ifa->ifa_addr->sa_family != AF_INET)
            continue;
        if (ifa->ifa_flags & IFF_LOOPBACK)
            continue;
        struct sockaddr_in *sa = (struct sockaddr_in *)ifa->ifa_addr;
        unsigned char *octet = (unsigned char *)&sa->sin_addr;
        if (octet[0] == 169 && octet[1] == 254)  /* link-local */
            continue;
        snprintf(prefix, cap, "%u.%u.%u.", octet[0], octet[1], octet[2]);
        fprintf(stderr, "Scanning subnet %s (interface %s)\n",
                prefix, ifa->ifa_name);
        found = 1;
    }
    freeifaddrs(ifaddr);
    return found ? 0 : -1;
}

static void scan_subnet(const char *prefix, int max_depth, int max_entries) {
    struct {
        int fd;
        char ip[16];
        int port;
    } pending[SCAN_HOSTS * 2];
    struct pollfd fds[SCAN_HOSTS * 2];
    int ports[] = {445, 2049};
    int count = 0;
    char ip[16];

    for (int i = 1; i <= SCAN_HOSTS; i++) {
        snprintf(ip, sizeof ip, "%s%d", prefix, i);
        for (int p = 0; p < 2; p++) {
            int fd = socket(AF_INET, SOCK_STREAM, 0);
            if (fd < 0) continue;
            int flags = fcntl(fd, F_GETFL, 0);
            fcntl(fd, F_SETFL, flags | O_NONBLOCK);
            struct sockaddr_in sa;
            memset(&sa, 0, sizeof sa);
            sa.sin_family = AF_INET;
            sa.sin_port = htons((uint16_t)ports[p]);
            if (inet_pton(AF_INET, ip, &sa.sin_addr) != 1) { close(fd); continue; }
            if (connect(fd, (struct sockaddr *)&sa, sizeof sa) != 0 &&
                errno != EINPROGRESS) { close(fd); continue; }
            pending[count].fd = fd;
            snprintf(pending[count].ip, sizeof pending[count].ip, "%s", ip);
            pending[count].port = ports[p];
            fds[count].fd = fd;
            fds[count].events = POLLOUT;
            fds[count].revents = 0;
            count++;
        }
    }

    int ready = poll(fds, (nfds_t)count, 1500);
    if (ready < 0) {
        fprintf(stderr, "poll failed: %s\n", strerror(errno));
    } else {
        int smb_hosts = 0, nfs_hosts = 0;
        char last_ip[16] = "";
        for (int n = 0; n < count; n++) {
            if (!(fds[n].revents & (POLLOUT | POLLERR))) continue;
            int error = 0;
            socklen_t len = sizeof error;
            if (getsockopt(fds[n].fd, SOL_SOCKET, SO_ERROR, &error, &len) != 0 || error != 0)
                continue;
            if (pending[n].port == 445) {
                if (strcmp(pending[n].ip, last_ip) != 0) {
                    last_ip[0] = '\0';
                    printf("\n==== SMB server %s ====\n", pending[n].ip);
                    last_ip[0] = '\0';
                    snprintf(last_ip, sizeof last_ip, "%s", pending[n].ip);
                    smb_hosts++;
                    char root[64];
                    snprintf(root, sizeof root, "smb://%.15s/", pending[n].ip);
                    walk(root, 0, max_depth, max_entries);
                }
            } else {
                printf("\n[NFS server] %s  (list exports: showmount -e %s)\n",
                       pending[n].ip, pending[n].ip);
                nfs_hosts++;
            }
        }
        printf("\nScan complete: %d SMB server(s), %d NFS server(s).\n",
               smb_hosts, nfs_hosts);
    }
    for (int n = 0; n < count; n++) close(fds[n].fd);
}

static void usage(const char *prog) {
    fprintf(stderr,
        "Usage: %s [options] [--scan [a.b.c]] [<host>[/share[/path]]]\n"
        "\n"
        "With <host>  walks SMB shares and subfolders using PIC's exact transport\n"
        "             (libsmbclient; default auth = current OS user + empty password,\n"
        "             like smbclient -N / accepting the GNOME auth prompt).\n"
        "With --scan  automatically finds SMB/NFS servers on the local subnet\n"
        "             (or the given a.b.c prefix) and lists each SMB server's shares.\n"
        "With no arguments, runs --scan.\n"
        "\n"
        "Options:\n"
        "  -u <user>     username (default: current OS user with empty password, like the app)\n"
        "  -p <pass>     password\n"
        "  -a            anonymous (empty username - true null session)\n"
        "  -w <workgroup> workgroup (default WORKGROUP)\n"
        "  -d <depth>    max recursion depth for subfolders (default 2, 0=shares only)\n"
        "  -n <count>    stop listing a folder after N entries (default 200)\n"
        "  -h            this help\n"
        "\n"
        "Examples:\n"
        "  %s                      # auto-find shares on the local subnet\n"
        "  %s --scan 10.0.0        # scan an explicit /24\n"
        "  %s Ella.local           # list a host's shares\n"
        "  %s 10.0.0.2/Photos      # list inside one share\n"
        "  %s -u peet -p changeme Ella.local/Documents 2>&1 | less\n",
        prog, prog, prog, prog, prog, prog);
}

int main(int argc, char **argv) {
    const char *positionals[4] = {0};
    int positional_count = 0;
    int scan_mode = 0;
    int max_depth = 2;
    int max_entries = 200;
    int i = 1;
    for (; i < argc; i++) {
        if (argv[i][0] != '-' || argv[i][1] == '\0') {
            if (positional_count < 4) positionals[positional_count++] = argv[i];
            continue;
        }
        if (strcmp(argv[i], "--scan") == 0) { scan_mode = 1; }
        else if (strcmp(argv[i], "-u") == 0 && i + 1 < argc) { strncpy(custom_user, argv[++i], sizeof custom_user - 1); }
        else if (strcmp(argv[i], "-p") == 0 && i + 1 < argc) { strncpy(custom_pass, argv[++i], sizeof custom_pass - 1); }
        else if (strcmp(argv[i], "-w") == 0 && i + 1 < argc) { strncpy(custom_workgroup, argv[++i], sizeof custom_workgroup - 1); }
        else if (strcmp(argv[i], "-d") == 0 && i + 1 < argc) { max_depth = atoi(argv[++i]); }
        else if (strcmp(argv[i], "-n") == 0 && i + 1 < argc) { max_entries = atoi(argv[++i]); }
        else if (strcmp(argv[i], "-a") == 0) { anonymous = 1; }
        else { usage(argv[0]); return 2; }
    }
    const char *target = scan_mode ? NULL : (positional_count > 0 ? positionals[0] : NULL);
    const char *scan_prefix = scan_mode && positional_count > 0 ? positionals[0] : NULL;
    if (!target && !scan_mode) scan_mode = 1;

    char auto_prefix[24] = {0};
    if (scan_mode) {
        char prefix[24] = {0};
        if (scan_prefix) snprintf(prefix, sizeof prefix, "%s", scan_prefix);
        else if (detect_subnet_prefix(prefix, sizeof prefix) != 0) {
            fprintf(stderr, "Cannot detect the local subnet; pass one: %s --scan a.b.c\n", argv[0]);
            return 2;
        }
        if (strstr(prefix, ".")) snprintf(auto_prefix, sizeof auto_prefix, "%s", prefix);
        fprintf(stderr, "Probing for SMB/NFS servers on %s.hosts (user=%s)\n",
                auto_prefix[0] ? auto_prefix : prefix,
                anonymous ? "<anonymous>" : (custom_user[0] ? custom_user : "<os-user/empty-pass>"));
        scan_subnet(auto_prefix[0] ? auto_prefix : prefix, max_depth, max_entries);
        return failures ? 1 : 0;
    }

    /* Normalize like the picker: a bare host defaults to smb://host/. */
    char uri[4096];
    if (strncmp(target, "smb://", 6) != 0) {
        const char *t = target;
        if (strncmp(t, "\\\\", 2) == 0 || strncmp(t, "//", 2) == 0) t += 2;
        snprintf(uri, sizeof uri, "smb://%s/", t);
    } else {
        snprintf(uri, sizeof uri, "%s", target);
    }

    /* Mirror resolved_uri in network_shares.rs: the app resolves .local
     * names to a numeric IPv4 address through Avahi before libsmbclient.
     * Prefer IPv4 so the probe never lets libsmbclient pick an IPv6
     * link-local address. */
    char *host_start = strstr(uri, "://");
    if (host_start) {
        host_start += 3;
        char *host_end = strchr(host_start, '/');
        size_t host_len = host_end ? (size_t)(host_end - host_start) : strlen(host_start);
        char host[256];
        if (host_len < sizeof host) {
            memcpy(host, host_start, host_len);
            host[host_len] = '\0';
            int is_local = strlen(host) > 6 &&
                           strcasecmp(host + host_len - 6, ".local") == 0;
            struct addrinfo hints = {0}, *res = NULL;
            hints.ai_family = AF_INET;
            hints.ai_socktype = SOCK_STREAM;
            if (is_local && getaddrinfo(host, NULL, &hints, &res) == 0 && res) {
                char ip[INET_ADDRSTRLEN] = {0};
                struct sockaddr_in *sa = (struct sockaddr_in *)res->ai_addr;
                inet_ntop(AF_INET, &sa->sin_addr, ip, sizeof ip);
                fprintf(stderr, "Resolved %s -> %s%s\n", host, ip,
                        host_end && *host_end ? host_end : "/");
                memmove(host_start, ip, strlen(ip));
                if (host_end) memmove(host_start + strlen(ip), host_end, strlen(host_end) + 1);
                else strcpy(host_start + strlen(ip), "/");
                freeaddrinfo(res);
            } else if (is_local) {
                fprintf(stderr, "Could not resolve %s via DNS (falling back to libsmbclient resolution)\n", host);
            }
        }
    }

    fprintf(stderr, "Probing %s  (user=%s)\n", uri,
            anonymous ? "<anonymous>" : (custom_user[0] ? custom_user : "<os-user/empty-pass>"));
    walk(uri, 0, max_depth, max_entries);
    printf(failures ? "\n%d access failure(s)\n" : "\nAll folders readable.\n", failures);
    return failures ? 1 : 0;
}