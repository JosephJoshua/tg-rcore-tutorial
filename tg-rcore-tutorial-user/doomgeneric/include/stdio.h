#ifndef _DOOM_STDIO_H
#define _DOOM_STDIO_H

#include <stdarg.h>
#include <stddef.h>

typedef struct _FILE FILE;

extern FILE *stdin;
extern FILE *stdout;
extern FILE *stderr;

#define EOF (-1)
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#define BUFSIZ 1024

FILE *fopen(const char *path, const char *mode);
int fclose(FILE *stream);
size_t fread(void *ptr, size_t size, size_t nmemb, FILE *stream);
size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *stream);
int fseek(FILE *stream, long offset, int whence);
long ftell(FILE *stream);
void rewind(FILE *stream);
int feof(FILE *stream);
int ferror(FILE *stream);
int fflush(FILE *stream);
int fgetc(FILE *stream);
char *fgets(char *s, int size, FILE *stream);
int fputc(int c, FILE *stream);
int fputs(const char *s, FILE *stream);
int ungetc(int c, FILE *stream);

int printf(const char *fmt, ...);
int fprintf(FILE *stream, const char *fmt, ...);
int sprintf(char *str, const char *fmt, ...);
int snprintf(char *str, size_t size, const char *fmt, ...);
int vsnprintf(char *str, size_t size, const char *fmt, va_list ap);
int vfprintf(FILE *stream, const char *fmt, va_list ap);

int sscanf(const char *str, const char *fmt, ...);

int puts(const char *s);
int putchar(int c);
int getchar(void);

int remove(const char *path);
int rename(const char *oldpath, const char *newpath);
FILE *tmpfile(void);

void perror(const char *s);

#endif /* _DOOM_STDIO_H */
