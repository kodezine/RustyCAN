/* STM32H753ZI memory map
 *
 * Flash: 2 MB at 0x0800_0000 (two 1 MB banks, Bank1 + Bank2)
 * DTCM:  128 KB at 0x2000_0000  — tightly coupled, fastest RAM, used for stack
 * AXI:   512 KB at 0x2400_0000  — general purpose RAM, DMA accessible
 * SRAM1: 128 KB at 0x3000_0000  — additional RAM
 * SRAM2: 128 KB at 0x3002_0000
 * SRAM3:  32 KB at 0x3004_0000
 * SRAM4:  64 KB at 0x3800_0000  — retained in standby; used for USB EP buffers
 *
 * We use DTCM for the stack (ORIGIN = RAM) and AXI SRAM for .bss/.data.
 * USB endpoint buffers must be in AXI SRAM (DMA accessible).
 */

MEMORY
{
    FLASH  (rx)  : ORIGIN = 0x08000000, LENGTH = 2M
    RAM    (rwx) : ORIGIN = 0x20000000, LENGTH = 128K  /* DTCM — stack */
    AXISRAM (rw) : ORIGIN = 0x24000000, LENGTH = 512K  /* AXI SRAM — heap/data */
}

/* Stack at top of DTCM */
_stack_start = ORIGIN(RAM) + LENGTH(RAM);
