use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::{RcwError, RcwResult};

pub const CHUNK_SIZE: usize = 64 * 1024;
pub const BINARY_FRAME_HEADER_LEN: usize = 1 + 16 + 4 + 4 + 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BinaryKind {
    UploadChunk = 1,
    DownloadChunk = 2,
    ScreenshotChunk = 3,
}

impl TryFrom<u8> for BinaryKind {
    type Error = RcwError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::UploadChunk),
            2 => Ok(Self::DownloadChunk),
            3 => Ok(Self::ScreenshotChunk),
            other => Err(RcwError::Protocol(format!(
                "unsupported binary frame kind: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BinaryFrame {
    pub kind: BinaryKind,
    pub request_id: String,
    pub sequence: u32,
    pub total_sequences: u32,
    pub payload: Vec<u8>,
}

impl BinaryFrame {
    pub fn encode(&self) -> RcwResult<Vec<u8>> {
        let ulid = Ulid::from_string(&self.request_id)
            .map_err(|err| RcwError::Protocol(format!("request_id is not a ULID: {err}")))?;
        let payload_len = u32::try_from(self.payload.len()).map_err(|_| {
            RcwError::Protocol("binary frame payload is too large for u32 length".to_owned())
        })?;
        let mut bytes = Vec::with_capacity(BINARY_FRAME_HEADER_LEN + self.payload.len());
        bytes.push(self.kind as u8);
        bytes.extend_from_slice(&ulid.to_bytes());
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&self.total_sequences.to_be_bytes());
        bytes.extend_from_slice(&payload_len.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> RcwResult<Self> {
        if bytes.len() < BINARY_FRAME_HEADER_LEN {
            return Err(RcwError::Protocol("binary frame is too short".to_owned()));
        }
        let kind = BinaryKind::try_from(bytes[0])?;
        let mut request_id = [0_u8; 16];
        request_id.copy_from_slice(&bytes[1..17]);
        let request_id = Ulid::from_bytes(request_id).to_string();
        let sequence = read_u32_be(bytes, 17, "sequence")?;
        let total_sequences = read_u32_be(bytes, 21, "total_sequences")?;
        let payload_len = read_u32_be(bytes, 25, "payload_len")? as usize;
        let payload_start = BINARY_FRAME_HEADER_LEN;
        let payload_end = payload_start
            .checked_add(payload_len)
            .ok_or_else(|| RcwError::Protocol("binary frame payload length overflow".to_owned()))?;
        if bytes.len() != payload_end {
            return Err(RcwError::Protocol(format!(
                "binary frame payload length mismatch: header={payload_len}, actual={}",
                bytes.len().saturating_sub(payload_start)
            )));
        }
        Ok(Self {
            kind,
            request_id,
            sequence,
            total_sequences,
            payload: bytes[payload_start..payload_end].to_vec(),
        })
    }
}

fn read_u32_be(bytes: &[u8], offset: usize, field: &str) -> RcwResult<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| RcwError::Protocol(format!("{field} offset overflow")))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| RcwError::Protocol(format!("binary frame missing {field}")))?;
    let array: [u8; 4] = slice
        .try_into()
        .map_err(|_| RcwError::Protocol(format!("binary frame invalid {field} length")))?;
    Ok(u32::from_be_bytes(array))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

pub struct Sha256Accumulator {
    hasher: Sha256,
}

impl Sha256Accumulator {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
    }

    pub fn finalize(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}

impl Default for Sha256Accumulator {
    fn default() -> Self {
        Self::new()
    }
}

pub fn sha256_file(path: impl AsRef<Path>) -> RcwResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn write_all_new(path: impl AsRef<Path>, bytes: &[u8], overwrite: bool) -> RcwResult<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    if overwrite {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

pub fn total_sequences_for_len(len: u64) -> RcwResult<u32> {
    let total_sequences = len.div_ceil(CHUNK_SIZE as u64).max(1);
    u32::try_from(total_sequences)
        .map_err(|_| RcwError::Protocol("too many chunks for u32 sequence count".to_owned()))
}

pub struct FileBinaryFrameReader {
    path: PathBuf,
    request_id: String,
    kind: BinaryKind,
    file: Option<File>,
    buffer: Vec<u8>,
    remaining: u64,
    total_sequences: u32,
    next_sequence: u32,
    finished: bool,
    hasher: Sha256Accumulator,
}

impl FileBinaryFrameReader {
    pub fn new(
        path: impl AsRef<Path>,
        size: u64,
        request_id: impl Into<String>,
        kind: BinaryKind,
    ) -> RcwResult<Self> {
        let path = path.as_ref().to_path_buf();
        let file = if size == 0 {
            None
        } else {
            Some(File::open(&path).map_err(|err| {
                RcwError::Other(format!("failed to open {}: {err}", path.display()))
            })?)
        };
        Ok(Self {
            path,
            request_id: request_id.into(),
            kind,
            file,
            buffer: vec![0_u8; CHUNK_SIZE],
            remaining: size,
            total_sequences: total_sequences_for_len(size)?,
            next_sequence: 0,
            finished: false,
            hasher: Sha256Accumulator::new(),
        })
    }

    pub fn next_frame(&mut self) -> RcwResult<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }
        if self.remaining == 0 {
            self.finished = true;
            return BinaryFrame {
                kind: self.kind,
                request_id: self.request_id.clone(),
                sequence: self.next_sequence,
                total_sequences: self.total_sequences,
                payload: Vec::new(),
            }
            .encode()
            .map(Some);
        }

        let chunk_len = self.remaining.min(CHUNK_SIZE as u64) as usize;
        let file = self.file.as_mut().ok_or_else(|| {
            RcwError::Protocol("file reader missing for non-empty transfer".to_owned())
        })?;
        file.read_exact(&mut self.buffer[..chunk_len])
            .map_err(|err| {
                RcwError::Other(format!("failed to read {}: {err}", self.path.display()))
            })?;
        self.hasher.update(&self.buffer[..chunk_len]);
        let frame = BinaryFrame {
            kind: self.kind,
            request_id: self.request_id.clone(),
            sequence: self.next_sequence,
            total_sequences: self.total_sequences,
            payload: self.buffer[..chunk_len].to_vec(),
        }
        .encode()?;
        self.next_sequence += 1;
        self.remaining -= chunk_len as u64;
        if self.remaining == 0 {
            self.finished = true;
        }
        Ok(Some(frame))
    }

    pub fn finalize_sha256(self) -> String {
        self.hasher.finalize()
    }
}

pub fn temp_output_path(path: &Path, request_id: &str) -> PathBuf {
    let mut temp_name = OsString::from(".");
    match path.file_name() {
        Some(file_name) => temp_name.push(file_name),
        None => temp_name.push("rcw-output"),
    }
    temp_name.push(".");
    temp_name.push(request_id);
    temp_name.push(".part");
    path.with_file_name(temp_name)
}

pub fn create_temp_output_file(path: &Path, temp_path: &Path, overwrite: bool) -> RcwResult<File> {
    if temp_path == path {
        return Err(RcwError::Other(format!(
            "temporary output path matches final output path {}",
            path.display()
        )));
    }
    if !overwrite && path.exists() {
        return Err(RcwError::Other(format!(
            "refusing to overwrite existing file {}",
            path.display()
        )));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::remove_file(temp_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|err| RcwError::Other(format!("failed to create {}: {err}", temp_path.display())))
}

pub fn commit_temp_output_file(temp_path: &Path, path: &Path, overwrite: bool) -> RcwResult<()> {
    if overwrite {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    } else if path.exists() {
        return Err(RcwError::Other(format!(
            "refusing to overwrite existing file {}",
            path.display()
        )));
    }
    std::fs::rename(temp_path, path).map_err(|err| {
        RcwError::Other(format!(
            "failed to move {} to {}: {err}",
            temp_path.display(),
            path.display()
        ))
    })?;
    Ok(())
}

pub fn chunk_binary(request_id: &str, kind: BinaryKind, bytes: &[u8]) -> RcwResult<Vec<Vec<u8>>> {
    let total_sequences = total_sequences_for_len(bytes.len() as u64)?;
    let mut frames = Vec::new();
    if bytes.is_empty() {
        frames.push(
            BinaryFrame {
                kind,
                request_id: request_id.to_owned(),
                sequence: 0,
                total_sequences,
                payload: Vec::new(),
            }
            .encode()?,
        );
        return Ok(frames);
    }
    for (index, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
        frames.push(
            BinaryFrame {
                kind,
                request_id: request_id.to_owned(),
                sequence: u32::try_from(index)
                    .map_err(|_| RcwError::Protocol("chunk index exceeded u32".to_owned()))?,
                total_sequences,
                payload: chunk.to_vec(),
            }
            .encode()?,
        );
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    #[test]
    fn binary_frame_round_trips() {
        let frame = BinaryFrame {
            kind: BinaryKind::DownloadChunk,
            request_id: REQUEST_ID.to_owned(),
            sequence: 7,
            total_sequences: 9,
            payload: b"hello".to_vec(),
        };

        let encoded = frame.encode().unwrap();
        let decoded = BinaryFrame::decode(&encoded).unwrap();

        assert_eq!(decoded.kind, frame.kind);
        assert_eq!(decoded.request_id, frame.request_id);
        assert_eq!(decoded.sequence, frame.sequence);
        assert_eq!(decoded.total_sequences, frame.total_sequences);
        assert_eq!(decoded.payload, frame.payload);
    }

    #[test]
    fn binary_frame_rejects_short_header() {
        let err = BinaryFrame::decode(&[BinaryKind::UploadChunk as u8]).unwrap_err();

        assert!(err.to_string().contains("binary frame is too short"));
    }

    #[test]
    fn binary_frame_rejects_unknown_kind() {
        let mut encoded = BinaryFrame {
            kind: BinaryKind::UploadChunk,
            request_id: REQUEST_ID.to_owned(),
            sequence: 0,
            total_sequences: 1,
            payload: Vec::new(),
        }
        .encode()
        .unwrap();
        encoded[0] = 99;

        let err = BinaryFrame::decode(&encoded).unwrap_err();

        assert!(err.to_string().contains("unsupported binary frame kind"));
    }

    #[test]
    fn binary_frame_rejects_payload_length_mismatch() {
        let mut encoded = BinaryFrame {
            kind: BinaryKind::UploadChunk,
            request_id: REQUEST_ID.to_owned(),
            sequence: 0,
            total_sequences: 1,
            payload: b"abc".to_vec(),
        }
        .encode()
        .unwrap();
        encoded.truncate(encoded.len() - 1);

        let err = BinaryFrame::decode(&encoded).unwrap_err();

        assert!(err
            .to_string()
            .contains("binary frame payload length mismatch"));
    }

    #[test]
    fn file_binary_frame_reader_emits_single_empty_frame() {
        let path = temp_transfer_path("empty");
        std::fs::write(&path, []).unwrap();
        let mut reader =
            FileBinaryFrameReader::new(&path, 0, REQUEST_ID, BinaryKind::DownloadChunk).unwrap();

        let first = reader.next_frame().unwrap().unwrap();
        let second = reader.next_frame().unwrap();

        let decoded = BinaryFrame::decode(&first).unwrap();
        assert_eq!(decoded.sequence, 0);
        assert_eq!(decoded.total_sequences, 1);
        assert!(decoded.payload.is_empty());
        assert!(second.is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_binary_frame_reader_stops_after_last_chunk() {
        let path = temp_transfer_path("data");
        std::fs::write(&path, b"abc").unwrap();
        let mut reader =
            FileBinaryFrameReader::new(&path, 3, REQUEST_ID, BinaryKind::UploadChunk).unwrap();

        let first = reader.next_frame().unwrap().unwrap();
        let second = reader.next_frame().unwrap();

        let decoded = BinaryFrame::decode(&first).unwrap();
        assert_eq!(decoded.sequence, 0);
        assert_eq!(decoded.total_sequences, 1);
        assert_eq!(decoded.payload, b"abc");
        assert!(second.is_none());

        let _ = std::fs::remove_file(&path);
    }

    fn temp_transfer_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rcw-transfer-{label}-{}", Ulid::new()))
    }
}
