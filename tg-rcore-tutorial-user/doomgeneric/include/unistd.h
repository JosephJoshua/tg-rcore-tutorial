#ifndef _DOOM_UNISTD_H
#define _DOOM_UNISTD_H

#include <stddef.h>
#include <sys/types.h>

int close(int fd);
ssize_t read(int fd, void *buf, size_t count);
ssize_t write(int fd, const void *buf, size_t count);
off_t lseek(int fd, off_t offset, int whence);
int unlink(const char *pathname);
unsigned int sleep(unsigned int seconds);
int usleep(unsigned int usec);
int access(const char *pathname, int mode);
char *getcwd(char *buf, size_t size);
int chdir(const char *path);
int isatty(int fd);
int rmdir(const char *pathname);

pid_t getpid(void);

#define R_OK 4
#define W_OK 2
#define X_OK 1
#define F_OK 0

#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

#endif /* _DOOM_UNISTD_H */
