#ifndef PIC_MOCK_LIBNFS_H
#define PIC_MOCK_LIBNFS_H
#include <stdint.h>
#include <stddef.h>
#include <sys/types.h>
struct nfs_context;
struct nfsfh;
struct nfsdir;
struct nfs_stat_64 { uint64_t nfs_mode,nfs_size,nfs_mtime; };
struct nfsdirent { const char *name; int type; uint64_t size; struct {long tv_sec;} mtime; };
struct nfs_context *nfs_init_context(void);
void nfs_destroy_context(struct nfs_context *);
void nfs_set_autoreconnect(struct nfs_context *, int);
void nfs_set_retrans(struct nfs_context *, int);
int nfs_set_version(struct nfs_context *, int);
int nfs_mount(struct nfs_context *, const char *, const char *);
const char *nfs_get_error(struct nfs_context *);
int nfs_stat64(struct nfs_context *, const char *, struct nfs_stat_64 *);
int nfs_opendir(struct nfs_context *, const char *, struct nfsdir **);
struct nfsdirent *nfs_readdir(struct nfs_context *, struct nfsdir *);
int nfs_closedir(struct nfs_context *, struct nfsdir *);
int nfs_open(struct nfs_context *, const char *, int, struct nfsfh **);
int nfs_read(struct nfs_context *, struct nfsfh *, void *, size_t);
int nfs_close(struct nfs_context *, struct nfsfh *);
#endif
