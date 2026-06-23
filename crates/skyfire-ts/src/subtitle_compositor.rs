//! DVB subtitle compositor — ETSI EN 300 743 display-set to RGBA region bitmaps.
//!
//! Maintains compositor state across PES packets for a selected subtitle PID.
//! Accumulates segments per display set (from page-composition through
//! end-of-display-set), then composites CLUT palette + object pixel data
//! into per-region RGBA buffers placed on the display page.

use dvb_common::Serialize;
use dvb_subtitle::{AnySegment, PesDataField};
use dvb_subtitle::{
    ClutDefinitionSegment, DataType, ObjectCodingMethod, ObjectDataPayload, ObjectDataSegment,
    PageCompositionSegment, PixelDataSubBlock, RegionCompositionSegment,
};

// ---------------------------------------------------------------------------
// RGBA colour conversion
// ---------------------------------------------------------------------------

/// Convert a single Y/Cr/Cb/T (CLUT entry) to RGBA via BT.601.
#[inline]
fn ycrcb_to_rgba(y: u8, cr: u8, cb: u8, t: u8) -> [u8; 4] {
    // BT.601 full-range (0-255)
    // Convert u8 to i16 for chroma to handle negative offsets (center=128)
    let y_f = f64::from(y);
    let cr_f = f64::from(cr as i16 - 128);
    let cb_f = f64::from(cb as i16 - 128);
    let r = (y_f + 1.402_00 * cr_f).clamp(0.0, 255.0) as u8;
    let g = (y_f - 0.344_14 * cb_f - 0.714_14 * cr_f).clamp(0.0, 255.0) as u8;
    let b = (y_f + 1.772_00 * cb_f).clamp(0.0, 255.0) as u8;
    [r, g, b, t]
}

// ---------------------------------------------------------------------------
// RLE pixel-code expansion (EN 300 743 Tables 20, 22-26)
// ---------------------------------------------------------------------------

/// Expand a 2-bit/pixel code string to per-pixel CLUT indices.
///
/// EN 300 743 §7.2.5.5, Tables 22-23.
#[allow(clippy::while_let_loop)]
fn expand_2bit(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    if data.is_empty() {
        return out;
    }
    let mut bitpos = 0usize;

    // read up to n bits (max 8)
    let read_bits = |data: &[u8], bitpos: &mut usize, n: usize| -> Option<u8> {
        if *bitpos + n > data.len() * 8 {
            return None;
        }
        let mut v = 0u8;
        for _ in 0..n {
            let byte = data[*bitpos / 8];
            let bit = 7 - (*bitpos % 8);
            *bitpos += 1;
            v = (v << 1) | ((byte >> bit) & 1);
        }
        Some(v)
    };

    loop {
        let b2 = match read_bits(data, &mut bitpos, 2) {
            Some(b) => b,
            None => break,
        };
        if b2 != 0 {
            out.push(b2);
            continue;
        }
        // 2-bit zero — check switch_1
        let s1 = match read_bits(data, &mut bitpos, 1) {
            Some(b) => b,
            None => break,
        };
        if s1 == 1 {
            // run_length_3-10 (3 bits) + 2-bit pixel-code
            let rl = match read_bits(data, &mut bitpos, 3) {
                Some(b) => b as usize + 3,
                None => break,
            };
            let px = match read_bits(data, &mut bitpos, 2) {
                Some(b) => b,
                None => break,
            };
            for _ in 0..rl {
                out.push(px);
            }
            continue;
        }
        // switch_1 == 0 — check switch_2
        let s2 = match read_bits(data, &mut bitpos, 1) {
            Some(b) => b,
            None => break,
        };
        if s2 == 1 {
            // 1 pixel in colour 0
            out.push(0);
            continue;
        }
        // switch_2 == 0 — check switch_3 (2 bits)
        let s3 = match read_bits(data, &mut bitpos, 2) {
            Some(b) => b,
            None => break,
        };
        match s3 {
            0b00 => break, // end of string
            0b01 => {
                // 2 pixels in colour 0
                out.push(0);
                out.push(0);
            }
            0b10 => {
                let rl = match read_bits(data, &mut bitpos, 4) {
                    Some(b) => b as usize + 12,
                    None => break,
                };
                let px = match read_bits(data, &mut bitpos, 2) {
                    Some(b) => b,
                    None => break,
                };
                for _ in 0..rl {
                    out.push(px);
                }
            }
            0b11 => {
                let rl = match read_bits(data, &mut bitpos, 8) {
                    Some(b) => b as usize + 29,
                    None => break,
                };
                let px = match read_bits(data, &mut bitpos, 2) {
                    Some(b) => b,
                    None => break,
                };
                for _ in 0..rl {
                    out.push(px);
                }
            }
            _ => unreachable!(),
        }
    }
    out
}

/// Expand a 4-bit/pixel code string to per-pixel CLUT indices.
///
/// EN 300 743 §7.2.5.5, Tables 24-25.
#[allow(clippy::while_let_loop)]
fn expand_4bit(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    if data.is_empty() {
        return out;
    }
    let mut bitpos = 0usize;

    let read_bits = |data: &[u8], bitpos: &mut usize, n: usize| -> Option<u8> {
        if *bitpos + n > data.len() * 8 {
            return None;
        }
        let mut v = 0u8;
        for _ in 0..n {
            let byte = data[*bitpos / 8];
            let bit = 7 - (*bitpos % 8);
            *bitpos += 1;
            v = (v << 1) | ((byte >> bit) & 1);
        }
        Some(v)
    };

    loop {
        let b4 = match read_bits(data, &mut bitpos, 4) {
            Some(b) => b,
            None => break,
        };
        if b4 != 0 {
            out.push(b4);
            continue;
        }
        // 4-bit zero — check switch_1
        let s1 = match read_bits(data, &mut bitpos, 1) {
            Some(b) => b,
            None => break,
        };
        if s1 == 0 {
            let n3 = match read_bits(data, &mut bitpos, 3) {
                Some(b) => b,
                None => break,
            };
            if n3 == 0 {
                break; // end_of_string
            }
            // run_length_3-9 in colour 0
            out.extend(std::iter::repeat_n(0u8, n3 as usize));
            continue;
        }
        // s1 == 1 — check switch_2
        let s2 = match read_bits(data, &mut bitpos, 1) {
            Some(b) => b,
            None => break,
        };
        if s2 == 0 {
            // run_length_4-7 + 4-bit pixel-code
            let rl = match read_bits(data, &mut bitpos, 2) {
                Some(b) => b as usize + 4,
                None => break,
            };
            let px = match read_bits(data, &mut bitpos, 4) {
                Some(b) => b,
                None => break,
            };
            for _ in 0..rl {
                out.push(px);
            }
            continue;
        }
        // switch_2 == 1 — check switch_3 (2 bits)
        let s3 = match read_bits(data, &mut bitpos, 2) {
            Some(b) => b,
            None => break,
        };
        match s3 {
            0b00 | 0b01 => {
                let count = s3 as usize + 1;
                out.extend(std::iter::repeat_n(0u8, count));
            }
            0b10 => {
                let rl = match read_bits(data, &mut bitpos, 4) {
                    Some(b) => b as usize + 9,
                    None => break,
                };
                let px = match read_bits(data, &mut bitpos, 4) {
                    Some(b) => b,
                    None => break,
                };
                for _ in 0..rl {
                    out.push(px);
                }
            }
            0b11 => {
                let rl = match read_bits(data, &mut bitpos, 8) {
                    Some(b) => b as usize + 25,
                    None => break,
                };
                let px = match read_bits(data, &mut bitpos, 4) {
                    Some(b) => b,
                    None => break,
                };
                for _ in 0..rl {
                    out.push(px);
                }
            }
            _ => unreachable!(),
        }
    }
    out
}

/// Expand an 8-bit/pixel code string to per-pixel CLUT indices.
///
/// EN 300 743 §7.2.5.5, Table 26.
fn expand_8bit(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    if data.is_empty() {
        return out;
    }
    let mut pos = 0usize;
    while pos < data.len() {
        let b = data[pos];
        if b != 0x00 {
            out.push(b);
            pos += 1;
            continue;
        }
        // zero byte — check next byte for run-length or end signal
        if pos + 1 >= data.len() {
            out.push(b);
            break;
        }
        let next = data[pos + 1];
        let s1 = (next >> 7) & 1;
        if s1 == 0 {
            let rl = (next & 0x7F) as usize;
            if rl == 0 {
                // end_of_string_signal — two bytes consumed (0x00 0x00)
                break;
            }
            // run_length_1-127 in colour 0
            out.extend(std::iter::repeat_n(0u8, rl));
            pos += 2;
        } else {
            // run_length_3-127 + 8-bit pixel-code
            let rl = (next & 0x7F) as usize + 2;
            if pos + 2 < data.len() {
                let px = data[pos + 2];
                for _ in 0..rl {
                    out.push(px);
                }
                pos += 3;
            } else {
                out.push(b);
            }
        }
    }
    out
}

/// Expand a pixel sub-block to per-pixel CLUT indices.
fn expand_sub_block(block: &PixelDataSubBlock) -> Vec<u8> {
    match block.data_type {
        DataType::CodeString2Bit => expand_2bit(block.data),
        DataType::CodeString4Bit => expand_4bit(block.data),
        DataType::CodeString8Bit => expand_8bit(block.data),
        _ => Vec::new(),
    }
}

/// Expand all sub-blocks for one field (top or bottom) into row-major pixels.
///
/// EndOfLine markers separate rows; we return the flat pixel buffer and
/// derive the row width from the first row.
fn expand_field(sub_blocks: &[PixelDataSubBlock]) -> (Vec<u8>, usize) {
    let mut all_pixels: Vec<u8> = Vec::new();
    let mut row_pixels: Vec<u8> = Vec::new();
    let mut row_width: Option<usize> = None;

    for block in sub_blocks {
        if block.data_type == DataType::EndOfLine {
            let rl = row_pixels.len();
            if row_width.is_none() && rl > 0 {
                row_width = Some(rl);
            }
            all_pixels.extend_from_slice(&row_pixels);
            row_pixels.clear();
        } else {
            let expanded = expand_sub_block(block);
            row_pixels.extend_from_slice(&expanded);
        }
    }
    // Flush any remaining pixels as a row
    if !row_pixels.is_empty() {
        let rl = row_pixels.len();
        if row_width.is_none() && rl > 0 {
            row_width = Some(rl);
        }
        all_pixels.extend_from_slice(&row_pixels);
    }

    let width = row_width.unwrap_or(all_pixels.len());
    (all_pixels, width)
}

/// Expand an object's pixel data into a (width, height, flat pixel-index buffer).
///
/// For interlaced coding: top field → even output lines, bottom → odd lines.
/// For progressive: zlib-decompress then expand as 8-bit code strings.
fn expand_object(obj: &ObjectDataSegment) -> Option<(usize, usize, Vec<u8>)> {
    match &obj.payload {
        ObjectDataPayload::InterlacedPixels(pixels) => {
            let (top_pixels, top_width) = expand_field(&pixels.top_sub_blocks);
            let (bottom_pixels, bottom_width) = expand_field(&pixels.bottom_sub_blocks);

            if top_pixels.is_empty() && bottom_pixels.is_empty() {
                return None;
            }
            let width = top_width.max(bottom_width);
            if width == 0 {
                return None;
            }
            let top_rows = if top_width > 0 {
                top_pixels.len() / width
            } else {
                0
            };
            let bottom_rows = if bottom_width > 0 {
                bottom_pixels.len() / width
            } else {
                0
            };
            let field_rows = top_rows.max(bottom_rows);
            let height = field_rows * 2;
            if height == 0 {
                return None;
            }

            let mut frame = vec![0u8; width * height];
            // Interleave top field (even lines) and bottom field (odd lines)
            for row in 0..top_rows {
                let src_off = row * width;
                if src_off + width > top_pixels.len() {
                    break;
                }
                let dst_off = row * 2 * width;
                frame[dst_off..dst_off + width]
                    .copy_from_slice(&top_pixels[src_off..src_off + width]);
            }
            for row in 0..bottom_rows {
                let src_off = row * width;
                if src_off + width > bottom_pixels.len() {
                    break;
                }
                let dst_off = (row * 2 + 1) * width;
                frame[dst_off..dst_off + width]
                    .copy_from_slice(&bottom_pixels[src_off..src_off + width]);
            }
            Some((width, height, frame))
        }
        ObjectDataPayload::ProgressivePixels(prog) => {
            let w = prog.bitmap_width as usize;
            let h = prog.bitmap_height as usize;
            if w == 0 || h == 0 {
                return None;
            }
            // Zlib-decompress via flate2
            use std::io::Read;
            let mut decompressed: Vec<u8> = Vec::new();
            let mut decoder = flate2::read::ZlibDecoder::new(prog.compressed_data);
            if decoder.read_to_end(&mut decompressed).is_err() {
                return None;
            }
            let pixels = expand_8bit(&decompressed);
            if pixels.is_empty() {
                return None;
            }
            Some((w, h, pixels))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// CLUT palette building
// ---------------------------------------------------------------------------

fn build_palette(clut: &ClutDefinitionSegment) -> Vec<[u8; 4]> {
    let mut palette = vec![[0u8; 4]; 256];
    for entry in &clut.entries {
        let idx = entry.clut_entry_id as usize;
        if idx < 256 {
            palette[idx] =
                ycrcb_to_rgba(entry.y_value, entry.cr_value, entry.cb_value, entry.t_value);
        }
    }
    palette
}

// ---------------------------------------------------------------------------
// Compositor state
// ---------------------------------------------------------------------------

/// Accumulated state for one display-set on a single subtitle PID.
#[derive(Debug, Default)]
pub struct CompositorState {
    /// Active CLUT definition (owned bytes for lifetime independence).
    clut_bytes: Option<Vec<u8>>,
    /// Region definitions, keyed by region_id (owned bytes).
    region_bytes: std::collections::HashMap<u8, Vec<u8>>,
    /// Object pixel data, keyed by object_id (owned bytes).
    object_bytes: std::collections::HashMap<u16, Vec<u8>>,
    /// Current page composition (owned bytes).
    page_bytes: Option<Vec<u8>>,
    /// Pending composited cues.
    pending_cues: Vec<CompositedCue>,
}

/// A fully composited subtitle cue — RGBA regions with screen placement.
#[derive(Debug)]
pub struct CompositedCue {
    pub start_pts: u64,
    pub end_pts: u64,
    pub pid: u16,
    pub regions: Vec<CompositedRegion>,
}

/// A single composited RGBA region, placed on screen.
#[derive(Debug)]
pub struct CompositedRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

impl CompositorState {
    /// Create a new empty compositor state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all state (for PID change or `select_subtitle(None)`).
    pub fn clear(&mut self) {
        self.clut_bytes = None;
        self.region_bytes.clear();
        self.object_bytes.clear();
        self.page_bytes = None;
        self.pending_cues.clear();
    }

    /// Feed a parsed PES data field for the given PID and PTS.
    ///
    /// Segments are accumulated until `EndOfDisplaySet` is seen, which
    /// triggers compositing and produces a [`CompositedCue`].
    pub fn feed_pes(&mut self, pid: u16, pts_ticks: Option<u64>, field: &PesDataField) {
        let start_pts = pts_ticks.unwrap_or(0);
        let mut page_time_out: u8 = 0;
        let mut has_end = false;

        for seg in &field.segments {
            match seg {
                AnySegment::PageComposition(ref pcs) => {
                    page_time_out = pcs.page_time_out;
                    self.page_bytes = Some(pcs.to_bytes());
                }
                AnySegment::RegionComposition(ref rcs) => {
                    self.region_bytes.insert(rcs.region_id, rcs.to_bytes());
                }
                AnySegment::ClutDefinition(ref clut) => {
                    self.clut_bytes = Some(clut.to_bytes());
                }
                AnySegment::ObjectData(ref obj) => {
                    self.object_bytes.insert(obj.object_id, obj.to_bytes());
                }
                AnySegment::EndOfDisplaySet(_) => {
                    has_end = true;
                }
                _ => {}
            }
        }

        if has_end {
            if let Some(cue) = self.composite(pid, start_pts, page_time_out) {
                self.pending_cues.push(cue);
            }
            // Clear display-set-local state (keep CLUT across display sets per spec)
            self.region_bytes.clear();
            self.object_bytes.clear();
            self.page_bytes = None;
        }
    }

    /// Take all pending composited cues since last call.
    pub fn take_cues(&mut self) -> Vec<CompositedCue> {
        std::mem::take(&mut self.pending_cues)
    }

    /// Composite current state into a cue.
    fn composite(&self, pid: u16, start_pts: u64, page_time_out: u8) -> Option<CompositedCue> {
        use dvb_common::Parse;

        let clut = self
            .clut_bytes
            .as_ref()
            .and_then(|b| ClutDefinitionSegment::parse(b).ok())?;
        let page = self
            .page_bytes
            .as_ref()
            .and_then(|b| PageCompositionSegment::parse(b).ok())?;
        if page.regions.is_empty() {
            return None;
        }

        let palette = build_palette(&clut);
        let mut regions_out: Vec<CompositedRegion> = Vec::new();

        for page_region in &page.regions {
            let region_bytes = self.region_bytes.get(&page_region.region_id)?;
            let region_def = RegionCompositionSegment::parse(region_bytes).ok()?;
            let width = region_def.region_width as usize;
            let height = region_def.region_height as usize;
            if width == 0 || height == 0 {
                continue;
            }

            let mut rgba = vec![0u8; width * height * 4];

            for obj_entry in &region_def.objects {
                let obj_bytes = self.object_bytes.get(&obj_entry.object_id)?;
                let obj = ObjectDataSegment::parse(obj_bytes).ok()?;
                if matches!(
                    obj.object_coding_method,
                    ObjectCodingMethod::Characters | ObjectCodingMethod::Reserved(_)
                ) {
                    continue;
                }

                let exp = expand_object(&obj);
                let (obj_w, obj_h, pixels) = exp?;
                let ox = obj_entry.object_horizontal_position as usize;
                let oy = obj_entry.object_vertical_position as usize;

                for sy in 0..obj_h {
                    let region_y = oy + sy;
                    if region_y >= height {
                        break;
                    }
                    for sx in 0..obj_w {
                        let region_x = ox + sx;
                        if region_x >= width {
                            break;
                        }
                        let src_idx = sy * obj_w + sx;
                        if src_idx >= pixels.len() {
                            break;
                        }
                        let clut_idx = pixels[src_idx] as usize;
                        if let Some(&[r, g, b, a]) = palette.get(clut_idx) {
                            if a > 0 {
                                let dst_idx = (region_y * width + region_x) * 4;
                                rgba[dst_idx..dst_idx + 4].copy_from_slice(&[r, g, b, a]);
                            }
                        }
                    }
                }
            }

            regions_out.push(CompositedRegion {
                x: page_region.region_horizontal_address,
                y: page_region.region_vertical_address,
                width: width as u16,
                height: height as u16,
                rgba,
            });
        }

        if regions_out.is_empty() {
            return None;
        }

        let end_pts = if page_time_out > 0 {
            start_pts.saturating_add(u64::from(page_time_out) * 90_000)
        } else {
            start_pts
        };

        Some(CompositedCue {
            start_pts,
            end_pts,
            pid,
            regions: regions_out,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dvb_common::Parse;

    /// Build a minimal display set PES data field (raw bytes).
    ///
    /// Contains:
    /// - Display definition (720x288)
    /// - CLUT: index 1 = opaque red (Y=81, Cr=90, Cb=240 gives R~G=0,B=0 via BT.601)
    /// - Region composition: 32x16, 8-bit, CLUT_id=1, places object 1 at (0,0)
    /// - Object data: id=1, interlaced, all pixels = index 1
    /// - Page composition: region 1 at screen (10,20), page_time_out=5
    /// - End of display set
    fn build_minimal_display_pes() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x20, 0x00]); // data_identifier + subtitle_stream_id

        // DDS header + body (seg_len=5): version=1, no window, 720x288
        buf.extend_from_slice(&[
            0x0F, 0x14, 0x00, 0x01, 0x00, 0x05, // header page_id=1, seg_len=5
            0x10, 0x02, 0xCF, 0x01, 0x1F, // version=1, width=719, height=287
        ]);

        // CLUT header + body (seg_len 2+6=8 for 1 full-range entry)
        // page_id=1, clut_id=1, version=1, reserved=0
        // Entry: id=1, flags=0x20 (8-bit) | 0x01 (full_range) = 0x21
        // Y=76, Cr=255, Cb=86, T=255 -> R=254, G=-0, B=2 via BT.601 (near-red)
        buf.extend_from_slice(&[
            0x0F, 0x12, 0x00, 0x01, 0x00, 0x08, // header page_id=1, seg_len=8
            0x01, 0x10, // clut_id=1, version=1, reserved=0
            0x01, 0x21, // entry_id=1, flags=0x21 (8-bit + full_range)
            0x4C, 0xFF, 0x56, 0xFF, // Y=76, Cr=255, Cb=86, T=255
        ]);

        // Region composition header + body (seg_len=10 + 6 for 1 object entry = 16)
        // body: region_id=1, version=1, fill=0, reserved=0
        //       width=32, height=16
        //       compat=8bit(3), depth=8bit(3) → byte=0xEC, reserved=0
        //       clut_id=1, 8bit_pixel=0, 4bit_pixel=0, 2bit_pixel=0, reserved=0
        // object: id=1, basic_bitmap(0), in_stream(0), hpos=0, vpos=0
        buf.extend_from_slice(&[
            0x0F, 0x11, 0x00, 0x01, 0x00, 0x10, // header page_id=1, seg_len=16
            0x01, 0x10, // region_id=1, version=1, fill=0
            0x00, 0x20, // width=32
            0x00, 0x10, // height=16
            0xEC, // compat=3<<5|depth=3<<2|reserved=0
            0x01, // clut_id=1
            0x00, // 8bit_pixel=0
            0x00, // 4bit=0, 2bit=0, reserved=0
            // object entry: id=1, type=0, provider=0, hpos=0, vpos=0 (no extra)
            0x00, 0x01, // object_id=1
            0x00, 0x00, // type=0|provider=0|hpos_hi=0, hpos_lo=0
            0x00, 0x00, // vpos_hi=0|reserved=0, vpos_lo=0
        ]);

        // Object data: id=1, interlaced, all pixels = 0x01
        // Fixed: object_id=1 (2), version=0|coding=pixels (1) = 3 bytes
        // Then 2 bytes top_len + 2 bytes bottom_len
        // Top field: 8 lines of 32 bytes 0x01 + 0xF0 end-of-line = 8*33 = 264 bytes
        // Bottom field: same = 264 bytes
        // Build interlaced object data as PixelDataSubBlock
        // Each top-field line: data_type=0x12 + 32 pixels of 0x01 + end-of-string(0x00 0x00) + end-of-line(0xF0)
        let mut top_field = Vec::new();
        for _ in 0..8 {
            top_field.push(0x12); // CodeString8Bit data_type
            top_field.extend_from_slice(&[0x01u8; 32]); // 32 pixels of CLUT index 1
            top_field.extend_from_slice(&[0x00, 0x00]); // end of string
            top_field.push(0xF0); // end of line
        }
        let mut bottom_field = Vec::new();
        for _ in 0..8 {
            bottom_field.push(0x12); // CodeString8Bit data_type
            bottom_field.extend_from_slice(&[0x01u8; 32]); // 32 pixels of CLUT index 1
            bottom_field.extend_from_slice(&[0x00, 0x00]); // end of string
            bottom_field.push(0xF0); // end of line
        }

        let mut obj_payload = Vec::new();
        obj_payload.extend_from_slice(&[0x00, 0x01, 0x00]); // object_id=1, version=0, coding=pixels
        obj_payload.extend_from_slice(&(top_field.len() as u16).to_be_bytes());
        obj_payload.extend_from_slice(&(bottom_field.len() as u16).to_be_bytes());
        obj_payload.extend_from_slice(&top_field);
        obj_payload.extend_from_slice(&bottom_field);

        let seg_len = obj_payload.len() as u16;
        buf.push(0x0F);
        buf.push(0x13); // object_data
        buf.extend_from_slice(&[0x00, 0x01]); // page_id=1
        buf.extend_from_slice(&seg_len.to_be_bytes());
        buf.extend_from_slice(&obj_payload);

        // Page composition: page_id=1, page_time_out=5, acquisition, region 1 at (10,20)
        // body: page_time_out(1) + flags(1) → [0x05, 0x1C] = time_out=5, version=1, acquisition
        // region entry: id=1, reserved=0, h_addr=10, v_addr=20
        buf.extend_from_slice(&[
            0x0F, 0x10, 0x00, 0x01, 0x00, 0x08, // header page_id=1, seg_len=8
            0x05, 0x14, // page_time_out=5, version=1|acquisition
            0x01, 0x00, // region_id=1, reserved=0
            0x00, 0x0A, // h_addr=10
            0x00, 0x14, // v_addr=20
        ]);

        // End of display set: page_id=1, seg_len=0
        buf.extend_from_slice(&[0x0F, 0x80, 0x00, 0x01, 0x00, 0x00]);
        buf.push(0xFF); // end marker
        buf
    }

    #[test]
    fn composite_red_region() {
        let pid = 0x42;
        let start_pts = 900_000u64;

        let pes_bytes = build_minimal_display_pes();
        let field = PesDataField::parse(&pes_bytes).expect("must parse valid PES data field");

        let mut compositor = CompositorState::new();
        compositor.feed_pes(pid, Some(start_pts), &field);
        let cues = compositor.take_cues();

        assert_eq!(cues.len(), 1, "must produce one composited cue");
        let cue = &cues[0];
        assert_eq!(cue.pid, pid);
        assert_eq!(cue.start_pts, start_pts);
        assert_eq!(cue.end_pts, start_pts + 5 * 90_000);

        assert_eq!(cue.regions.len(), 1, "must have one region");
        let region = &cue.regions[0];
        assert_eq!(region.x, 10, "region screen x");
        assert_eq!(region.y, 20, "region screen y");
        assert_eq!(region.width, 32, "region width");
        assert_eq!(region.height, 16, "region height");
        assert_eq!(region.rgba.len(), 32 * 16 * 4, "RGBA buffer size");

        // Centre pixel must be opaque red
        let mid = (8 * 32 + 16) * 4;
        assert_eq!(
            &region.rgba[mid..mid + 4],
            &[254u8, 0, 1, 255],
            "centre pixel must be opaque red (BT.601 approx)"
        );

        // Top-left pixel must be opaque red
        assert_eq!(
            &region.rgba[0..4],
            &[254u8, 0, 1, 255],
            "top-left pixel must be opaque red (BT.601 approx)"
        );

        // Count red pixels — all 32x16 should be red
        let red_count = region
            .rgba
            .chunks_exact(4)
            .filter(|px| px[0] == 254 && px[1] == 0 && px[2] == 1 && px[3] == 255)
            .count();
        assert_eq!(red_count, 32 * 16, "all pixels must be red");
    }
}
