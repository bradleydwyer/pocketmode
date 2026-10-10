use crate::{COLOR_GAMMA_KEY, Mode};
use std::io::{self, Read};

// Only metadata is buffered; video payloads are streamed through the fingerprint
// scanner. Bound allocation and nesting even for malformed input files.
const MAX_METADATA_SIZE: u64 = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 32;

#[derive(Default)]
pub(crate) struct DirectMetadata {
    pub(crate) found: bool,
    value: Option<Mode>,
    ambiguous: bool,
}

impl DirectMetadata {
    pub(crate) fn mode(&self) -> Option<Mode> {
        self.value.filter(|_| !self.ambiguous)
    }

    fn record(&mut self, value: Option<Mode>) {
        if value.is_none() || self.value.is_some_and(|previous| Some(previous) != value) {
            self.ambiguous = true;
        }
        if self.value.is_none() {
            self.value = value;
        }
    }
}

pub(crate) fn inspect(reader: &mut dyn Read, buffer_size: usize) -> io::Result<DirectMetadata> {
    let mut metadata = DirectMetadata::default();
    let mut scratch = vec![0; buffer_size.max(1)];
    inspect_atoms(reader, u64::MAX, 0, &mut scratch, &mut metadata)?;
    drain(reader, &mut scratch)?;
    Ok(metadata)
}

fn drain(reader: &mut dyn Read, scratch: &mut [u8]) -> io::Result<()> {
    loop {
        match reader.read(scratch) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn inspect_atoms(
    reader: &mut dyn Read,
    mut remaining: u64,
    depth: usize,
    scratch: &mut [u8],
    metadata: &mut DirectMetadata,
) -> io::Result<()> {
    while remaining >= 8 {
        let mut header = [0; 8];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let kind = &header[4..];
        let (size, header_size) = match size {
            0 => (remaining, 8),
            1 if remaining >= 16 => {
                let mut extended = [0; 8];
                match reader.read_exact(&mut extended) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                    Err(error) => return Err(error),
                }
                (u64::from_be_bytes(extended), 16)
            }
            size => (u64::from(size), 8),
        };
        if size < header_size || size > remaining {
            return Ok(());
        }
        let payload_size = size - header_size;
        let mut payload = reader.take(payload_size);
        match kind {
            b"moov" | b"udta" | b"trak" | b"mdia" if depth < MAX_DEPTH => {
                inspect_atoms(&mut payload, payload_size, depth + 1, scratch, metadata)?;
            }
            b"meta" if payload_size <= MAX_METADATA_SIZE => {
                let mut bytes = Vec::new();
                payload.read_to_end(&mut bytes)?;
                if bytes.len() as u64 == payload_size {
                    parse_meta(&bytes, metadata);
                }
            }
            _ => {}
        }
        drain(&mut payload, scratch)?;
        if payload.limit() != 0 {
            return Ok(());
        }
        remaining -= size;
    }
    Ok(())
}

// QuickTime meta atoms have no version/flags prefix; ISO BMFF meta boxes do.
// Resolve keys and ilst within the same meta atom, regardless of child ordering.
fn parse_meta(mut bytes: &[u8], metadata: &mut DirectMetadata) {
    if bytes.starts_with(&[0; 4]) {
        bytes = &bytes[4..];
    }
    let mut keys = None;
    let mut items = None;
    while !bytes.is_empty() {
        let Some((kind, payload)) = next_atom(&mut bytes) else {
            return;
        };
        match &kind {
            b"keys" if keys.is_none() => keys = Some(payload),
            b"ilst" if items.is_none() => items = Some(payload),
            b"keys" | b"ilst" => return,
            _ => {}
        }
    }
    let Some(indices) = keys.and_then(color_gamma_indices) else {
        return;
    };
    if indices.is_empty() {
        return;
    }
    metadata.found = true;
    let Some(mut items) = items else {
        metadata.record(None);
        return;
    };
    let mut found_value = false;
    while !items.is_empty() {
        let Some((index, mut item)) = next_atom(&mut items) else {
            metadata.record(None);
            return;
        };
        if !indices.contains(&u32::from_be_bytes(index)) {
            continue;
        }
        while !item.is_empty() {
            let Some((kind, data)) = next_atom(&mut item) else {
                metadata.record(None);
                return;
            };
            if &kind == b"data" {
                found_value = true;
                metadata.record(parse_value(data));
            }
        }
    }
    if !found_value {
        metadata.record(None);
    }
}

fn color_gamma_indices(mut keys: &[u8]) -> Option<Vec<u32>> {
    if keys.len() < 8 || keys[..4] != [0; 4] {
        return None;
    }
    let count = u32::from_be_bytes(keys[4..8].try_into().ok()?);
    keys = &keys[8..];
    let mut indices = Vec::new();
    for index in 1..=count {
        if keys.len() < 8 {
            return None;
        }
        let size = u32::from_be_bytes(keys[..4].try_into().ok()?) as usize;
        if size < 8 || size > keys.len() {
            return None;
        }
        let key = keys[8..size].strip_suffix(&[0]).unwrap_or(&keys[8..size]);
        if &keys[4..8] == b"mdta" && key == COLOR_GAMMA_KEY {
            indices.push(index);
        }
        keys = &keys[size..];
    }
    keys.is_empty().then_some(indices)
}

fn parse_value(data: &[u8]) -> Option<Mode> {
    if data.len() < 8 {
        return None;
    }
    let text = match u32::from_be_bytes(data[..4].try_into().ok()?) {
        1 => std::str::from_utf8(&data[8..]).ok()?.to_owned(),
        2 => {
            let pairs = data[8..].chunks_exact(2);
            if !pairs.remainder().is_empty() {
                return None;
            }
            let units: Vec<_> = pairs
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&units).ok()?
        }
        _ => return None,
    };
    match text.trim_end_matches('\0') {
        "Normal" => Some(Mode::Normal),
        "D-Log" => Some(Mode::DLog),
        "D-Log M" => Some(Mode::DLogM),
        "D-Log 2" | "D-Log2" => Some(Mode::DLog2),
        "HLG" => Some(Mode::Hlg),
        _ => None,
    }
}

fn next_atom<'a>(bytes: &mut &'a [u8]) -> Option<([u8; 4], &'a [u8])> {
    if bytes.len() < 8 {
        return None;
    }
    let size = u32::from_be_bytes(bytes[..4].try_into().ok()?);
    let kind = bytes[4..8].try_into().ok()?;
    let (size, header_size) = match size {
        0 => (bytes.len(), 8),
        1 if bytes.len() >= 16 => (
            usize::try_from(u64::from_be_bytes(bytes[8..16].try_into().ok()?)).ok()?,
            16,
        ),
        size => (size as usize, 8),
    };
    if size < header_size || size > bytes.len() {
        return None;
    }
    let payload = &bytes[header_size..size];
    *bytes = &bytes[size..];
    Some((kind, payload))
}
