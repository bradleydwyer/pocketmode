use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

mod metadata;

const COLOR_GAMMA_KEY: &[u8] = b"com.dji.camera.ColorGammaSxS";
const D_LOG_2_SPACED: &[u8] = b"D-Log 2";
const D_LOG_2_COMPACT: &[u8] = b"D-Log2";
const D_LOG_M: &[u8] = b"D-Log M";
const D_LOG: &[u8] = b"D-Log";
const NORMAL: &[u8] = b"Normal";
const HLG: &[u8] = b"HLG";
const DE_GAMMA: &[u8] = b"dcc0_degamma1";
const YUV_GAMMA: &[u8] = b"yuvpp0_gamma_v1";

const PATTERNS: [&[u8]; 9] = [
    COLOR_GAMMA_KEY,
    D_LOG_2_SPACED,
    D_LOG_2_COMPACT,
    D_LOG_M,
    D_LOG,
    NORMAL,
    HLG,
    DE_GAMMA,
    YUV_GAMMA,
];

const SCAN_BUFFER_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    DLog,
    DLogM,
    DLog2,
    Hlg,
    Unknown,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::DLog => "D-Log",
            Self::DLogM => "D-Log M",
            Self::DLog2 => "D-Log 2",
            Self::Hlg => "HLG",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fingerprints {
    pub color_gamma_key: u64,
    pub d_log_2: u64,
    pub d_log_m: u64,
    pub d_log: u64,
    pub normal: u64,
    pub hlg: u64,
    pub de_gamma: u64,
    pub yuv_gamma: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub mode: Mode,
    pub confidence: Confidence,
    pub evidence: String,
    pub fingerprints: Fingerprints,
}

pub fn inspect_file(path: &Path) -> io::Result<Detection> {
    let file = File::open(path)?;
    inspect_reader(file)
}

pub fn inspect_reader<R: Read>(reader: R) -> io::Result<Detection> {
    inspect_reader_with_buffer(reader, SCAN_BUFFER_SIZE)
}

fn inspect_reader_with_buffer<R: Read>(reader: R, buffer_size: usize) -> io::Result<Detection> {
    let mut reader = FingerprintReader::new(reader);
    let metadata = metadata::inspect(&mut reader, buffer_size)?;
    let counts = reader.counts;
    let fingerprints = Fingerprints {
        color_gamma_key: counts[0],
        d_log_2: counts[1] + counts[2],
        d_log_m: counts[3],
        d_log: counts[4],
        normal: counts[5],
        hlg: counts[6],
        de_gamma: counts[7],
        yuv_gamma: counts[8],
    };

    Ok(classify(fingerprints, metadata))
}

fn classify(fingerprints: Fingerprints, metadata: metadata::DirectMetadata) -> Detection {
    let (mode, confidence, evidence) = if let Some(mode) = metadata.mode() {
        (
            mode,
            Confidence::High,
            format!("DJI ColorGammaSxS metadata declares {}", mode.as_str()),
        )
    } else if metadata.found {
        (
            Mode::Unknown,
            Confidence::Low,
            "DJI ColorGammaSxS value is missing, unsupported, or conflicting".to_owned(),
        )
    } else if fingerprints.de_gamma > 0 && fingerprints.yuv_gamma > 0 {
        (
            Mode::DLog2,
            Confidence::High,
            format!(
                "legacy DJI pipeline fingerprint: dcc0_degamma1={} and yuvpp0_gamma_v1={}",
                fingerprints.de_gamma, fingerprints.yuv_gamma
            ),
        )
    } else if fingerprints.yuv_gamma > 0 && fingerprints.de_gamma == 0 {
        (
            Mode::Normal,
            Confidence::Medium,
            format!(
                "legacy DJI pipeline fingerprint: yuvpp0_gamma_v1={} without dcc0_degamma1",
                fingerprints.yuv_gamma
            ),
        )
    } else {
        (
            Mode::Unknown,
            Confidence::Low,
            "no recognised DJI colour-profile metadata or pipeline fingerprint".to_owned(),
        )
    };

    Detection {
        mode,
        confidence,
        evidence,
        fingerprints,
    }
}

// Count the existing diagnostic fingerprints as the atom parser reads the stream.
struct FingerprintReader<R> {
    inner: R,
    counts: [u64; PATTERNS.len()],
    tail: Vec<u8>,
    window: Vec<u8>,
    patterns: Vec<SearchPattern>,
    overlap: usize,
}

impl<R> FingerprintReader<R> {
    fn new(inner: R) -> Self {
        let overlap = PATTERNS.iter().map(|pattern| pattern.len()).max().unwrap() - 1;
        Self {
            inner,
            counts: [0; PATTERNS.len()],
            tail: Vec::with_capacity(overlap),
            window: Vec::new(),
            patterns: PATTERNS.iter().map(|p| SearchPattern::new(p)).collect(),
            overlap,
        }
    }
}

impl<R: Read> Read for FingerprintReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let bytes_read = self.inner.read(buffer)?;
        let tail_len = self.tail.len();
        self.window.clear();
        self.window.extend_from_slice(&self.tail);
        self.window.extend_from_slice(&buffer[..bytes_read]);

        for (pattern_index, pattern) in self.patterns.iter().enumerate() {
            let mut offset = 0;
            while let Some(start) = pattern.find_from(&self.window, offset) {
                let end = start + pattern.needle.len();
                if end > tail_len {
                    self.counts[pattern_index] += 1;
                }
                offset = start + 1;
            }
        }

        let keep = self.window.len().min(self.overlap);
        self.tail.clear();
        self.tail
            .extend_from_slice(&self.window[self.window.len() - keep..]);
        Ok(bytes_read)
    }
}

struct SearchPattern {
    needle: &'static [u8],
    skip: [usize; 256],
}

impl SearchPattern {
    fn new(needle: &'static [u8]) -> Self {
        let mut skip = [needle.len(); 256];
        for (index, byte) in needle
            .iter()
            .take(needle.len().saturating_sub(1))
            .enumerate()
        {
            skip[*byte as usize] = needle.len() - 1 - index;
        }
        Self { needle, skip }
    }

    fn find_from(&self, haystack: &[u8], start: usize) -> Option<usize> {
        let pattern_len = self.needle.len();
        if pattern_len == 0 {
            return Some(start.min(haystack.len()));
        }
        if start > haystack.len().saturating_sub(pattern_len) {
            return None;
        }

        let last_index = pattern_len - 1;
        let mut offset = start;
        while offset + pattern_len <= haystack.len() {
            let last_byte = haystack[offset + last_index];
            if last_byte == self.needle[last_index]
                && haystack[offset..offset + last_index] == self.needle[..last_index]
            {
                return Some(offset);
            }
            offset += self.skip[last_byte as usize];
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn inspect(bytes: &[u8]) -> Detection {
        inspect_reader_with_buffer(Cursor::new(bytes), 7).unwrap()
    }

    fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn metadata_atom(entries: &[(&[u8], &[u8])], full_box: bool, items_first: bool) -> Vec<u8> {
        let mut keys = vec![0; 4];
        keys.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        let mut items = Vec::new();
        for (index, (key, value)) in entries.iter().enumerate() {
            keys.extend(atom(b"mdta", key));
            let mut data = 1_u32.to_be_bytes().to_vec(); // UTF-8, default locale
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(value);
            items.extend(atom(
                &((index + 1) as u32).to_be_bytes(),
                &atom(b"data", &data),
            ));
        }
        let keys = atom(b"keys", &keys);
        let items = atom(b"ilst", &items);
        let mut payload = if full_box { vec![0; 4] } else { Vec::new() };
        if items_first {
            payload.extend(items);
            payload.extend(keys);
        } else {
            payload.extend(keys);
            payload.extend(items);
        }
        atom(b"meta", &payload)
    }

    fn movie(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
        atom(b"moov", &metadata_atom(entries, true, false))
    }

    #[test]
    fn detects_direct_d_log_metadata_across_buffer_boundaries() {
        struct ShortReads<R>(R);
        impl<R: Read> Read for ShortReads<R> {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                let len = bytes.len().min(3);
                self.0.read(&mut bytes[..len])
            }
        }
        let bytes = movie(&[(COLOR_GAMMA_KEY, b"D-Log\0")]);

        let detection = inspect_reader(ShortReads(Cursor::new(bytes))).unwrap();

        assert_eq!(detection.mode, Mode::DLog);
        assert_eq!(detection.confidence, Confidence::High);
        assert_eq!(detection.fingerprints.color_gamma_key, 1);
    }

    #[test]
    fn detects_each_supported_metadata_value() {
        for (value, expected) in [
            ("Normal", Mode::Normal),
            ("D-Log", Mode::DLog),
            ("D-Log M", Mode::DLogM),
            ("D-Log 2", Mode::DLog2),
            ("D-Log2", Mode::DLog2),
            ("HLG", Mode::Hlg),
        ] {
            let detection = inspect(&movie(&[(COLOR_GAMMA_KEY, value.as_bytes())]));
            assert_eq!(detection.mode, expected, "{value}");
            assert_eq!(detection.confidence, Confidence::High, "{value}");
        }
    }

    #[test]
    fn ignores_unrelated_profile_names_in_other_metadata_and_video_data() {
        // The camera key is index 2; index 1 contains an unrelated profile name.
        let mut bytes = movie(&[
            (b"com.example.comment", b"D-Log 2"),
            (COLOR_GAMMA_KEY, b"Normal"),
        ]);
        bytes.extend(atom(b"mdat", b"D-Log M D-Log 2 HLG"));

        let detection = inspect(&bytes);

        assert_eq!(detection.mode, Mode::Normal);
        assert_eq!(detection.confidence, Confidence::High);
        assert_eq!(
            detection.evidence,
            "DJI ColorGammaSxS metadata declares Normal"
        );
        // Raw counts remain available for diagnostics, but do not select the mode.
        assert_eq!(detection.fingerprints.d_log_2, 2);
    }

    #[test]
    fn raw_key_and_mode_strings_do_not_count_as_metadata() {
        let bytes = [COLOR_GAMMA_KEY, b"\0Normal\0unrelated D-Log 2"].concat();

        assert_eq!(inspect(&bytes).mode, Mode::Unknown);
        assert_eq!(inspect(&atom(b"mdat", &bytes)).mode, Mode::Unknown);
    }

    #[test]
    fn supports_quicktime_and_iso_metadata_in_either_child_order() {
        for full_box in [false, true] {
            for items_first in [false, true] {
                let bytes = atom(
                    b"moov",
                    &atom(
                        b"udta",
                        &metadata_atom(&[(COLOR_GAMMA_KEY, b"D-Log M")], full_box, items_first),
                    ),
                );
                assert_eq!(inspect(&bytes).mode, Mode::DLogM);
            }
        }
    }

    #[test]
    fn requires_an_exact_supported_value() {
        for value in ["D-Log 20", "D-Log M extra", "Normal D-Log 2", "Custom"] {
            let mut bytes = movie(&[(COLOR_GAMMA_KEY, value.as_bytes())]);
            bytes.extend(atom(b"mdat", b"Normal dcc0_degamma1 yuvpp0_gamma_v1"));
            let detection = inspect(&bytes);

            assert_eq!(detection.mode, Mode::Unknown, "{value}");
            assert_eq!(detection.confidence, Confidence::Low, "{value}");
        }
    }

    #[test]
    fn does_not_resolve_a_key_using_another_metadata_scope() {
        let mut keys = vec![0; 4];
        keys.extend_from_slice(&1_u32.to_be_bytes());
        keys.extend(atom(b"mdta", COLOR_GAMMA_KEY));
        let mut payload = vec![0; 4];
        payload.extend(atom(b"keys", &keys));
        let mut bytes = atom(b"moov", &atom(b"meta", &payload));
        bytes.extend(movie(&[(b"com.example.comment", b"D-Log 2")]));

        assert_eq!(inspect(&bytes).mode, Mode::Unknown);
    }

    #[test]
    fn conflicting_direct_values_are_unknown() {
        let mut bytes = movie(&[(COLOR_GAMMA_KEY, b"Normal")]);
        bytes.extend(movie(&[(COLOR_GAMMA_KEY, b"D-Log 2")]));

        let detection = inspect(&bytes);
        assert_eq!(detection.mode, Mode::Unknown);
        assert_eq!(detection.confidence, Confidence::Low);
    }

    #[test]
    fn respects_the_metadata_data_type_and_text_encoding() {
        fn typed_movie(data_type: u32, value: &[u8]) -> Vec<u8> {
            let mut keys = vec![0; 4];
            keys.extend_from_slice(&1_u32.to_be_bytes());
            keys.extend(atom(b"mdta", COLOR_GAMMA_KEY));
            let mut data = data_type.to_be_bytes().to_vec();
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(value);
            let item = atom(&1_u32.to_be_bytes(), &atom(b"data", &data));
            let mut meta = vec![0; 4];
            meta.extend(atom(b"keys", &keys));
            meta.extend(atom(b"ilst", &item));
            atom(b"moov", &atom(b"meta", &meta))
        }

        assert_eq!(
            inspect(&typed_movie(2, b"\0N\0o\0r\0m\0a\0l")).mode,
            Mode::Normal
        );
        for (data_type, value) in [
            (21, b"Normal".as_slice()), // Integer data is not a profile declaration.
            (1, b"Normal\xff".as_slice()), // Invalid UTF-8.
            (2, b"\0N\0o\0r\0m\0a\0l\0".as_slice()), // Truncated UTF-16.
        ] {
            assert_eq!(inspect(&typed_movie(data_type, value)).mode, Mode::Unknown);
        }
    }

    #[test]
    fn propagates_input_errors() {
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("input failure"))
            }
        }

        assert!(inspect_reader(BrokenReader).is_err());
    }

    #[test]
    fn supports_extended_and_zero_sized_atoms() {
        let meta = metadata_atom(&[(COLOR_GAMMA_KEY, b"HLG")], true, false);
        let mut bytes = 1_u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"moov");
        bytes.extend_from_slice(&(meta.len() as u64 + 16).to_be_bytes());
        bytes.extend_from_slice(&meta);
        assert_eq!(inspect(&bytes).mode, Mode::Hlg);

        let mut bytes = atom(b"moov", &meta);
        bytes[..4].fill(0);
        assert_eq!(inspect(&bytes).mode, Mode::Hlg);
    }

    #[test]
    fn truncated_metadata_never_declares_a_mode() {
        let bytes = movie(&[(COLOR_GAMMA_KEY, b"D-Log 2")]);
        for length in 0..bytes.len() {
            assert_eq!(inspect(&bytes[..length]).mode, Mode::Unknown, "{length}");
        }
    }

    #[test]
    fn detects_legacy_d_log_2_pipeline() {
        let bytes = b"dbgi dcc0_degamma1 other yuvpp0_gamma_v1";
        let detection = inspect(bytes);

        assert_eq!(detection.mode, Mode::DLog2);
        assert_eq!(detection.confidence, Confidence::High);
    }

    #[test]
    fn detects_legacy_normal_pipeline_with_lower_confidence() {
        let bytes = b"dbgi yuvpp0_gamma_v1";
        let detection = inspect(bytes);

        assert_eq!(detection.mode, Mode::Normal);
        assert_eq!(detection.confidence, Confidence::Medium);
    }

    #[test]
    fn returns_unknown_without_a_known_fingerprint() {
        assert_eq!(inspect(b"ordinary mp4 bytes").mode, Mode::Unknown);
    }

    #[test]
    fn counts_repeated_patterns_once_each() {
        let bytes = b"dcc0_degamma1 dcc0_degamma1 yuvpp0_gamma_v1 yuvpp0_gamma_v1";
        let detection = inspect(bytes);

        assert_eq!(detection.fingerprints.de_gamma, 2);
        assert_eq!(detection.fingerprints.yuv_gamma, 2);
    }

    #[test]
    fn search_pattern_finds_offsets_and_absent_values() {
        let pattern = SearchPattern::new(b"needle");

        assert_eq!(pattern.find_from(b"a needle in a haystack", 0), Some(2));
        assert_eq!(pattern.find_from(b"a needle and needle", 3), Some(13));
        assert_eq!(pattern.find_from(b"nothing here", 0), None);
    }
}
