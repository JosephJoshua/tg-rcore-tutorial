#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>

// Forward declarations — implemented in Rust
extern void doom_putchar(char c);
extern int doom_write_stdout(const char *s, int len);

static int fmt_int(char *buf, int bufsize, int *pos, long val, int base, int width, int pad_zero, int is_signed) {
    char tmp[24];
    int i = 0;
    int neg = 0;
    unsigned long uval;
    if (is_signed && val < 0) { neg = 1; uval = (unsigned long)(-val); }
    else { uval = (unsigned long)val; }
    if (uval == 0) { tmp[i++] = '0'; }
    else { while (uval > 0) { int d = uval % base; tmp[i++] = d < 10 ? '0' + d : 'a' + d - 10; uval /= base; } }
    int total = i + neg;
    int padding = width > total ? width - total : 0;
    char padchar = pad_zero ? '0' : ' ';
    if (!pad_zero) { for (int p = 0; p < padding && *pos < bufsize - 1; p++) buf[(*pos)++] = padchar; }
    if (neg && *pos < bufsize - 1) buf[(*pos)++] = '-';
    if (pad_zero) { for (int p = 0; p < padding && *pos < bufsize - 1; p++) buf[(*pos)++] = padchar; }
    for (int j = i - 1; j >= 0 && *pos < bufsize - 1; j--) buf[(*pos)++] = tmp[j];
    return 0;
}

int vsnprintf(char *str, size_t size, const char *fmt, va_list ap) {
    int pos = 0;
    int bufsize = (int)size;
    while (*fmt && pos < bufsize - 1) {
        if (*fmt != '%') { str[pos++] = *fmt++; continue; }
        fmt++;
        int pad_zero = 0, left_align = 0;
        while (*fmt == '0' || *fmt == '-' || *fmt == '+' || *fmt == ' ') {
            if (*fmt == '0') pad_zero = 1;
            if (*fmt == '-') left_align = 1;
            fmt++;
        }
        int width = 0;
        if (*fmt == '*') { width = va_arg(ap, int); fmt++; }
        else { while (*fmt >= '0' && *fmt <= '9') { width = width * 10 + (*fmt - '0'); fmt++; } }
        // Precision
        int has_prec = 0, prec = 0;
        if (*fmt == '.') {
            has_prec = 1; fmt++;
            if (*fmt == '*') { prec = va_arg(ap, int); fmt++; }
            else { while (*fmt >= '0' && *fmt <= '9') { prec = prec * 10 + (*fmt - '0'); fmt++; } }
        }
        int is_long = 0;
        if (*fmt == 'l') { is_long = 1; fmt++; }
        if (*fmt == 'l') { is_long = 2; fmt++; }
        if (*fmt == 'h') { fmt++; if (*fmt == 'h') fmt++; }
        if (*fmt == 'z') { is_long = 1; fmt++; }
        switch (*fmt) {
            case 'd': case 'i': { long val = is_long ? va_arg(ap, long) : (long)va_arg(ap, int); int w = has_prec ? prec : width; int pz = has_prec ? 1 : pad_zero; fmt_int(str, bufsize, &pos, val, 10, w, pz, 1); break; }
            case 'u': { unsigned long val = is_long ? va_arg(ap, unsigned long) : (unsigned long)va_arg(ap, unsigned int); int w = has_prec ? prec : width; int pz = has_prec ? 1 : pad_zero; fmt_int(str, bufsize, &pos, (long)val, 10, w, pz, 0); break; }
            case 'x': case 'X': { unsigned long val = is_long ? va_arg(ap, unsigned long) : (unsigned long)va_arg(ap, unsigned int); int w = has_prec ? prec : width; int pz = has_prec ? 1 : pad_zero; fmt_int(str, bufsize, &pos, (long)val, 16, w, pz, 0); break; }
            case 'p': { unsigned long val = (unsigned long)va_arg(ap, void*); if (pos < bufsize-1) str[pos++]='0'; if (pos < bufsize-1) str[pos++]='x'; fmt_int(str, bufsize, &pos, (long)val, 16, 0, 0, 0); break; }
            case 's': { const char *s = va_arg(ap, const char*); if (!s) s = "(null)"; int slen = 0; while (s[slen]) slen++; int pad = width > slen ? width - slen : 0; if (!left_align) for (int p = 0; p < pad && pos < bufsize-1; p++) str[pos++] = ' '; while (*s && pos < bufsize-1) str[pos++] = *s++; if (left_align) for (int p = 0; p < pad && pos < bufsize-1; p++) str[pos++] = ' '; break; }
            case 'c': { char c = (char)va_arg(ap, int); if (pos < bufsize-1) str[pos++] = c; break; }
            case '%': if (pos < bufsize-1) str[pos++] = '%'; break;
            case '\0': goto done;
            default: if (pos < bufsize-1) str[pos++] = *fmt; break;
        }
        fmt++;
    }
done:
    if (bufsize > 0) str[pos] = '\0';
    return pos;
}

int snprintf(char *str, size_t size, const char *fmt, ...) { va_list ap; va_start(ap, fmt); int ret = vsnprintf(str, size, fmt, ap); va_end(ap); return ret; }
int sprintf(char *str, const char *fmt, ...) { va_list ap; va_start(ap, fmt); int ret = vsnprintf(str, 4096, fmt, ap); va_end(ap); return ret; }
int printf(const char *fmt, ...) { char buf[1024]; va_list ap; va_start(ap, fmt); int ret = vsnprintf(buf, sizeof(buf), fmt, ap); va_end(ap); doom_write_stdout(buf, ret); return ret; }
int fprintf(FILE *f, const char *fmt, ...) { char buf[1024]; va_list ap; va_start(ap, fmt); int ret = vsnprintf(buf, sizeof(buf), fmt, ap); va_end(ap); doom_write_stdout(buf, ret); return ret; }
int puts(const char *s) { int len = 0; while (s[len]) len++; doom_write_stdout(s, len); doom_putchar('\n'); return 0; }
int putchar(int c) { char ch = (char)c; doom_write_stdout(&ch, 1); return c; }

int sscanf(const char *str, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt); int count = 0;
    while (*fmt && *str) {
        if (*fmt == '%') {
            fmt++;
            if (*fmt == 'd' || *fmt == 'i') {
                int *out = va_arg(ap, int*); int val = 0, neg = 0;
                while (*str == ' ') str++;
                if (*str == '-') { neg = 1; str++; }
                if (*str < '0' || *str > '9') break;
                while (*str >= '0' && *str <= '9') { val = val * 10 + (*str - '0'); str++; }
                *out = neg ? -val : val; count++; fmt++;
            } else if (*fmt == 's') {
                char *out = va_arg(ap, char*);
                while (*str && *str != ' ' && *str != '\n' && *str != '\t') *out++ = *str++;
                *out = '\0'; count++; fmt++;
            } else { break; }
        } else { if (*fmt == *str) { fmt++; str++; } else break; }
    }
    va_end(ap); return count;
}
