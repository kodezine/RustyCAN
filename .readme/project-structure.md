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
  tests/
    integration_test.rs   end-to-end EDS + PDO + SDO + NMT tests
    fixtures/
      sample_drive.eds    CiA 402 servo drive test fixture
firmware/           separate Cargo workspace (embedded target)
  Cargo.toml        firmware workspace (dongle-h753)
  memory.x          STM32H753ZI memory map (FLASH 2 MB, DTCM 128 KB, AXI 512 KB)
  dongle-h753/
    src/
      main.rs       Embassy entry point, clock config (PLL1/2/3), task spawning
      kcan_usb.rs   KCanUsbClass — bulk IN/OUT endpoint pair (vendor class)
      can_task.rs   FDCAN1 RX/TX task (Frame ↔ KCanFrame conversion)
      usb_task.rs   USB device task + bulk IN/OUT bridge
      status_task.rs LED heartbeat + periodic STATUS frame every 100 ms
```
