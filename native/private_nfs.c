/* Direct userspace NFS backend. nfs_mount is libnfs's session setup,
 * NOT a Linux kernel mount nor a GVfs mount. Read-only operations only. */
#define _DEFAULT_SOURCE

#include <stddef.h>
#include <nfsc/libnfs.h>
#include <nfsc/libnfs-raw.h>
#include <nfsc/libnfs-raw-mount.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <errno.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>
#include <limits.h>
#include <pthread.h>
#include <time.h>
#include <netdb.h>
#include <unistd.h>

typedef int (*pic_entry_cb)(void *, const char *, unsigned int);
static int trace_enabled(void) {
    const char *value = getenv("PICASA_TRACE");
    return value && *value;
}
static uint64_t monotonic_ms(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}
static void trace_stage(const char *stage, uint64_t started, const char *host,
                        const char *export_path, const char *relative,
                        const char *detail) {
    if (!trace_enabled()) return;
    fprintf(stderr, "PIC_NFS stage=%s elapsed_ms=%llu tid=%lu host=%s export=%s relative=%s%s%s\n",
            stage, (unsigned long long)(monotonic_ms() - started),
            (unsigned long)pthread_self(), host ? host : "-",
            export_path ? export_path : "-", relative ? relative : "-",
            detail ? " " : "", detail ? detail : "");
}
static void trace_hostname_resolution(const char *host) {
    if (!trace_enabled()) return;
    uint64_t started = monotonic_ms();
    struct addrinfo hints = {0}, *results = NULL;
    hints.ai_socktype = SOCK_STREAM;
    int status = getaddrinfo(host, NULL, &hints, &results);
    int count = 0;
    for (struct addrinfo *entry = results; entry; entry = entry->ai_next) count++;
    char detail[64];
    snprintf(detail, sizeof detail, "status=%d addresses=%d", status, count);
    trace_stage("hostname_resolution", started, host, NULL, NULL, detail);
    if (results) freeaddrinfo(results);
}
static void err(char *dst, size_t cap, const char *op, struct nfs_context *nfs) {
    const char *detail = nfs ? nfs_get_error(nfs) : NULL;
    snprintf(dst, cap, "%s: %s", op,
             detail && *detail ? detail : "libnfs returned no error details");
}
/* libnfs defaults to v3 and can silently retry v4, obscuring the first
 * failure. Make each attempt explicit and preserve BOTH diagnostics. This
 * session stays inside this process; it never creates a GVfs/kernel mount. */
static struct nfs_context *open_session(const char *host, const char *export_path,
                                        char *error, size_t cap) {
    char attempts[768] = {0};
    for (int version = 3; version <= 4; version++) {
        uint64_t context_started = monotonic_ms();
        struct nfs_context *nfs = nfs_init_context();
        trace_stage("context_creation", context_started, host, export_path, NULL,
                    nfs ? "outcome=ok" : "outcome=error");
        if (!nfs) {
            snprintf(error, cap, "nfs_init_context failed for %s", host);
            return NULL;
        }
        int selected = nfs_set_version(nfs, version);
        if (selected != 0) {
            snprintf(error, cap, "nfs_set_version(%d) failed: status=%d", version, selected);
            nfs_destroy_context(nfs);
            return NULL;
        }
        /* Disable endless reconnection during the initial connection probe. */
        nfs_set_autoreconnect(nfs, 0);
        char session_detail[128];
        snprintf(session_detail, sizeof session_detail,
                 "version=%d uid=%ld gid=%ld source_port=libnfs_default",
                 version, (long)geteuid(), (long)getegid());
        trace_stage("session_configuration", context_started, host, export_path, NULL,
                    session_detail);
        uint64_t connect_started = monotonic_ms();
        if (trace_enabled()) fprintf(stderr, "PIC_NFS_CONNECT create_start host=%s export=%s version=%d tid=%lu\n",
                                     host, export_path, version, (unsigned long)pthread_self());
        int status = nfs_mount(nfs, host, export_path);
        char connect_detail[64];
        snprintf(connect_detail, sizeof connect_detail, "version=%d outcome=%s status=%d",
                 version, status == 0 ? "ok" : "error", status);
        trace_stage("connection_establishment", connect_started, host, export_path, NULL,
                    connect_detail);
        if (status == 0) {
            if (trace_enabled()) fprintf(stderr, "PIC_NFS_CONNECT create_ok host=%s export=%s version=%d\n",
                                         host, export_path, version);
            return nfs;
        }
        const char *detail = nfs_get_error(nfs);
        char line[384];
        snprintf(line, sizeof line, "%sv%d status=%d (%s)%s%s",
                 attempts[0] ? "; " : "", version, status,
                 status < 0 && -status < 4096 ? strerror(-status) : "unknown status",
                 detail && *detail ? ": " : "", detail && *detail ? detail : "");
        size_t used = strlen(attempts);
        if (used < sizeof(attempts) - 1)
            snprintf(attempts + used, sizeof(attempts) - used, "%s", line);
        if (trace_enabled()) fprintf(stderr, "PIC_NFS_CONNECT create_failed host=%s export=%s %s\n",
                                     host, export_path, line);
        nfs_destroy_context(nfs);
    }
    snprintf(error, cap,
             "NFS session failed host=%s export=%s; %.250s; if the NAS requires reserved client source ports, add the server-side 'insecure' export option for this read-only export",
             host, export_path, attempts);
    return NULL;
}
int pic_nfs_exports(const char *host, pic_entry_cb cb, void *ctx,
                    char *error, size_t cap) {
    trace_hostname_resolution(host);
    uint64_t discovery_started = monotonic_ms();
    struct exportnode *list = mount_getexports(host);
    if (!list) {
        trace_stage("export_discovery", discovery_started, host, NULL, NULL, "outcome=error");
        snprintf(error, cap, "NFS export discovery failed for %s (check rpcbind/mountd and NFSv3)", host); return -1;
    }
    int total=0;
    for (struct exportnode *e=list;e;e=e->ex_next) {
        if (e->ex_dir && cb(ctx,e->ex_dir,7) != 0) break;
        total++;
    }
    mount_free_export_list(list);
    char detail[48];
    snprintf(detail, sizeof detail, "outcome=ok exports=%d", total);
    trace_stage("export_discovery", discovery_started, host, NULL, NULL, detail);
    return total;
}
int pic_nfs_list(const char *host, const char *export_path, const char *relative,
                 pic_entry_cb cb, void *context, char *error, size_t cap) {
    struct nfs_context *nfs=open_session(host,export_path,error,cap);
    if (!nfs) return -1;
    struct nfsdir *dir=NULL;
    uint64_t open_started = monotonic_ms();
    if (nfs_opendir(nfs,relative,&dir)!=0) {
        err(error,cap,"nfs_opendir",nfs);
        trace_stage("directory_open", open_started, host, export_path, relative, "outcome=error");
        if (strstr(error,"NFS4ERR_PERM") || strstr(error,"Permission denied")) {
            char detail[512];
            snprintf(detail,sizeof detail,"%s",error);
            snprintf(error,cap,"%.340s; server denied directory access (check export path, client UID/GID and directory execute/read permissions)",detail);
        }
        nfs_destroy_context(nfs);return -1;
    }
    trace_stage("directory_open", open_started, host, export_path, relative, "outcome=ok");
    int count=0;
    struct nfsdirent *entry;
    while ((entry=nfs_readdir(nfs,dir))) {
        if (!entry->name || strcmp(entry->name,".")==0 || strcmp(entry->name,"..")==0) continue;
        unsigned int kind=(entry->mode & S_IFMT)==S_IFDIR ? 7 :
                           (entry->mode & S_IFMT)==S_IFREG ? 8 : 0;
        if (kind && cb(context,entry->name,kind)!=0) break;
        count++;
    }
    nfs_closedir(nfs,dir);
    char detail[64];
    snprintf(detail,sizeof detail,"outcome=ok entries=%d",count);
    trace_stage("directory_list_complete", open_started, host, export_path, relative, detail);
    nfs_destroy_context(nfs);
    return count;
}
/* A libnfs context must never be used concurrently. Viewer reads run on
 * short-lived Rust threads, so thread-local storage caused every image read to
 * create a fresh session. Keep one process-local session and serialize its
 * complete operation; failed NFS operations invalidate it. The context is NOT
 * a GVfs or Linux kernel mount. */
static pthread_mutex_t read_session_lock = PTHREAD_MUTEX_INITIALIZER;
static struct nfs_context *read_session=NULL;
static char read_host[256]={0};
static char read_export[4096]={0};
static void invalidate_read_session(void) {
    if (read_session) nfs_destroy_context(read_session);
    read_session=NULL;
    read_host[0]=0;
    read_export[0]=0;
}
static struct nfs_context *get_read_session(const char *host, const char *export_path,
                                            char *error, size_t cap) {
    if (!read_session || strcmp(read_host,host) || strcmp(read_export,export_path)) {
        invalidate_read_session();
        read_session=open_session(host,export_path,error,cap);
        if (!read_session) return NULL;
        snprintf(read_host,sizeof read_host,"%s",host);
        snprintf(read_export,sizeof read_export,"%s",export_path);
    } else {
        if (trace_enabled()) fprintf(stderr,"PIC_NFS_CONNECT reuse host=%s export=%s\n",host,export_path);
    }
    return read_session;
}
int pic_nfs_read(const char *host, const char *export_path, const char *relative,
                 unsigned char **out, size_t *length, size_t max_bytes,
                 char *error, size_t cap) {
    *out=NULL;*length=0;
    uint64_t lock_started = monotonic_ms();
    pthread_mutex_lock(&read_session_lock);
    trace_stage("read_session_lock", lock_started, host, export_path, relative, "outcome=acquired");
    struct nfs_context *nfs=get_read_session(host,export_path,error,cap);
    if (!nfs) {pthread_mutex_unlock(&read_session_lock);return -1;}
    struct nfsfh *fh=NULL;
    uint64_t open_started = monotonic_ms();
    if(nfs_open(nfs,relative,O_RDONLY,&fh)!=0) {
        trace_stage("file_open", open_started, host, export_path, relative, "outcome=error");
        err(error,cap,"nfs_open",nfs);invalidate_read_session();
        pthread_mutex_unlock(&read_session_lock);return -1;
    }
    trace_stage("file_open", open_started, host, export_path, relative, "outcome=ok");
    size_t capbytes=64*1024,used=0;
    if(max_bytes<capbytes)capbytes=max_bytes;
    unsigned char *bytes=malloc(capbytes?capbytes:1);
    if(!bytes) {snprintf(error,cap,"NFS out of memory");nfs_close(nfs,fh);
        pthread_mutex_unlock(&read_session_lock);return -1;}
    int failed=0;
    uint64_t read_started = monotonic_ms();
    int read_calls = 0;
    for (;;) {
        if(used==capbytes) {
            if(capbytes>=max_bytes) {snprintf(error,cap,"NFS image exceeds 128 MiB safety limit");failed=1;break;}
            size_t next=capbytes*2;if(next>max_bytes)next=max_bytes;
            unsigned char *more=realloc(bytes,next);
            if(!more){snprintf(error,cap,"NFS out of memory");failed=1;break;}
            bytes=more;capbytes=next;
        }
        uint64_t call_started = monotonic_ms();
        struct rpc_stats stats_before = {0}, stats_after = {0};
        int collect_rpc_stats = trace_enabled();
        if (collect_rpc_stats)
            rpc_get_stats(nfs_get_rpc_context(nfs), &stats_before);
        int got=nfs_read(nfs,fh,bytes+used,capbytes-used);
        read_calls++;
        if (collect_rpc_stats) {
            rpc_get_stats(nfs_get_rpc_context(nfs), &stats_after);
            char detail[192];
            snprintf(detail, sizeof detail,
                     "iteration=%d outcome=%s bytes=%d timedout_delta=%llu retransmitted_delta=%llu reconnects_delta=%llu",
                     read_calls, got < 0 ? "error" : "ok", got,
                     (unsigned long long)(stats_after.num_timedout - stats_before.num_timedout),
                     (unsigned long long)(stats_after.num_retransmitted - stats_before.num_retransmitted),
                     (unsigned long long)(stats_after.num_reconnects - stats_before.num_reconnects));
            trace_stage("rpc_read", call_started, host, export_path, relative, detail);
        }
        if (read_calls == 1) {
            char detail[64];
            snprintf(detail, sizeof detail, "outcome=%s bytes=%d", got < 0 ? "error" : "ok", got);
            trace_stage("first_read", call_started, host, export_path, relative, detail);
        }
        if(got<0){err(error,cap,"nfs_read",nfs);failed=1;break;}
        if(got==0)break;
        used+=(size_t)got;
    }
    int close_status=nfs_close(nfs,fh);
    char read_detail[96];
    snprintf(read_detail, sizeof read_detail, "outcome=%s bytes=%zu calls=%d",
             failed ? "error" : "ok", used, read_calls);
    trace_stage("file_read_complete", read_started, host, export_path, relative, read_detail);
    if (close_status<0 && !failed) {err(error,cap,"nfs_close",nfs);failed=1;}
    if(failed){
        /* Resource/memory/size errors do not imply a broken NFS session. */
        if (close_status<0 || strstr(error,"nfs_read:") || strstr(error,"nfs_close:"))
            invalidate_read_session();
        free(bytes);pthread_mutex_unlock(&read_session_lock);return -1;
    }
    *out=bytes;*length=used;pthread_mutex_unlock(&read_session_lock);return 0;
}

int pic_nfs_read_range(const char *host, const char *export_path, const char *relative,
                       uint64_t offset, size_t requested, unsigned char **out, size_t *length,
                       char *error, size_t cap) {
    *out=NULL; *length=0;
    pthread_mutex_lock(&read_session_lock);
    struct nfs_context *nfs=get_read_session(host,export_path,error,cap);
    if (!nfs) { pthread_mutex_unlock(&read_session_lock); return -1; }
    struct nfsfh *fh=NULL;
    if (nfs_open(nfs,relative,O_RDONLY,&fh)!=0) { err(error,cap,"nfs_open",nfs); invalidate_read_session(); pthread_mutex_unlock(&read_session_lock); return -1; }
    uint64_t position=0;
    if (nfs_lseek(nfs,fh,(int64_t)offset,SEEK_SET,&position)!=0) { err(error,cap,"nfs_lseek",nfs); nfs_close(nfs,fh); invalidate_read_session(); pthread_mutex_unlock(&read_session_lock); return -1; }
    unsigned char *bytes=malloc(requested ? requested : 1);
    if (!bytes) { snprintf(error,cap,"NFS out of memory"); nfs_close(nfs,fh); pthread_mutex_unlock(&read_session_lock); return -1; }
    size_t used=0;
    while (used < requested) {
        int got=nfs_read(nfs,fh,bytes+used,requested-used);
        if (got < 0) { err(error,cap,"nfs_read",nfs); free(bytes); nfs_close(nfs,fh); invalidate_read_session(); pthread_mutex_unlock(&read_session_lock); return -1; }
        if (got == 0) break;
        used+=(size_t)got;
    }
    nfs_close(nfs,fh); pthread_mutex_unlock(&read_session_lock);
    *out=bytes; *length=used;
    if (trace_enabled()) fprintf(stderr,"PIC_NFS_RANGE offset=%llu requested=%zu received=%zu host=%s relative=%s\n",(unsigned long long)offset,requested,used,host,relative);
    return 0;
}

/* Scanner calls stat repeatedly on one worker thread. Reuse the libnfs
 * userspace session per worker, not one TCP session per photo. Never shared
 * across threads. An NFS error invalidates this cached context. */
static _Thread_local struct nfs_context *stat_session=NULL;
static _Thread_local char stat_host[256]={0};
static _Thread_local char stat_export[4096]={0};
int pic_nfs_stat(const char *host, const char *export_path, const char *relative,
                 uint64_t *size, int64_t *mtime, int *is_dir,
                 char *error, size_t cap) {
    if (!stat_session || strcmp(stat_host,host) || strcmp(stat_export,export_path)) {
        if (stat_session) {nfs_destroy_context(stat_session);stat_session=NULL;}
        stat_session=open_session(host,export_path,error,cap);
        if (!stat_session) return -1;
        snprintf(stat_host,sizeof stat_host,"%s",host);
        snprintf(stat_export,sizeof stat_export,"%s",export_path);
    }
    struct nfs_stat_64 st={0};
    int result=nfs_stat64(stat_session,relative,&st);
    if (result) {
        err(error,cap,"nfs_stat64",stat_session);
        nfs_destroy_context(stat_session);
        stat_session=NULL;
        stat_host[0]=0;
        stat_export[0]=0;
        return -1;
    }
    *size=st.nfs_size;
    *mtime=(int64_t)st.nfs_mtime;
    *is_dir=(st.nfs_mode & S_IFMT)==S_IFDIR;
    return 0;
}
