use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

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

fn inspect_reader_with_buffer<R: Read>(mut reader: R, buffer_size: usize) -> io::Result<Detection> {
    let counts = scan_patterns(&mut reader, buffer_size)?;
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

    Ok(classify(fingerprints))
}

fn classify(fingerprints: Fingerprints) -> Detection {
    let has_direct_metadata = fingerprints.color_gamma_key > 0;

    let (mode, confidence, evidence) = if has_direct_metadata && fingerprints.d_log_2 > 0 {
        (
            Mode::DLog2,
            Confidence::High,
            "DJI ColorGammaSxS metadata declares D-Log 2".to_owned(),
        )
    } else if has_direct_metadata && fingerprints.d_log_m > 0 {
        (
            Mode::DLogM,
            Confidence::High,
            "DJI ColorGammaSxS metadata declares D-Log M".to_owned(),
        )
    } else if has_direct_metadata && fingerprints.d_log > 0 {
        (
            Mode::DLog,
            Confidence::High,
            "DJI ColorGammaSxS metadata declares D-Log".to_owned(),
        )
    } else if has_direct_metadata && fingerprints.hlg > 0 {
        (
            Mode::Hlg,
            Confidence::High,
            "DJI ColorGammaSxS metadata declares HLG".to_owned(),
        )
    } else if has_direct_metadata && fingerprints.normal > 0 {
        (
            Mode::Normal,
            Confidence::High,
            "DJI ColorGammaSxS metadata declares Normal".to_owned(),
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
    } else if has_direct_metadata {
        (
            Mode::Unknown,
            Confidence::Low,
            "DJI ColorGammaSxS key found, but its value is not recognised".to_owned(),
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

fn scan_patterns<R: Read>(reader: &mut R, buffer_size: usize) -> io::Result<[u64; PATTERNS.len()]> {
    let max_pattern_len = PATTERNS
        .iter()
        .map(|pattern| pattern.len())
        .max()
        .unwrap_or(1);
    let mut counts = [0_u64; PATTERNS.len()];
    let mut read_buffer = vec![0_u8; buffer_size.max(1)];
    let mut tail = Vec::with_capacity(max_pattern_len.saturating_sub(1));
    let search_patterns: Vec<_> = PATTERNS
        .iter()
        .map(|pattern| SearchPattern::new(pattern))
        .collect();

    loop {
        let bytes_read = reader.read(&mut read_buffer)?;
        if bytes_read == 0 {
            break;
        }

        let tail_len = tail.len();
        let mut window = Vec::with_capacity(tail_len + bytes_read);
        window.extend_from_slice(&tail);
        window.extend_from_slice(&read_buffer[..bytes_read]);

        for (pattern_index, pattern) in search_patterns.iter().enumerate() {
            let mut offset = 0;
            while let Some(start) = pattern.find_from(&window, offset) {
                let end = start + pattern.needle.len();
                if end > tail_len {
                    counts[pattern_index] += 1;
                }
                offset = start + 1;
            }
        }

        let keep = window.len().min(max_pattern_len.saturating_sub(1));
        tail.clear();
        tail.extend_from_slice(&window[window.len() - keep..]);
    }

    Ok(counts)
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

    #[test]
    fn detects_direct_d_log_metadata_across_buffer_boundaries() {
        let bytes = [
            b"prefix".as_slice(),
            COLOR_GAMMA_KEY,
            b"\0\0D-Log\0suffix".as_slice(),
        ]
        .concat();

        let detection = inspect(&bytes);

        assert_eq!(detection.mode, Mode::DLog);
        assert_eq!(detection.confidence, Confidence::High);
        assert_eq!(detection.fingerprints.color_gamma_key, 1);
    }

    #[test]
    fn prefers_d_log_2_over_the_d_log_substring() {
        let bytes = [COLOR_GAMMA_KEY, b"\0D-Log 2"].concat();

        assert_eq!(inspect(&bytes).mode, Mode::DLog2);
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
