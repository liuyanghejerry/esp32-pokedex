/*
 * ESP32-C3 memory layout for this firmware — our own copy of esp-hal's
 * `ld/esp32c3/memory.x`, with only the external-flash window sizes changed.
 * `build.rs` swaps it in for esp-hal's copy (see the comment there).
 *
 * Why: esp-hal sizes both flash windows at 4 MB, but this firmware is
 * asset-heavy (386 sprites + 386 cries + BGM). The chip's flash MMU has
 * 128 x 64 KB pages shared between the instruction and data windows, i.e.
 * 8 MB of mapped flash in total, and `esp32c3.x` adds a `.rotext_dummy` gap to
 * IROM as large as `.rodata` (both windows map the same flash), so IROM has to
 * span `rodata + text`. Sizing both windows to the app partition (0x7F0000)
 * makes the linker reject anything that could not be flashed anyway.
 */
MEMORY
{
    /*
        https://github.com/espressif/esptool/blob/ed64d20b051d05f3f522bacc6a786098b562d4b8/esptool/targets/esp32c3.py#L78-L90
        MEMORY_MAP = [[0x00000000, 0x00010000, "PADDING"],
                  [0x3C000000, 0x3C800000, "DROM"],
                  [0x3FC80000, 0x3FCE0000, "DRAM"],
                  [0x3FC88000, 0x3FD00000, "BYTE_ACCESSIBLE"],
                  [0x3FF00000, 0x3FF20000, "DROM_MASK"],
                  [0x40000000, 0x40060000, "IROM_MASK"],
                  [0x42000000, 0x42800000, "IROM"],
                  [0x4037C000, 0x403E0000, "IRAM"],
                  [0x50000000, 0x50002000, "RTC_IRAM"],
                  [0x50000000, 0x50002000, "RTC_DRAM"],
                  [0x600FE000, 0x60100000, "MEM_INTERNAL2"]]
    */

    ICACHE : ORIGIN = 0x4037C000,  LENGTH = 0x4000
    /* Instruction RAM */
    IRAM : ORIGIN = 0x4037C000 + 0x4000, LENGTH = 313K - 0x4000
    /* Data RAM */
    DRAM : ORIGIN = 0x3FC80000, LENGTH = 313K

    /* memory available after the 2nd stage bootloader is finished */
    dram2_seg ( RW )       : ORIGIN = ORIGIN(DRAM) + LENGTH(DRAM), len = 0x3fcde710 - (ORIGIN(DRAM) + LENGTH(DRAM))

    /* External flash

     The 0x20 offset is a convenience for the app binary image generation.
     Flash cache has 64KB pages. The .bin file which is flashed to the chip
     has a 0x18 byte file header, and each segment has a 0x08 byte segment
     header. Setting this offset makes it simple to meet the flash cache MMU's
     constraint that (paddr % 64KB == vaddr % 64KB).)
    */

    /* Instruction ROM — must also span the .rotext_dummy gap, so it is sized
       like DROM plus the text segments. */
    IROM : ORIGIN =   0x42000000 + 0x20, LENGTH = 0x7F0000 - 0x20
    /* Data ROM — sprites, cries, BGM and fonts all live here. */
    DROM : ORIGIN = 0x3C000000 + 0x20, LENGTH = 0x7F0000 - 0x20

    /* RTC fast memory (executable). Persists over deep sleep. */
    RTC_FAST : ORIGIN = 0x50000000, LENGTH = 0x2000 /*- ESP_BOOTLOADER_RESERVE_RTC*/
}
