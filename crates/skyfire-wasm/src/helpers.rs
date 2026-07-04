/// Convert a 3-byte ISO 639-2 language code to a `String`.
pub fn lang_bytes_to_string(lang: &[u8; 3]) -> String {
    String::from_utf8_lossy(lang).into_owned()
}

/// Parse the sample_count from a trun box in a media segment.
///
/// `trun` is nested inside `moof`→`traf`→`trun`, so we scan all byte offsets
/// rather than walking only top-level boxes.
pub fn parse_sample_count_from_segment(bytes: &[u8]) -> u32 {
    // Scan for the 4-byte box-type b"trun" at any offset.
    // Layout when scanning by TYPE field offset (i = offset of "trun" bytes):
    //   +0..+3  type = b"trun"
    //   +4      version (1 byte)
    //   +5..+7  flags (3 bytes)
    //   +8..+11 sample_count (4 bytes)
    // So sample_count sits at bytes[i+8..i+11] where i is the type-field offset.
    let mut total = 0u32;
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4] == *b"trun" {
            // i is where the type field is; box start is i-4
            if i >= 4 && i + 12 <= bytes.len() {
                let sc =
                    u32::from_be_bytes([bytes[i + 8], bytes[i + 9], bytes[i + 10], bytes[i + 11]]);
                total += sc;
            }
        }
        i += 1;
    }
    total
}

/// Parse base_media_decode_time from a tfdt box in a media segment.
///
/// `tfdt` is nested inside `moof`→`traf`→`tfdt`, so we scan all byte offsets.
/// Layout: size(4) + type(4) + version(1) + flags(3) + decode_time(4 or 8)
pub fn parse_base_media_decode_time(bytes: &[u8]) -> u64 {
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4] == *b"tfdt" {
            // i is the type field offset; version is at i+4, decode_time at i+8
            if i + 8 <= bytes.len() {
                let version = bytes[i + 4];
                if version == 1 && i + 16 <= bytes.len() {
                    return u64::from_be_bytes([
                        bytes[i + 8],
                        bytes[i + 9],
                        bytes[i + 10],
                        bytes[i + 11],
                        bytes[i + 12],
                        bytes[i + 13],
                        bytes[i + 14],
                        bytes[i + 15],
                    ]);
                } else if i + 12 <= bytes.len() {
                    return u32::from_be_bytes([
                        bytes[i + 8],
                        bytes[i + 9],
                        bytes[i + 10],
                        bytes[i + 11],
                    ]) as u64;
                }
            }
        }
        i += 1;
    }
    0
}
