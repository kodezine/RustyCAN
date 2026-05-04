//! IBM CP437 8×16 font atlas, A8 format.
//!
//! Each of the 256 glyphs is stored as 128 bytes: 16 rows × 8 columns,
//! one byte per pixel.  Each byte is either `0x00` (transparent / background)
//! or `0xFF` (opaque / foreground).  This A8 layout is fed directly to DMA2D
//! in A8 + fixed-colour blending mode, allowing hardware colour substitution
//! without modifying the atlas.
//!
//! The bitmaps are the classic IBM VGA 8×16 CP437 glyphs, reproduced from
//! the public-domain VGA ROM image.  The raw 1 bpp bitmaps (256 × 16 bytes =
//! 4 096 bytes) are expanded here to 256 × 128 bytes = 32 768 bytes total,
//! stored in flash as `pub const FONT_ATLAS`.
//!
//! `glyph(ch)` returns a `&'static [u8; 128]` slice for one character.

// Raw IBM CP437 VGA 8x16 bitmaps, 1bpp, 16 bytes per glyph.
// Each byte is one row; bit 7 = leftmost pixel.
// Source: public-domain VGA ROM font data.
static IBM_1BPP: [[u8; 16]; 256] = include!(concat!(env!("OUT_DIR"), "/ibm_cp437_1bpp.rs"));

/// IBM CP437 8×16 glyph atlas in A8 format (0x00 = bg, 0xFF = fg).
/// 256 glyphs × 128 bytes each = 32 768 bytes, stored in flash.
pub static FONT_ATLAS: [[u8; 128]; 256] = {
    // Build the atlas at compile time by expanding 1bpp → A8.
    let mut atlas = [[0u8; 128]; 256];
    let mut g = 0usize;
    while g < 256 {
        let src = &IBM_1BPP[g];
        let mut row = 0usize;
        while row < 16 {
            let byte = src[row];
            let mut bit = 0usize;
            while bit < 8 {
                atlas[g][row * 8 + bit] = if byte & (0x80 >> bit) != 0 {
                    0xFF
                } else {
                    0x00
                };
                bit += 1;
            }
            row += 1;
        }
        g += 1;
    }
    atlas
};

/// Return the A8 bitmap for a CP437 character code.
#[inline(always)]
pub fn glyph(ch: u8) -> &'static [u8; 128] {
    &FONT_ATLAS[ch as usize]
}
