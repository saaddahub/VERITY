//! Chunk formats: a version byte, a little-endian count, then one array per
//! column, all zstd-compressed. Columnar because same-typed values compress well.

use verity_core::metrics::Frame;

use crate::StoreError;

const VERSION: u8 = 1;
const ZSTD_LEVEL: i32 = 3;
/// Upper bound when decompressing, so a corrupt blob cannot exhaust memory.
const MAX_CHUNK_BYTES: usize = 64 << 20;

fn compress(raw: &[u8]) -> Result<Vec<u8>, StoreError> {
    zstd::bulk::compress(raw, ZSTD_LEVEL).map_err(|e| StoreError::Codec(e.to_string()))
}

/// Decompresses and checks the header; returns the payload and the item count.
fn open(blob: &[u8], bytes_per_item: usize) -> Result<(Vec<u8>, usize), StoreError> {
    let raw = zstd::bulk::decompress(blob, MAX_CHUNK_BYTES)
        .map_err(|e| StoreError::Codec(e.to_string()))?;
    let bad = |why: &str| StoreError::Codec(why.to_string());
    let (&version, rest) = raw.split_first().ok_or_else(|| bad("empty chunk"))?;
    if version != VERSION {
        return Err(StoreError::Codec(format!(
            "chunk format {version}, expected {VERSION}"
        )));
    }
    let (count, body) = rest
        .split_first_chunk::<4>()
        .ok_or_else(|| bad("no count"))?;
    let n = u32::from_le_bytes(*count) as usize;
    if body.len() != n * bytes_per_item {
        return Err(bad("chunk length does not match its count"));
    }
    Ok((body.to_vec(), n))
}

fn header(n: usize, bytes_per_item: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(5 + n * bytes_per_item);
    raw.push(VERSION);
    raw.extend((n as u32).to_le_bytes());
    raw
}

fn u64s(body: &[u8]) -> impl Iterator<Item = u64> + '_ {
    body.as_chunks::<8>()
        .0
        .iter()
        .map(|b| u64::from_le_bytes(*b))
}

fn f32s(body: &[u8]) -> impl Iterator<Item = f32> + '_ {
    body.as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
}

const FRAME_BYTES: usize = 8 + 5 * 4 + 2;

pub fn encode_frames(frames: &[Frame]) -> Result<Vec<u8>, StoreError> {
    let mut raw = header(frames.len(), FRAME_BYTES);
    raw.extend(frames.iter().flat_map(|f| f.qpc.to_le_bytes()));
    for column in [
        |f: &Frame| f.frame_time_ms,
        |f: &Frame| f.cpu_busy_ms,
        |f: &Frame| f.gpu_busy_ms,
        |f: &Frame| f.displayed_time_ms,
        |f: &Frame| f.display_latency_ms,
    ] {
        raw.extend(frames.iter().flat_map(|f| column(f).to_le_bytes()));
    }
    raw.extend(frames.iter().map(|f| f.present_mode));
    raw.extend(frames.iter().map(|f| f.flags));
    compress(&raw)
}

pub fn decode_frames(blob: &[u8]) -> Result<Vec<Frame>, StoreError> {
    let (body, n) = open(blob, FRAME_BYTES)?;
    let (qpc, rest) = body.split_at(n * 8);
    let (floats, bytes) = rest.split_at(n * 20);
    let column = |i: usize| f32s(&floats[i * n * 4..(i + 1) * n * 4]);
    let mut cols = [column(0), column(1), column(2), column(3), column(4)];
    let mut next = |i: usize| cols[i].next().unwrap_or(f32::NAN);
    Ok(u64s(qpc)
        .enumerate()
        .map(|(i, qpc)| Frame {
            qpc,
            frame_time_ms: next(0),
            cpu_busy_ms: next(1),
            gpu_busy_ms: next(2),
            displayed_time_ms: next(3),
            display_latency_ms: next(4),
            present_mode: bytes[i],
            flags: bytes[n + i],
        })
        .collect())
}

/// A counter series: (QPC, value) pairs.
pub fn encode_series(points: &[(u64, f32)]) -> Result<Vec<u8>, StoreError> {
    let mut raw = header(points.len(), 12);
    raw.extend(points.iter().flat_map(|p| p.0.to_le_bytes()));
    raw.extend(points.iter().flat_map(|p| p.1.to_le_bytes()));
    compress(&raw)
}

pub fn decode_series(blob: &[u8]) -> Result<Vec<(u64, f32)>, StoreError> {
    let (body, n) = open(blob, 12)?;
    let (qpc, values) = body.split_at(n * 8);
    Ok(u64s(qpc).zip(f32s(values)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(i: u64) -> Frame {
        Frame {
            qpc: 1_580_000_000_000 + i * 166_667,
            frame_time_ms: 16.6 + i as f32 * 0.001,
            cpu_busy_ms: 3.25,
            gpu_busy_ms: 0.2,
            displayed_time_ms: if i.is_multiple_of(7) { f32::NAN } else { 16.7 },
            display_latency_ms: f32::NAN,
            present_mode: (i % 8) as u8,
            flags: (i % 4) as u8,
        }
    }

    /// NaN != NaN, so compare bit patterns.
    fn bits(f: &Frame) -> (u64, [u32; 5], u8, u8) {
        (
            f.qpc,
            [
                f.frame_time_ms,
                f.cpu_busy_ms,
                f.gpu_busy_ms,
                f.displayed_time_ms,
                f.display_latency_ms,
            ]
            .map(f32::to_bits),
            f.present_mode,
            f.flags,
        )
    }

    #[test]
    fn frames_round_trip_exactly() {
        let frames: Vec<Frame> = (0..1500).map(frame).collect();
        let blob = encode_frames(&frames).unwrap();
        assert!(blob.len() < frames.len() * FRAME_BYTES / 2, "compresses");
        let back = decode_frames(&blob).unwrap();
        assert_eq!(back.len(), frames.len());
        assert!(back.iter().zip(&frames).all(|(a, b)| bits(a) == bits(b)));
        assert!(decode_frames(&encode_frames(&[]).unwrap())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn series_round_trip() {
        let points: Vec<(u64, f32)> = (0..100).map(|i| (i * 10_000_000, i as f32 * 0.5)).collect();
        assert_eq!(
            decode_series(&encode_series(&points).unwrap()).unwrap(),
            points
        );
    }

    #[test]
    fn corrupt_chunks_are_errors_not_panics() {
        assert!(decode_frames(b"not zstd").is_err());
        let wrong_version = zstd::bulk::compress(&[9, 0, 0, 0, 0], 3).unwrap();
        assert!(decode_frames(&wrong_version).is_err());
        let short = zstd::bulk::compress(&[1, 5, 0, 0, 0, 1, 2, 3], 3).unwrap();
        assert!(decode_frames(&short).is_err());
        assert!(decode_series(&zstd::bulk::compress(&[], 3).unwrap()).is_err());
    }
}
