/*
 * PIC private-NFS read probe. Read-only, no desktop/GVfs/kernel mount.
 * Fedora: sudo dnf install libnfs-devel gcc pkgconf-pkg-config
 * Build:  cc -O2 -Wall -Wextra pic-nfs-probe.c -o pic-nfs-probe $(pkg-config --cflags --libs libnfs)
 * Run:    ./pic-nfs-probe --exports 10.0.0.1
 *         ./pic-nfs-probe --list 3 10.0.0.1 /mnt/4TBP /
 *         ./pic-nfs-probe 3 10.0.0.1 /mnt/4TBP '/Other/Tat Sing/20190917_184453.jpg'
 *         ./pic-nfs-probe 4 10.0.0.1 / '/mnt/4TBP/Other/Tat Sing/20190917_184453.jpg'
 */
#define _DEFAULT_SOURCE

#include <nfsc/libnfs.h>
#include <nfsc/libnfs-raw-mount.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void print_failure(struct nfs_context *nfs, const char *op, int rc) {
    const char *detail = nfs_get_error(nfs);
    fprintf(stderr, "%s: rc=%d; libnfs=%s\n", op, rc,
            (detail && *detail) ? detail : "<empty>");
    if (rc == -5 || rc == -13) {
        fprintf(stderr,
                "NFS compatibility hint: if the NAS requires reserved client source ports, add the server-side 'insecure' export option; this probe is intentionally unprivileged.\n");
    }
}

static int print_exports(const char *host) {
    struct exportnode *exports = mount_getexports(host);
    if (!exports) {
        fprintf(stderr, "export discovery failed for %s\n", host);
        return 1;
    }
    int count = 0;
    for (struct exportnode *entry = exports; entry; entry = entry->ex_next) {
        if (entry->ex_dir) {
            puts(entry->ex_dir);
            count++;
        }
    }
    mount_free_export_list(exports);
    fprintf(stderr, "EXPORTS OK: %d export(s)\n", count);
    return count ? 0 : 1;
}

static int list_directory(const char *version_text, const char *host,
                          const char *export_path, const char *path) {
    struct nfs_context *nfs = nfs_init_context();
    if (!nfs) return 1;
    int version = atoi(version_text);
    int rc = nfs_set_version(nfs, version);
    if (rc == 0) rc = nfs_mount(nfs, host, export_path);
    if (rc != 0) {
        print_failure(nfs, "nfs_mount", rc);
        nfs_destroy_context(nfs);
        return 1;
    }
    struct nfsdir *dir = NULL;
    rc = nfs_opendir(nfs, path, &dir);
    if (rc != 0) {
        print_failure(nfs, "nfs_opendir", rc);
        nfs_destroy_context(nfs);
        return 1;
    }
    int count = 0;
    struct nfsdirent *entry;
    while ((entry = nfs_readdir(nfs, dir)) != NULL) {
        if (entry->name && strcmp(entry->name, ".") && strcmp(entry->name, "..")) {
            printf("%c %s\n", entry->type == 2 ? 'd' : 'f', entry->name);
            count++;
        }
    }
    nfs_closedir(nfs, dir);
    nfs_destroy_context(nfs);
    fprintf(stderr, "LIST OK: %d entr%s\n", count, count == 1 ? "y" : "ies");
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 3 && strcmp(argv[1], "--exports") == 0)
        return print_exports(argv[2]);
    if (argc == 6 && strcmp(argv[1], "--list") == 0 &&
        (strcmp(argv[2], "3") == 0 || strcmp(argv[2], "4") == 0))
        return list_directory(argv[2], argv[3], argv[4], argv[5]);
    if (argc != 5 || (strcmp(argv[1], "3") != 0 && strcmp(argv[1], "4") != 0)) {
        fprintf(stderr, "usage: %s --exports <host>\n"
                       "       %s --list <3|4> <host> <export> <directory>\n"
                       "       %s <3|4> <host> <export> <file>\n", argv[0], argv[0], argv[0]);
        return 2;
    }
    struct nfs_context *nfs = nfs_init_context();
    struct nfsfh *fh = NULL;
    int status = 1;
    if (!nfs) {
        fprintf(stderr, "nfs_init_context returned null\n");
        return 1;
    }
    fprintf(stderr, "uid=%ld gid=%ld version=%s host=%s export=%s file=%s\n",
            (long)getuid(), (long)getgid(), argv[1], argv[2], argv[3], argv[4]);
    int rc = nfs_set_version(nfs, atoi(argv[1]));
    if (rc != 0) {
        print_failure(nfs, "nfs_set_version", rc);
        goto cleanup;
    }
    rc = nfs_mount(nfs, argv[2], argv[3]);
    if (rc != 0) {
        print_failure(nfs, "nfs_mount", rc);
        goto cleanup;
    }
    fprintf(stderr, "MOUNT OK\n");
    struct nfs_stat_64 stat_result = {0};
    rc = nfs_stat64(nfs, argv[4], &stat_result);
    if (rc != 0) {
        print_failure(nfs, "nfs_stat64", rc);
        goto cleanup;
    }
    fprintf(stderr, "STAT OK size=%llu\n", (unsigned long long)stat_result.nfs_size);
    rc = nfs_open(nfs, argv[4], O_RDONLY, &fh);
    if (rc != 0) {
        print_failure(nfs, "nfs_open(O_RDONLY)", rc);
        goto cleanup;
    }
    fprintf(stderr, "OPEN OK\n");
    unsigned char bytes[32];
    int n = nfs_read(nfs, fh, bytes, sizeof(bytes));
    if (n < 0) {
        print_failure(nfs, "nfs_read", n);
        goto cleanup;
    }
    if (n == 0) {
        fprintf(stderr, "READ returned EOF; no data\n");
        goto cleanup;
    }
    printf("READ OK: %d bytes:", n);
    for (int i = 0; i < n; i++) printf(" %02x", bytes[i]);
    printf("\n");
    if (n >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff)
        puts("JPEG SIGNATURE OK");
    else
        puts("Note: these bytes do not start with the usual JPEG signature");
    status = 0;
cleanup:
    if (fh) {
        rc = nfs_close(nfs, fh);
        if (rc != 0) print_failure(nfs, "nfs_close", rc);
    }
    nfs_destroy_context(nfs);
    return status;
}
