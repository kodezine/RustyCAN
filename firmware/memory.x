/* STM32H743XI / STM32H753ZI memory map
 *
 * Flash:  2 MB at 0x0800_0000 (two 1 MB banks, Bank1 + Bank2)
 * DTCM:   128 KB at 0x2000_0000  — tightly coupled, fastest RAM, used for stack
 * AXI:    512 KB at 0x2400_0000  — general purpose RAM, DMA accessible
 * SRAM1:  128 KB at 0x3000_0000  — additional RAM
 * SRAM2:  128 KB at 0x3002_0000
 * SRAM3:   32 KB at 0x3004_0000
 * SRAM4:   64 KB at 0x3800_0000  — retained in standby; .lcd_handoff NOLOAD lives here
 * SDRAM:   32 MB at 0xD000_0000  — IS42S32800J-6BLI via FMC Bank 2 (SDCLK = HCLK/3)
 *
 * We use DTCM for the stack (ORIGIN = RAM) and AXI SRAM for .bss/.data.
 * USB endpoint buffers must be in AXI SRAM (DMA accessible).
 * LCD framebuffer (640×480 RGB565 = 600 KB) lives in SDRAM.
 * LcdHandoff struct lives in SRAM4 as a NOLOAD section so it survives
 * a bootloader→firmware CPU reset without being zeroed by cortex-m-rt.
 */

MEMORY
{
    FLASH   (rx)  : ORIGIN = 0x08000000, LENGTH = 2M
    RAM     (rwx) : ORIGIN = 0x20000000, LENGTH = 128K  /* DTCM — stack */
    AXISRAM (rw)  : ORIGIN = 0x24000000, LENGTH = 512K  /* AXI SRAM — heap/data */
    SRAM4   (rw)  : ORIGIN = 0x38000000, LENGTH = 64K   /* backup SRAM — .lcd_handoff */
    SDRAM   (rw)  : ORIGIN = 0xD0000000, LENGTH = 32M   /* FMC SDRAM — LCD framebuffer */
}

/* Stack at top of DTCM */
_stack_start = ORIGIN(RAM) + LENGTH(RAM);
