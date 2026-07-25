//! Dynamic DAQ (Data AcQuisition) configuration tracking and decode.
//!
//! XCP measurement data arrives on the DTO identifier as "DAQ" frames whose
//! first byte (the PID) selects a *DAQ list* / *ODT* (Object Descriptor Table).
//! To decode those frames passively — without an A2L file describing the
//! measurement layout — RustyCAN reconstructs the layout by observing the
//! master's *dynamic DAQ configuration* command sequence on the CRO:
//!
//! ```text
//! SET_DAQ_PTR(daq, odt, odt_entry)      ; select where to write
//! WRITE_DAQ(bit_offset, size, ext, adr) ; append one ODT entry (auto-increments)
//! …                                     ; repeated per entry
//! START_STOP_DAQ_LIST(select, daq) -> RES{first_pid}  ; assigns PIDs
//! ```
//!
//! Each ODT of a started DAQ list is assigned a PID: `first_pid + odt_index`.
//! With that map, an incoming DTO DAQ frame is sliced back into the raw bytes of
//! each configured [`OdtEntry`].
//!
//! Address→name resolution (via A2L) is layered on top by the caller; this
//! module deals only in numeric addresses.

use std::collections::HashMap;

use super::command::XcpCommand;

/// One measurement element within an ODT: a memory region sampled by the slave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OdtEntry {
    /// Base address of the sampled element.
    pub address: u32,
    /// Address extension (segment / page selector).
    pub addr_ext: u8,
    /// Size of the element in bytes.
    pub size: u8,
    /// Bit offset for bit-wise elements; `0xFF` (or `>= 32`) means byte-aligned.
    pub bit_offset: u8,
}

/// One ODT — an ordered list of entries packed contiguously into a DTO frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Odt {
    /// Entries in transmission order (byte layout within the DTO payload).
    pub entries: Vec<OdtEntry>,
}

/// A DAQ list: a set of ODTs plus its assigned base PID once started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaqList {
    /// ODTs indexed by ODT number.
    pub odts: Vec<Odt>,
    /// Base PID assigned by `START_STOP_DAQ_LIST`; `None` until the list starts.
    pub first_pid: Option<u8>,
}

/// One decoded measurement element extracted from a DAQ DTO frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaqSample {
    /// Address of the sampled element (key for A2L name lookup).
    pub address: u32,
    /// Address extension.
    pub addr_ext: u8,
    /// Raw little-/big-endian bytes as they appeared on the bus (`size` long).
    pub raw: Vec<u8>,
}

/// Tracks dynamic DAQ configuration and decodes DAQ DTO frames.
#[derive(Debug, Default)]
pub struct DaqTracker {
    /// DAQ lists keyed by DAQ list number.
    lists: HashMap<u16, DaqList>,
    /// Current DAQ pointer `(daq, odt, odt_entry)` set by `SET_DAQ_PTR`.
    ptr: Option<(u16, u8, u8)>,
    /// PID → `(daq, odt_index)` resolved once lists are started.
    pid_map: HashMap<u8, (u16, usize)>,
    /// Bytes of timestamp inserted at the start of the first ODT payload.
    ///
    /// Most identified-DAQ configurations without timestamp use 0. Callers may
    /// override via [`DaqTracker::set_timestamp_size`] when the slave reports a
    /// DAQ timestamp in `GET_DAQ_PROCESSOR_INFO`.
    timestamp_size: u8,
}

impl DaqTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timestamp field size (bytes) prepended to the first ODT payload.
    pub fn set_timestamp_size(&mut self, bytes: u8) {
        self.timestamp_size = bytes;
    }

    /// Read-only view of the tracked DAQ lists (for UI / diagnostics).
    pub fn lists(&self) -> &HashMap<u16, DaqList> {
        &self.lists
    }

    /// Feed one observed CRO command into the tracker.
    ///
    /// Returns `Some(daq)` when the command is a `START_STOP_DAQ_LIST` in a mode
    /// that will elicit a `first_pid` in the response — the caller should then
    /// pair the next positive response and call [`DaqTracker::on_start_pid`].
    pub fn on_command(&mut self, cmd: &XcpCommand) -> Option<u16> {
        match *cmd {
            XcpCommand::ClearDaqList { daq } => {
                self.lists.remove(&daq);
                self.rebuild_pid_map();
                None
            }
            XcpCommand::SetDaqPtr {
                daq,
                odt,
                odt_entry,
            } => {
                self.ptr = Some((daq, odt, odt_entry));
                let list = self.lists.entry(daq).or_default();
                let need = odt as usize + 1;
                if list.odts.len() < need {
                    list.odts.resize(need, Odt::default());
                }
                None
            }
            XcpCommand::WriteDaq {
                bit_offset,
                size,
                addr_ext,
                address,
            } => {
                if let Some((daq, odt, entry_idx)) = self.ptr {
                    let list = self.lists.entry(daq).or_default();
                    let odt_idx = odt as usize;
                    if list.odts.len() <= odt_idx {
                        list.odts.resize(odt_idx + 1, Odt::default());
                    }
                    let entries = &mut list.odts[odt_idx].entries;
                    let idx = entry_idx as usize;
                    if entries.len() <= idx {
                        entries.resize(
                            idx + 1,
                            OdtEntry {
                                address: 0,
                                addr_ext: 0,
                                size: 0,
                                bit_offset: 0xFF,
                            },
                        );
                    }
                    entries[idx] = OdtEntry {
                        address,
                        addr_ext,
                        size,
                        bit_offset,
                    };
                    // WRITE_DAQ auto-increments the ODT entry pointer.
                    self.ptr = Some((daq, odt, entry_idx.wrapping_add(1)));
                }
                None
            }
            XcpCommand::StartStopDaqList { mode, daq } => {
                // mode: 0 = stop, 1 = start, 2 = select. Start/select return first_pid.
                if mode == 0x01 || mode == 0x02 {
                    Some(daq)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Record the `first_pid` returned by a `START_STOP_DAQ_LIST` response and
    /// (re)assign PIDs to the DAQ list's ODTs.
    pub fn on_start_pid(&mut self, daq: u16, first_pid: u8) {
        if let Some(list) = self.lists.get_mut(&daq) {
            list.first_pid = Some(first_pid);
        }
        self.rebuild_pid_map();
    }

    /// Rebuild the PID → `(daq, odt_index)` lookup from all started lists.
    fn rebuild_pid_map(&mut self) {
        self.pid_map.clear();
        for (&daq, list) in &self.lists {
            if let Some(first) = list.first_pid {
                for odt_idx in 0..list.odts.len() {
                    let pid = first.wrapping_add(odt_idx as u8);
                    self.pid_map.insert(pid, (daq, odt_idx));
                }
            }
        }
    }

    /// Decode a DAQ DTO frame (`data[0]` is the PID) into samples.
    ///
    /// Returns `None` when the PID is not mapped to a known ODT. Entries whose
    /// bytes fall outside the received payload are skipped defensively (partial
    /// frames therefore yield the entries that did fit).
    pub fn decode(&self, data: &[u8]) -> Option<Vec<DaqSample>> {
        let pid = *data.first()?;
        let &(daq, odt_idx) = self.pid_map.get(&pid)?;
        let odt = self.lists.get(&daq)?.odts.get(odt_idx)?;

        // Payload begins after the PID byte; the first ODT may carry a timestamp.
        let mut offset = 1usize;
        if odt_idx == 0 {
            offset += self.timestamp_size as usize;
        }

        let mut samples = Vec::with_capacity(odt.entries.len());
        for entry in &odt.entries {
            // Skip placeholder / unconfigured entries (size 0) so they neither
            // emit empty samples nor stall the offset for following entries.
            if entry.size == 0 {
                continue;
            }
            let end = offset + entry.size as usize;
            if end > data.len() {
                break;
            }
            samples.push(DaqSample {
                address: entry.address,
                addr_ext: entry.addr_ext,
                raw: data[offset..end].to_vec(),
            });
            offset = end;
        }
        Some(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Configure a single DAQ list (daq 0) with one ODT holding two u16 entries.
    fn configure_two_u16() -> DaqTracker {
        let mut t = DaqTracker::new();
        t.on_command(&XcpCommand::SetDaqPtr {
            daq: 0,
            odt: 0,
            odt_entry: 0,
        });
        t.on_command(&XcpCommand::WriteDaq {
            bit_offset: 0xFF,
            size: 2,
            addr_ext: 0,
            address: 0x2000_0000,
        });
        t.on_command(&XcpCommand::WriteDaq {
            bit_offset: 0xFF,
            size: 2,
            addr_ext: 0,
            address: 0x2000_0002,
        });
        t
    }

    #[test]
    fn write_daq_appends_entries_and_autoincrements() {
        let t = configure_two_u16();
        let list = &t.lists()[&0];
        assert_eq!(list.odts.len(), 1);
        assert_eq!(list.odts[0].entries.len(), 2);
        assert_eq!(list.odts[0].entries[0].address, 0x2000_0000);
        assert_eq!(list.odts[0].entries[1].address, 0x2000_0002);
    }

    #[test]
    fn start_returns_daq_and_assigns_pid() {
        let mut t = configure_two_u16();
        let daq = t.on_command(&XcpCommand::StartStopDaqList { mode: 0x02, daq: 0 });
        assert_eq!(daq, Some(0));
        t.on_start_pid(0, 0x10);
        assert_eq!(t.lists()[&0].first_pid, Some(0x10));
    }

    #[test]
    fn decode_maps_pid_to_entries() {
        let mut t = configure_two_u16();
        t.on_command(&XcpCommand::StartStopDaqList { mode: 0x01, daq: 0 });
        t.on_start_pid(0, 0x10);
        // DTO: PID=0x10, then two u16 values.
        let samples = t.decode(&[0x10, 0xAA, 0xBB, 0xCC, 0xDD]).unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].address, 0x2000_0000);
        assert_eq!(samples[0].raw, vec![0xAA, 0xBB]);
        assert_eq!(samples[1].address, 0x2000_0002);
        assert_eq!(samples[1].raw, vec![0xCC, 0xDD]);
    }

    #[test]
    fn decode_unknown_pid_is_none() {
        let t = configure_two_u16();
        assert!(t.decode(&[0x99, 0x00, 0x00]).is_none());
    }

    #[test]
    fn decode_skips_zero_size_placeholder_entries() {
        // Point at ODT entry index 1, leaving index 0 as a size-0 placeholder.
        let mut t = DaqTracker::new();
        t.on_command(&XcpCommand::SetDaqPtr {
            daq: 0,
            odt: 0,
            odt_entry: 1,
        });
        t.on_command(&XcpCommand::WriteDaq {
            bit_offset: 0xFF,
            size: 2,
            addr_ext: 0,
            address: 0x2000_0010,
        });
        t.on_command(&XcpCommand::StartStopDaqList { mode: 0x01, daq: 0 });
        t.on_start_pid(0, 0x10);
        // The placeholder (index 0, size 0) must be skipped, and the real entry
        // decoded from offset 1 — not shifted or duplicated.
        let samples = t.decode(&[0x10, 0xAA, 0xBB]).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].address, 0x2000_0010);
        assert_eq!(samples[0].raw, vec![0xAA, 0xBB]);
    }

    #[test]
    fn decode_respects_timestamp_offset() {
        let mut t = configure_two_u16();
        t.set_timestamp_size(2);
        t.on_command(&XcpCommand::StartStopDaqList { mode: 0x01, daq: 0 });
        t.on_start_pid(0, 0x20);
        // PID + 2 timestamp bytes + two u16 values.
        let samples = t
            .decode(&[0x20, 0x00, 0x01, 0xAA, 0xBB, 0xCC, 0xDD])
            .unwrap();
        assert_eq!(samples[0].raw, vec![0xAA, 0xBB]);
        assert_eq!(samples[1].raw, vec![0xCC, 0xDD]);
    }

    #[test]
    fn partial_frame_yields_fitting_entries() {
        let mut t = configure_two_u16();
        t.on_command(&XcpCommand::StartStopDaqList { mode: 0x01, daq: 0 });
        t.on_start_pid(0, 0x10);
        // Only room for the first u16.
        let samples = t.decode(&[0x10, 0xAA, 0xBB]).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].raw, vec![0xAA, 0xBB]);
    }

    #[test]
    fn clear_daq_list_forgets_config() {
        let mut t = configure_two_u16();
        t.on_command(&XcpCommand::StartStopDaqList { mode: 0x01, daq: 0 });
        t.on_start_pid(0, 0x10);
        t.on_command(&XcpCommand::ClearDaqList { daq: 0 });
        assert!(t.lists().get(&0).is_none());
        assert!(t.decode(&[0x10, 0xAA, 0xBB]).is_none());
    }

    #[test]
    fn multi_odt_pid_assignment() {
        let mut t = DaqTracker::new();
        // ODT 0 with one entry, ODT 1 with one entry.
        t.on_command(&XcpCommand::SetDaqPtr {
            daq: 3,
            odt: 0,
            odt_entry: 0,
        });
        t.on_command(&XcpCommand::WriteDaq {
            bit_offset: 0xFF,
            size: 1,
            addr_ext: 0,
            address: 0x100,
        });
        t.on_command(&XcpCommand::SetDaqPtr {
            daq: 3,
            odt: 1,
            odt_entry: 0,
        });
        t.on_command(&XcpCommand::WriteDaq {
            bit_offset: 0xFF,
            size: 1,
            addr_ext: 0,
            address: 0x200,
        });
        t.on_command(&XcpCommand::StartStopDaqList { mode: 0x01, daq: 3 });
        t.on_start_pid(3, 0x40);
        // PID 0x40 -> odt 0 (address 0x100), PID 0x41 -> odt 1 (address 0x200).
        assert_eq!(t.decode(&[0x40, 0x11]).unwrap()[0].address, 0x100);
        assert_eq!(t.decode(&[0x41, 0x22]).unwrap()[0].address, 0x200);
    }
}
