use std::io;

use async_trait::async_trait;
use futures::prelude::*;
use libp2p::request_response::{self, Codec, ProtocolSupport};
use libp2p::StreamProtocol;
use serde::{Deserialize, Serialize};

use super::SharedFile;

// ============================================================
// Protocol messages
// ============================================================

/// Request sent to a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FileRequest {
    /// Request a file by its content hash.
    #[serde(rename = "get_file")]
    GetFile { hash: String },
    /// Request the peer's full file catalog.
    #[serde(rename = "list_files")]
    ListFiles,
}

/// Response sent back to the requester.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FileResponse {
    /// File data (base64-encoded for JSON transport).
    #[serde(rename = "file_data")]
    FileData {
        name: String,
        size: u64,
        /// Base64-encoded file content.
        data: String,
    },
    /// File catalog listing.
    #[serde(rename = "file_list")]
    FileList { files: Vec<SharedFile> },
    /// Error response.
    #[serde(rename = "error")]
    Error { message: String },
}

// ============================================================
// Codec for the file-sharing protocol
// ============================================================

/// Protocol name.
pub const PROTOCOL_NAME: &str = "/fms-app/file-share/1.0.0";

/// Max message size: 50 MB (covers most model files).
const MAX_MESSAGE_SIZE: usize = 50 * 1024 * 1024;

/// Codec for the file request/response protocol.
/// Uses length-prefixed JSON messages.
#[derive(Debug, Clone, Default)]
pub struct FileShareCodec;

#[async_trait]
impl Codec for FileShareCodec {
    type Protocol = StreamProtocol;
    type Request = FileRequest;
    type Response = FileResponse;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_length_prefixed(io, MAX_MESSAGE_SIZE)
            .await
            .and_then(|bytes| {
                serde_json::from_slice(&bytes).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("JSON decode: {e}"))
                })
            })
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_length_prefixed(io, MAX_MESSAGE_SIZE)
            .await
            .and_then(|bytes| {
                serde_json::from_slice(&bytes).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("JSON decode: {e}"))
                })
            })
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = serde_json::to_vec(&req)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JSON encode: {e}")))?;
        write_length_prefixed(io, &bytes).await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let bytes = serde_json::to_vec(&res)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("JSON encode: {e}")))?;
        write_length_prefixed(io, &bytes).await
    }
}

// ============================================================
// Length-prefixed I/O helpers
// ============================================================

async fn read_length_prefixed<T>(io: &mut T, max_size: usize) -> io::Result<Vec<u8>>
where
    T: AsyncRead + Unpin + Send,
{
    let mut len_buf = [0u8; 4];
    io.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > max_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message too large: {len} > {max_size}"),
        ));
    }
    let mut buf = vec![0u8; len];
    io.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_length_prefixed<T>(io: &mut T, data: &[u8]) -> io::Result<()>
where
    T: AsyncWrite + Unpin + Send,
{
    let len = (data.len() as u32).to_be_bytes();
    io.write_all(&len).await?;
    io.write_all(data).await?;
    io.close().await?;
    Ok(())
}

// ============================================================
// Behaviour type aliases
// ============================================================

pub type FileShareBehaviour = request_response::Behaviour<FileShareCodec>;

/// Build the request-response behaviour for file sharing.
pub fn build_file_share_behaviour() -> FileShareBehaviour {
    request_response::Behaviour::new(
        [(
            StreamProtocol::new(PROTOCOL_NAME),
            ProtocolSupport::Full,
        )],
        request_response::Config::default(),
    )
}
