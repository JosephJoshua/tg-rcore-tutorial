#ifndef _DOOM_SYS_STAT_H
#define _DOOM_SYS_STAT_H

#include <sys/types.h>

struct stat {
    dev_t     st_dev;
    ino_t     st_ino;
    mode_t    st_mode;
    nlink_t   st_nlink;
    uid_t     st_uid;
    gid_t     st_gid;
    dev_t     st_rdev;
    off_t     st_size;
    long      st_blksize;
    long      st_blocks;
    long      st_atime;
    long      st_mtime;
    long      st_ctime;
};

#define S_IFMT   0170000
#define S_IFDIR  0040000
#define S_IFREG  0100000
#define S_ISDIR(m)  (((m) & S_IFMT) == S_IFDIR)
#define S_ISREG(m)  (((m) & S_IFMT) == S_IFREG)

int stat(const char *path, struct stat *buf);
int fstat(int fd, struct stat *buf);
int mkdir(const char *path, mode_t mode);

#endif /* _DOOM_SYS_STAT_H */
