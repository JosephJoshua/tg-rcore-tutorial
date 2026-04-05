// Stub implementations for excluded SDL-dependent files.
// These provide the symbols that other Doom source files reference.
// Also provides memcpy/memset/memmove in C to avoid Rust ABI issues.

#include <stddef.h>
#include <stdarg.h>

// Implement memcpy/memset/memmove in C to avoid Rust intrinsic recursion
// and ensure correct calling convention. These MUST be defined before
// any headers that might use them as builtins.
void *memcpy(void *dest, const void *src, size_t n) {
    unsigned char *d = dest;
    const unsigned char *s = src;
    // Word-sized copy when aligned
    if (((size_t)d | (size_t)s) % sizeof(long) == 0) {
        unsigned long *dw = (unsigned long *)d;
        const unsigned long *sw = (const unsigned long *)s;
        size_t words = n / sizeof(long);
        for (size_t i = 0; i < words; i++) dw[i] = sw[i];
        size_t rem = words * sizeof(long);
        for (size_t i = rem; i < n; i++) d[i] = s[i];
    } else {
        for (size_t i = 0; i < n; i++) d[i] = s[i];
    }
    return dest;
}

void *memset(void *s, int c, size_t n) {
    unsigned char *p = s;
    unsigned char val = (unsigned char)c;
    if ((size_t)p % sizeof(long) == 0) {
        unsigned long word = val;
        word |= word << 8; word |= word << 16; word |= word << 32;
        unsigned long *pw = (unsigned long *)p;
        size_t words = n / sizeof(long);
        for (size_t i = 0; i < words; i++) pw[i] = word;
        size_t rem = words * sizeof(long);
        for (size_t i = rem; i < n; i++) p[i] = val;
    } else {
        for (size_t i = 0; i < n; i++) p[i] = val;
    }
    return s;
}

void *memmove(void *dest, const void *src, size_t n) {
    unsigned char *d = dest;
    const unsigned char *s = src;
    if (d < s) {
        return memcpy(dest, src, n);
    } else {
        for (size_t i = n; i > 0; i--) d[i-1] = s[i-1];
    }
    return dest;
}

int memcmp(const void *s1, const void *s2, size_t n) {
    const unsigned char *a = s1, *b = s2;
    for (size_t i = 0; i < n; i++) {
        if (a[i] != b[i]) return (int)a[i] - (int)b[i];
    }
    return 0;
}

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "doomtype.h"
#include "d_event.h"
#include "i_sound.h"
#include "i_joystick.h"

// ============================================================
// i_system.c stubs
// ============================================================

typedef void (*atexit_func_t)(void);

void I_AtExit(atexit_func_t func, boolean run_on_error)
{
    (void)func; (void)run_on_error;
}

void I_Tactile(int on, int off, int total)
{
    (void)on; (void)off; (void)total;
}

byte *I_ZoneBase(int *size)
{
    *size = 8 * 1024 * 1024;
    return (byte *)malloc(*size);
}

void I_PrintBanner(char *msg) { printf("%s\n", msg); }
void I_PrintDivider(void) { printf("---\n"); }
void I_PrintStartupBanner(char *gamedescription) { printf("Starting %s\n", gamedescription); }
boolean I_ConsoleStdout(void) { return 1; }
void I_Init(void) {}
void I_BindVariables(void) {}

void I_Quit(void)
{
    exit(0);
}

void I_Error(char *error, ...)
{
    char buf[512];
    va_list ap;
    va_start(ap, error);
    vsnprintf(buf, sizeof(buf), error, ap);
    va_end(ap);
    printf("I_Error: %s\n", buf);
    exit(1);
}

boolean I_GetMemoryValue(unsigned int offset, void *value, int size)
{
    (void)offset; (void)value; (void)size;
    return 0;
}

// ============================================================
// i_sound.c stubs — signatures match i_sound.h exactly
// ============================================================

int snd_sfxdevice = 0;
int snd_musicdevice = 0;
int snd_samplerate = 0;
int snd_cachesize = 0;
int snd_maxslicetime_ms = 0;
char *snd_musiccmd = "";

void I_InitSound(boolean use_sfx_prefix) { (void)use_sfx_prefix; }
void I_ShutdownSound(void) {}
int I_GetSfxLumpNum(sfxinfo_t *sfxinfo) { (void)sfxinfo; return 0; }
void I_UpdateSound(void) {}
void I_UpdateSoundParams(int channel, int vol, int sep) { (void)channel; (void)vol; (void)sep; }
int I_StartSound(sfxinfo_t *sfxinfo, int channel, int vol, int sep) { (void)sfxinfo; (void)channel; (void)vol; (void)sep; return 0; }
void I_StopSound(int channel) { (void)channel; }
boolean I_SoundIsPlaying(int channel) { (void)channel; return 0; }
void I_PrecacheSounds(sfxinfo_t *sounds, int num_sounds) { (void)sounds; (void)num_sounds; }

void I_InitMusic(void) {}
void I_ShutdownMusic(void) {}
void I_SetMusicVolume(int volume) { (void)volume; }
void I_PauseSong(void) {}
void I_ResumeSong(void) {}
void *I_RegisterSong(void *data, int len) { (void)data; (void)len; return NULL; }
void I_UnRegisterSong(void *handle) { (void)handle; }
void I_PlaySong(void *handle, boolean looping) { (void)handle; (void)looping; }
void I_StopSong(void) {}
boolean I_MusicIsPlaying(void) { return 0; }
void I_BindSoundVariables(void) {}


// ============================================================
// i_timer.c stubs — doomgeneric.c provides the real timing
// ============================================================

extern unsigned int DG_GetTicksMs(void);
extern void DG_SleepMs(unsigned int ms);

int I_GetTime(void)
{
    return (int)(DG_GetTicksMs() * 35 / 1000);
}

int I_GetTimeMS(void)
{
    return (int)DG_GetTicksMs();
}

void I_Sleep(int ms)
{
    DG_SleepMs((unsigned int)ms);
}

void I_InitTimer(void) {}
void I_WaitVBL(int count) { (void)count; I_Sleep(count * 1000 / 70); }

// ============================================================
// i_input.c stubs — keyboard handled by DG_GetKey
// ============================================================

int vanilla_keyboard_mapping = 1;

void I_BindInputVariables(void) {}
void I_ReadMouse(void) {}
void I_InitInput(void) {}

// I_GetEvent: read keyboard events via DG_GetKey and post them to the engine
extern int DG_GetKey(int *pressed, unsigned char *key);
extern void D_PostEvent(event_t *ev);
void I_GetEvent(void)
{
    event_t event;
    int pressed;
    unsigned char key;
    while (DG_GetKey(&pressed, &key))
    {
        if (pressed)
        {
            event.type = ev_keydown;
            event.data1 = key;
            event.data2 = key;
            event.data3 = 0;
        }
        else
        {
            event.type = ev_keyup;
            event.data1 = key;
            event.data2 = 0;
            event.data3 = 0;
        }
        D_PostEvent(&event);
    }
}

// ============================================================
// i_joystick.c stubs
// ============================================================

void I_InitJoystick(void) {}
void I_ShutdownJoystick(void) {}
void I_UpdateJoystick(void) {}
void I_BindJoystickVariables(void) {}

// ============================================================
// i_cdmus.c stubs
// ============================================================

int cd_Error;
