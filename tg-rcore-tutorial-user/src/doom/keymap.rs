//! VirtIO keycode to Doom keycode translation.

// Doom key constants (from doomkeys.h)
pub const KEY_RIGHTARROW: u8 = 0xae;
pub const KEY_LEFTARROW: u8 = 0xac;
pub const KEY_UPARROW: u8 = 0xad;
pub const KEY_DOWNARROW: u8 = 0xaf;
pub const KEY_FIRE: u8 = 0xa3;
pub const KEY_USE: u8 = 0xa2;
pub const KEY_ESCAPE: u8 = 27;
pub const KEY_ENTER: u8 = 13;
pub const KEY_TAB: u8 = 9;
pub const KEY_BACKSPACE: u8 = 127;
pub const KEY_RSHIFT: u8 = 0xb6;
pub const KEY_LALT: u8 = 0xb8;
pub const KEY_F1: u8 = 0x80 + 0x3b;
pub const KEY_F2: u8 = 0x80 + 0x3c;
pub const KEY_F3: u8 = 0x80 + 0x3d;
pub const KEY_F4: u8 = 0x80 + 0x3e;
pub const KEY_F5: u8 = 0x80 + 0x3f;
pub const KEY_F6: u8 = 0x80 + 0x40;
pub const KEY_F7: u8 = 0x80 + 0x41;
pub const KEY_F8: u8 = 0x80 + 0x42;
pub const KEY_F9: u8 = 0x80 + 0x43;
pub const KEY_F10: u8 = 0x80 + 0x44;

// VirtIO scancode to ASCII lookup table
const SCANCODE_TO_ASCII: [u8; 128] = {
    let mut t = [0u8; 128];
    // Number row: 1-0 = scancodes 2-11
    t[2] = b'1'; t[3] = b'2'; t[4] = b'3'; t[5] = b'4'; t[6] = b'5';
    t[7] = b'6'; t[8] = b'7'; t[9] = b'8'; t[10] = b'9'; t[11] = b'0';
    // Letters (QWERTY layout scancodes)
    t[16] = b'q'; t[17] = b'w'; t[18] = b'e'; t[19] = b'r'; t[20] = b't';
    t[21] = b'y'; t[22] = b'u'; t[23] = b'i'; t[24] = b'o'; t[25] = b'p';
    t[30] = b'a'; t[31] = b's'; t[32] = b'd'; t[33] = b'f'; t[34] = b'g';
    t[35] = b'h'; t[36] = b'j'; t[37] = b'k'; t[38] = b'l';
    t[44] = b'z'; t[45] = b'x'; t[46] = b'c'; t[47] = b'v'; t[48] = b'b';
    t[49] = b'n'; t[50] = b'm';
    t[12] = b'-'; t[13] = b'='; t[39] = b';'; t[40] = b'\'';
    t[51] = b','; t[52] = b'.'; t[53] = b'/';
    t
};

/// Translate a VirtIO scancode to a Doom keycode.
/// Returns 0 if no translation exists.
pub fn translate(scancode: u8) -> u8 {
    match scancode {
        103 => KEY_UPARROW,
        108 => KEY_DOWNARROW,
        105 => KEY_LEFTARROW,
        106 => KEY_RIGHTARROW,
        29 => KEY_FIRE,       // LCtrl
        57 => KEY_USE,        // Space
        28 => KEY_ENTER,
        1 => KEY_ESCAPE,
        42 => KEY_RSHIFT,     // LShift
        56 => KEY_LALT,       // LAlt
        15 => KEY_TAB,
        14 => KEY_BACKSPACE,
        // F-keys: VirtIO 59-68 -> Doom F1-F10
        59 => KEY_F1, 60 => KEY_F2, 61 => KEY_F3, 62 => KEY_F4, 63 => KEY_F5,
        64 => KEY_F6, 65 => KEY_F7, 66 => KEY_F8, 67 => KEY_F9, 68 => KEY_F10,
        // Number/letter keys via lookup table
        sc if sc < 128 => {
            let ascii = SCANCODE_TO_ASCII[sc as usize];
            if ascii != 0 { ascii } else { 0 }
        }
        _ => 0,
    }
}
