#include <nfsc/libnfs.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
struct nfs_context { int mounted; };
struct nfsfh { size_t offset; };
struct nfsdir { int pos; };
static const char jpeg[] = "\xff\xd8\xff" "sample-jpeg";
struct nfs_context *nfs_init_context(void) {return calloc(1,sizeof(struct nfs_context));}
void nfs_destroy_context(struct nfs_context *n) {free(n);}
void nfs_set_autoreconnect(struct nfs_context *n,int v) {(void)n;(void)v;}
void nfs_set_retrans(struct nfs_context *n,int v) {(void)n;(void)v;}
int nfs_set_version(struct nfs_context *n,int v) {(void)n;return v==4?0:-1;}
int nfs_mount(struct nfs_context *n,const char *h,const char *p) {(void)p;n->mounted=1;return !strcmp(h,"mock.local")?0:-1;}
const char *nfs_get_error(struct nfs_context *n) {(void)n;return "mock: host refused";}
int nfs_stat64(struct nfs_context *n,const char *p,struct nfs_stat_64 *s) {
 (void)n;memset(s,0,sizeof(*s));if(!strcmp(p,"/export")) {s->nfs_mode=S_IFDIR;return 0;}
 if(!strcmp(p,"/export/img.jpg")) {s->nfs_mode=S_IFREG;s->nfs_size=sizeof(jpeg)-1;s->nfs_mtime=42;return 0;}return -2;
}
int nfs_opendir(struct nfs_context *n,const char *p,struct nfsdir **d) {
 (void)n;if(strcmp(p,"/export"))return -2;*d=calloc(1,sizeof(**d));return 0;
}
struct nfsdirent *nfs_readdir(struct nfs_context *n,struct nfsdir *d) {
 (void)n;static struct nfsdirent e={"img.jpg",1,sizeof(jpeg)-1,{42}};
 return d->pos++==0 ? &e : NULL;
}
int nfs_closedir(struct nfs_context *n,struct nfsdir *d) {(void)n;free(d);return 0;}
int nfs_open(struct nfs_context *n,const char *p,int flags,struct nfsfh **f) {
 (void)n;(void)flags;if(strcmp(p,"/export/img.jpg"))return -2;*f=calloc(1,sizeof(**f));return 0;
}
int nfs_read(struct nfs_context *n,struct nfsfh *f,void *out,size_t max) {
 (void)n;size_t rem=sizeof(jpeg)-1-f->offset;size_t size=rem<max?rem:max;
 memcpy(out,jpeg+f->offset,size);f->offset+=size;return (int)size;
}
int nfs_close(struct nfs_context *n,struct nfsfh *f) {(void)n;free(f);return 0;}
