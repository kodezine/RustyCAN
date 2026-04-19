//! Binary Block File parser.
//!
//! The `.bin` file used by the CANopen bootloader toolchain consists of a
//! sequence of binary blocks. Each block has the following layout (all integers little-endian):
//!
//! ```text
//! ┌─────────────────┐
//! │ block_num  u32  │  4 bytes
//! ├─────────────────┤
//! │ flash_addr u32  │  4 bytes
//! ├─────────────────┤
//! │ data_size  u32  │  4 bytes
//! ├─────────────────┤
//! │ data      [u8]  │  data_size bytes
//! ├─────────────────┤
//! │ crc32      u32  │  4 bytes  (CRC over entire block from block_num)
//! └─────────────────┘
//! ```
//!
//! The **entire raw block** (header + data + CRC) is what gets sent wholesale
//! to the bootloader via SDO download to object 0x1F50/prog_no — the bootloader
//! verifies the embedded CRC itself.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

/// A parsed binary block from the firmware file.
#[derive(Debug)]
#[allow(dead_code)] // flash_addr retained for informational use / future logging
pub struct BinaryBlock {
    /// Block sequence number from the file header.
    pub block_num: u32,
    /// Target flash address (informational; the bootloader uses this from the header).
    pub flash_addr: u32,
    /// Raw bytes of this block including the 12-byte header and trailing CRC.
    ///
    /// This is the exact buffer that must be sent to object 0x1F50 via SDO.
    pub raw: Vec<u8>,
}

impl BinaryBlock {
    /// Number of payload data bytes (excluding header and CRC).
    #[allow(dead_code)]
    pub fn data_size(&self) -> usize {
        self.raw.len().saturating_sub(16) // 12 header + 4 CRC
    }
}

/// Errors produced by the binary block file parser.
#[derive(Debug)]
pub enum FileError {
    /// The file could not be opened or read.
    Io(io::Error),
    /// The file contains a block whose format is invalid (truncated, etc.).
    InvalidFormat(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::InvalidFormat(s) => write!(f, "invalid block format: {s}"),
        }
    }
}

impl From<io::Error> for FileError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Iterator over the binary blocks in a firmware file.
pub struct BinaryBlockIter {
    reader: BufReader<File>,
    /// Total file size in bytes (used for progress reporting).
    pub total_size: u64,
    bytes_read: u64,
}

impl BinaryBlockIter {
    /// Open the file at `path` and prepare to iterate over its blocks.
    pub fn open(path: &Path) -> Result<Self, FileError> {
        let file = File::open(path)?;
        let total_size = file.metadata()?.len();
        Ok(Self {
            reader: BufReader::new(file),
            total_size,
            bytes_read: 0,
        })
    }

    /// Bytes consumed so far (for progress reporting).
    #[allow(dead_code)]
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
}

impl Iterator for BinaryBlockIter {
    type Item = Result<BinaryBlock, FileError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Read the 12-byte header: [block_num u32][flash_addr u32][data_size u32]
        let mut header = [0u8; 12];
        match self.reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                // A zero-byte read at the start of a record means end of file.
                return None;
            }
            Err(e) => return Some(Err(FileError::Io(e))),
        }

        let block_num = u32::from_le_bytes(header[0..4].try_into().unwrap());
        let flash_addr = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let data_size = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;

        // Total raw size: 12-byte header + data_size bytes + 4-byte CRC
        let total = 12 + data_size + 4;
        let mut raw = vec![0u8; total];
        raw[..12].copy_from_slice(&header);

        // Read remaining data + CRC into the buffer after the header
        if let Err(e) = self.reader.read_exact(&mut raw[12..]) {
            return Some(Err(FileError::InvalidFormat(format!(
                "truncated block {block_num}: expected {data_size} data bytes + 4 CRC bytes, got: {e}"
            ))));
        }

        self.bytes_read += total as u64;

        Some(Ok(BinaryBlock {
            block_num,
            flash_addr,
            raw,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_block(buf: &mut Vec<u8>, block_num: u32, flash_addr: u32, data: &[u8]) {
        buf.extend_from_slice(&block_num.to_le_bytes());
        buf.extend_from_slice(&flash_addr.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(data);
        // Dummy CRC (the parser does not verify it — bootloader does)
        buf.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    }

    #[test]
    fn parses_two_blocks() {
        let mut raw = Vec::new();
        write_block(&mut raw, 1, 0x0800_0000, &[0xAA; 16]);
        write_block(&mut raw, 2, 0x0800_0010, &[0xBB; 32]);

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&raw).unwrap();
        tmp.flush().unwrap();

        let blocks: Vec<_> = BinaryBlockIter::open(tmp.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_num, 1);
        assert_eq!(blocks[0].flash_addr, 0x0800_0000);
        assert_eq!(blocks[0].data_size(), 16);
        assert_eq!(blocks[0].raw.len(), 12 + 16 + 4);

        assert_eq!(blocks[1].block_num, 2);
        assert_eq!(blocks[1].flash_addr, 0x0800_0010);
        assert_eq!(blocks[1].data_size(), 32);
        assert_eq!(blocks[1].raw.len(), 12 + 32 + 4);
    }

    #[test]
    fn empty_file_yields_no_blocks() {
        let tmp = NamedTempFile::new().unwrap();
        let blocks: Vec<_> = BinaryBlockIter::open(tmp.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn truncated_data_returns_error() {
        let mut raw = Vec::new();
        // Write header claiming 16 bytes of data, but only supply 8
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&16u32.to_le_bytes()); // claims 16 bytes
        raw.extend_from_slice(&[0u8; 8]); // only 8 bytes

        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&raw).unwrap();
        tmp.flush().unwrap();

        let mut iter = BinaryBlockIter::open(tmp.path()).unwrap();
        assert!(matches!(
            iter.next(),
            Some(Err(FileError::InvalidFormat(_)))
        ));
    }
}
