#ifndef _DOOM_ASSERT_H
#define _DOOM_ASSERT_H

extern void doom_assert_fail(const char *expr, const char *file, int line);

#define assert(expr) ((expr) ? (void)0 : doom_assert_fail(#expr, __FILE__, __LINE__))

#endif /* _DOOM_ASSERT_H */
