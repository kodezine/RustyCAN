# Project Structure

```
Cargo.toml          root workspace (host, kcan-protocol)
kcan-protocol/      shared wire-protocol crate (no_std + std feature)
  src/
    frame.rs        KCanFrame — 80-byte wire format, LE, to_bytes/from_bytes
    control.rs      KCanBitTiming, KCanBtConst, KCanDeviceInfo, KCanMode, RequestCode
    encrypted.rs    EncryptionLayer trait stub (Phase 3 — STM32H563 SAES)
host/               rustycan host application
  Cargo.toml
  src/
    lib.rs          public library surface
    main.rs         binary entry-point (launches GUI)
    app.rs          AppState, CanEvent enum, event application logic
    session.rs      SessionConfig, CanCommand, adapter lifecycle, recv thread
    logger.rs       EventLogger — JSONL line writer (hw_ts_us for KCAN)
    adapters/
      mod.rs        CanAdapter trait, ReceivedFrame, AdapterKind, open_adapter
      peak.rs       PeakAdapter — wraps host_can (PCAN-USB)
      kcan.rs       KCanAdapter — nusb background-thread USB adapter
    eds/
      mod.rs        EDS INI parser; parse_node_id, parse_node_id_str
      types.rs      ObjectDictionary, OdEntry, DataType, AccessType
    canopen/
      mod.rs        COB-ID classification (classify_frame / extract_cob_id)
      nmt.rs        NMT decode (heartbeat, command) + encode_nmt_command
      sdo.rs        Expedited SDO decode (upload / download / abort)
      pdo.rs        PdoDecoder built from EDS TPDO/RPDO mapping objects
    gui/
      mod.rs        egui application — Connect & Monitor screens
    dfu/
      mod.rs        DFU Class 1.1 flash protocol over nusb (wait_for_dfu_interface, flash_firmware)
    firmware_verify.rs  Ed25519 host-side signature verification before DFU
    version_check.rs    Bundled vs GitHub Releases version comparison; background fetch
  assets/
    firmware/
      dongle-h743-app-latest.bin.signed   signed app binary bundled with host release
      dongle-h753-app-latest.bin.signed
  tests/
    integration_test.rs   end-to-end EDS + PDO + SDO + NMT tests
    fixtures/
      sample_drive.eds    CiA 402 servo drive test fixture
tools/
  sign-firmware/    Ed25519 firmware signing tool (key generation, signing, verify)
firmware/           separate Cargo workspace (embedded target)
  Cargo.toml        firmware workspace (bootloader-h743, bootloader-h753, dongle-h743, dongle-h753, lcd-terminal)
  memory.x          STM32H743XI/H753ZI memory map — FLASH 2 MB, DTCM 128 KB,
                    AXI 512 KB, SRAM4 64 KB (.lcd_handoff), SDRAM 32 MB (framebuffer)
  signing-pubkey.bin  32-byte Ed25519 public key (committed; private key in CI secret only)
  bootloader-h743/  embassy-boot bootloader for H743XI: USB DFU class, signature verify, A/B swap
  bootloader-h753/  embassy-boot bootloader for H753ZI: same + OTG-FS SDIS delay
  dongle-h743/      per-crate memory.x (app at 0x08020000); ENTER_DFU_MODE EP0 handler; mark_booted()
  lcd-terminal/     shared no_std crate — LTDC + DMA2D + FMC SDRAM boot console
    build.rs        generates ibm_cp437_1bpp.rs + lcd_handoff.x (NOLOAD linker snippet)
    ibm_cp437_8x16.bin  public-domain IBM CP437 VGA 8×16 font bitmap (4096 bytes)
    src/
      lib.rs        init_or_attach(), LcdTerminal, SdramPins, boot_log! macro
      handoff.rs    LcdHandoff (SRAM4 NOLOAD) — warm-boot cursor / magic 0xCAFE_FEED
      font.rs       FONT_ATLAS — 256-glyph CP437 A8 atlas (32 KB, const)
      sdram.rs      FMC SDRAM init for IS42S32800J-6BLI (Bank2, 32-bit, 133 MHz)
      ltdc.rs       LTDC init for Ampire AM640480GTNQW via CN20 (PLL3R 25 MHz)
      renderer.rs   DMA2D fill_rect / scroll_up / draw_glyph (software pixel writes)
      console.rs    80×30 char-cell Console + BootLogEntry / BootStatus
  dongle-h753/
    src/
      main.rs       Embassy entry point, clock config (PLL1/2/3), task spawning
      kcan_usb.rs   KCanUsbClass — bulk IN/OUT endpoint pair (vendor class)
      can_task.rs   FDCAN1 RX/TX task (Frame ↔ KCanFrame conversion)
      usb_task.rs   USB device task + bulk IN/OUT bridge
      status_task.rs LED heartbeat + periodic STATUS frame every 100 ms
  dongle-h743/
    src/
      main.rs       Embassy entry point — PLL1/2/3, LTDC, SDRAM, task spawning
      kcan_usb.rs   KCanUsbClass — bulk IN/OUT endpoint pair (vendor class)
      can_task.rs   FDCAN1 RX/TX bridge + boot_log! hook
      usb_task.rs   USB device task + bulk IN/OUT bridge + boot_log! hook
      status_task.rs LED heartbeat + boot_log! hook
      display_task.rs Embassy task owning LcdTerminal + LOG_CHANNEL receiver
```
