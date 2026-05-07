/* bootloader-size-probe: temporary measurement crate.
 *
 * Uses the FULL 128 KB bootloader budget as its code flash region.
 * Also defines the embassy-boot partition symbols so the crate links cleanly.
 *
 * Partition layout (same as the real bootloader will use):
 *   FLASH   (bootloader code) : 0x08000000, 128KB  (Bank1 sector 0)
 *   ACTIVE  (app)             : 0x08020000, 896KB  (Bank1 sectors 1-7)
 *   STATE   (boot state)      : 0x08100000, 128KB  (Bank2 sector 0)
 *   DFU     (update staging)  : 0x08120000, 896KB  (Bank2 sectors 1-7)
 */

MEMORY
{
    FLASH           (rx)  : ORIGIN = 0x08000000, LENGTH = 128K
    ACTIVE                : ORIGIN = 0x08020000, LENGTH = 896K
    BOOTLOADER_STATE      : ORIGIN = 0x08100000, LENGTH = 128K
    DFU                   : ORIGIN = 0x08120000, LENGTH = 896K
    RAM             (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
}

_stack_start = ORIGIN(RAM) + LENGTH(RAM);

/* embassy-boot linker symbols (offsets from start of FLASH) */
__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(FLASH);
__bootloader_state_end   = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(FLASH);
__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(FLASH);
__bootloader_active_end   = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(FLASH);
__bootloader_dfu_start    = ORIGIN(DFU) - ORIGIN(FLASH);
__bootloader_dfu_end      = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(FLASH);
