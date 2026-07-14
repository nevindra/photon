//! Segment frame encoding/decoding.
//!
//! A segment file is a sequence of frames. Each frame is:
//! ```text
//! [u32 LE payload_len][u32 LE crc32(payload)][payload bytes]
//! ```
//! where `payload` is one `RecordBatch` serialized as a self-contained Arrow IPC
//! **stream** (schema message + one record batch + EOS). Making each frame carry its
//! own schema means decoding a frame needs no external state, which keeps torn-tail
//! recovery trivial: scan frames front-to-back, and the first frame whose declared
//! length runs past EOF or whose crc32 mismatches is the torn tail — stop there.

use arrow::error::ArrowError;
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use photon_core::PhotonError;
use std::io::Cursor;

/// Size of the fixed frame header: 4 bytes length + 4 bytes crc32.
const HEADER_LEN: usize = 8;

/// Serialize one `RecordBatch` into a self-contained Arrow IPC stream, framed with a
/// length + crc32 header, ready to append to a segment file — all in a single buffer.
///
/// This builds the frame in one allocation instead of encoding into a scratch buffer and
/// then copying the whole payload into a second, header-prefixed one: the header's
/// `HEADER_LEN` bytes are reserved up front (as a zeroed placeholder), the IPC stream is
/// written directly after them, and once the payload length and crc32 are known the header
/// is patched in place. The on-disk layout is unchanged:
/// `[u32 LE payload_len][u32 LE crc32(payload)][payload bytes]`.
pub(crate) fn frame_batch(batch: &RecordBatch) -> Result<Vec<u8>, PhotonError> {
    let mut buf = vec![0u8; HEADER_LEN];
    {
        let schema = batch.schema();
        let mut writer = StreamWriter::try_new(&mut buf, schema.as_ref())
            .map_err(|e| PhotonError::Wal(format!("ipc writer init: {e}")))?;
        writer
            .write(batch)
            .map_err(|e| PhotonError::Wal(format!("ipc write: {e}")))?;
        writer
            .finish()
            .map_err(|e| PhotonError::Wal(format!("ipc finish: {e}")))?;
    }
    let payload_len = (buf.len() - HEADER_LEN) as u32;
    let crc = crc32fast::hash(&buf[HEADER_LEN..]);
    buf[0..4].copy_from_slice(&payload_len.to_le_bytes());
    buf[4..8].copy_from_slice(&crc.to_le_bytes());
    Ok(buf)
}

/// Decode a single frame payload (an IPC stream) back into record batches. A frame
/// always holds exactly one batch, but the reader is driven to completion regardless.
fn decode_payload(payload: &[u8]) -> Result<Vec<RecordBatch>, ArrowError> {
    let reader = StreamReader::try_new(Cursor::new(payload), None)?;
    let mut out = Vec::new();
    for batch in reader {
        out.push(batch?);
    }
    Ok(out)
}

/// Scan a whole segment file image, returning the recovered batches and the byte offset
/// of the end of the last *valid* frame. Any trailing bytes (`offset < bytes.len()`) are
/// a torn tail: a frame whose length runs past EOF, whose crc32 mismatches, or whose
/// payload fails to decode. Recovery truncates to `offset`; reads simply drop the tail.
pub(crate) fn scan_segment(bytes: &[u8]) -> (Vec<RecordBatch>, usize) {
    let mut offset = 0usize;
    let mut batches = Vec::new();
    loop {
        // Need at least a full header to describe the next frame.
        if offset + HEADER_LEN > bytes.len() {
            break;
        }
        let len = u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let crc = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);
        let payload_start = offset + HEADER_LEN;
        let payload_end = match payload_start.checked_add(len) {
            Some(end) => end,
            None => break, // length overflow -> torn
        };
        if payload_end > bytes.len() {
            break; // frame runs past EOF -> torn tail
        }
        let payload = &bytes[payload_start..payload_end];
        if crc32fast::hash(payload) != crc {
            break; // checksum mismatch -> torn/corrupt tail
        }
        match decode_payload(payload) {
            Ok(mut decoded) => batches.append(&mut decoded),
            Err(_) => break, // undecodable despite crc match -> treat as torn
        }
        offset = payload_end;
    }
    (batches, offset)
}
