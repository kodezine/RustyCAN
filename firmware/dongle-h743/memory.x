/* STM32H743XI app linker script.
 *
 * The KCAN app lives in Bank1 sectors 1–7 (896 KB, 0x08020000–0x080FFFFF).
 * The bootloader occupies Bank1 sector 0 (128 KB, 0x08000000–0x0801FFFF).
 *
 * Partition layout (full 2 MB flash) — same as bootloader-h743/memory.x:
 *   BOOTLOADER  : 0x08000000  128 KB  Bank1 sector 0   — do not overwrite!
 *   ACTIVE      : 0x08020000  768 KB  Bank1 sectors 1–6 — this binary
 *   (unused)    : 0x080E0000  128 KB  Bank1 sector 7
 *   STATE       : 0x08100000  128 KB  Bank2 sector 0   — embassy-boot state
 *   DFU         : 0x08120000  896 KB  Bank2 sectors 1–7 — update staging
 *
 * The __bootloader_* symbols allow the app to call
 * FirmwareUpdaterConfig::from_linkerfile_blocking for mark_booted().
 */

MEMORY
{
    FLASH  (rx)  : ORIGIN = 0x08020000, LENGTH = 768K  /* app code */
    RAM    (rwx) : ORIGIN = 0x20000000, LENGTH = 128K  /* DTCM — stack */
    AXISRAM (rw) : ORIGIN = 0x24000000, LENGTH = 512K  /* AXI SRAM — data/bss */
    SRAM4   (rw) : ORIGIN = 0x38000000, LENGTH = 64K   /* D3 SRAM4 — lcd_handoff NOLOAD */
}

_stack_start = ORIGIN(RAM) + LENGTH(RAM);

/* embassy-boot partition symbols (byte offsets from 0x08000000) */
__bootloader_active_start = 0x00020000;  /* 0x08020000 */
__bootloader_active_end   = 0x000E0000;  /* 0x080E0000 */
__bootloader_state_start  = 0x00100000;  /* 0x08100000 */
__bootloader_state_end    = 0x00120000;  /* 0x08120000 */
__bootloader_dfu_start    = 0x00120000;  /* 0x08120000 */
__bootloader_dfu_end      = 0x00200000;  /* 0x08200000 */
