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

/* The legacy smbc_* API owns one process-wide client context. Keep calls
 * serialized because the context and its connection cache are not safe for
 * concurrent use; do not reinitialize it for each image read. */
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int initialized = 0;
static int trace_enabled(void) {
    const char *value = getenv("PICASA_TRACE");
    return value && *value;
}
static void guest_auth(const char *server, const char *share, char *workgroup, int wglen,
                       char *username, int unlen, char *password, int pwlen) {
    (void)server; (void)share; (void)workgroup; (void)wglen;
    if (unlen > 0) snprintf(username, (size_t)unlen, "guest");
    if (pwlen > 0) password[0] = '\0';
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
