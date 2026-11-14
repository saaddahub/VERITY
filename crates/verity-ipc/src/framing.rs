use std::io::{self, ErrorKind};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::MAX_FRAME;

/// Reads one frame. `Ok(None)` means the peer disconnected cleanly between frames.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if matches!(e.kind(), ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe) => {
            return Ok(None)
        }
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("frame of {n} bytes exceeds {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> io::Result<()> {
    if data.len() > MAX_FRAME {
        return Err(io::Error::new(ErrorKind::InvalidInput, "frame too large"));
    }
    w.write_all(&(data.len() as u32).to_le_bytes()).await?;
    w.write_all(data).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_then_clean_eof() {
        let (mut a, mut b) = tokio::io::duplex(64);
        write_frame(&mut a, b"{\"x\":1}").await.unwrap();
        write_frame(&mut a, b"").await.unwrap();
        drop(a);
        assert_eq!(read_frame(&mut b).await.unwrap().unwrap(), b"{\"x\":1}");
        assert_eq!(read_frame(&mut b).await.unwrap().unwrap(), b"");
        assert!(read_frame(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversized_length_is_rejected_before_allocating() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        let e = read_frame(&mut b).await.unwrap_err();
        assert_eq!(e.kind(), ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn truncated_body_is_an_error() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&10u32.to_le_bytes()).await.unwrap();
        a.write_all(b"abc").await.unwrap();
        drop(a);
        assert!(read_frame(&mut b).await.is_err());
    }
}
