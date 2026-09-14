//! Length-prefixed framing: `u32 LE len` + protobuf bytes.
//! Max frame 16 MiB (subscription payloads can be sizeable).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::common::{SvcError, SvcResult};

pub const MAX_FRAME: usize = 16 * 1024 * 1024;

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> SvcResult<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return Err(SvcError::Other("empty frame".into()));
    }
    if len > MAX_FRAME {
        return Err(SvcError::Other(format!("frame too large: {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, bytes: &[u8]) -> SvcResult<()> {
    let len = bytes.len() as u32;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(bytes).await?;
    w.flush().await?;
    Ok(())
}
