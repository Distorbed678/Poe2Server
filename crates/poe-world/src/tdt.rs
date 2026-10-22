//! Tile definitions (`.tdt`, binary, versions 14/15): a tile's geometry (`.tgt`), its size in
//! tiles, the edge type on each side and the ground type at each corner, which is what the
//! generator matches when it places tiles. Layout after `third_party/reference/poeformats/Tdt.cs`:
//!
//! `u32 version, u32 string-table chars, UTF-16 strings (NUL-separated)`, then string
//! references (`i32` char offsets into the table, -1 = none): `inherits`; when empty: `tgt`,
//! `feature`, edge types down/right/up/left, `u8 size x, u8 size y`, ground types
//! down-left/down-right/up-right/up-left, 4 bytes, 8 edge-distance bytes. The rest (sub-tile
//! data, doodads, entities) is kept raw for now.
//!
//! A tile that inherits another (`*_Reorient_N.tdt`) has a `u8` of override flags after the
//! `inherits` reference (`0x1429153f0`) and carries only the overridden parts: 1 `tgt`, 8 the
//! header from `feature` on and the corner points, 0x10 the size and sub-tile geometry, 0x20 the
//! orientation mask. [`Tdt::resolve`] applies them to the parent.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tdt {
    pub version: u32,
    /// Another `.tdt` this one is based on (then the fields below are not present).
    pub inherits: Option<String>,
    pub tgt: Option<String>,
    pub feature: Option<String>,
    /// Edge types (`.et`): down, right, up, left.
    pub edges: [Option<String>; 4],
    /// Size in tiles.
    pub size: (u8, u8),
    /// Ground types (`.gt`): down-left, down-right, up-right, up-left.
    pub grounds: [Option<String>; 4],
    pub unk: [u8; 4],
    pub edge_distances: [u8; 8],
    /// Everything after the header.
    pub rest: Vec<u8>,
    /// Ground type per corner point, `(w + 1) × (h + 1)` row-major in the tile's own
    /// orientation (data +0xa8; empty when the file has none).
    pub point_grounds: Vec<Option<String>>,
    /// Height per corner point (data +0xc0; empty when the file has none).
    pub point_heights: Vec<i8>,
    /// Orientations the tile may be used in (data +0x171, bit per orientation; 0 or 0xff:
    /// all), intersected with the `.tst` rotation list by the tile-set index (`0x141d09ab0`).
    pub orientation_mask: u8,
    /// Override flags of an inheriting tile (see the module docs).
    pub inherit_flags: u8,
    /// Sub-tiles (row-major in the tile's own orientation, `size.0 × size.1`) with their cell
    /// maps; empty when the file has none.
    pub subtiles: Vec<SubTile>,
    /// Hull plane masks (data +0x173, `n` bytes; `[0]` for a single plane): plane `i` serves the
    /// orientations in its mask bits, 0 meaning all.
    pub hull_masks: Vec<u8>,
}

/// One sub-tile's data (`0x1429153f0`): flag byte, the 23×23 byte map of cell codes and the
/// byte list that follows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubTile {
    pub flags: u8,
    /// The byte after the flags (present with the header; bit 0: the plane reaches 4 cells
    /// past the left edge).
    pub b1: u8,
    /// Header byte of the cell map (kind in bits 0..2) and the decoded map: `None` for a
    /// sub-tile without a map (flag 4), `Some(vec![v])` for a uniform map (kind 0, value `v`;
    /// empty for value 0), else 529 codes, row-major.
    pub map: Option<Vec<u8>>,
    pub list: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
#[error("tdt at byte {offset}: {what}")]
pub struct TdtError {
    pub offset: usize,
    pub what: &'static str,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    strings: Vec<u16>,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], TdtError> {
        let b = self.data.get(self.pos..self.pos + N).ok_or(TdtError {
            offset: self.pos,
            what: "truncated",
        })?;
        self.pos += N;
        Ok(b.try_into().unwrap())
    }

    fn u8(&mut self) -> Result<u8, TdtError> {
        Ok(self.bytes::<1>()?[0])
    }

    fn i32(&mut self) -> Result<i32, TdtError> {
        Ok(i32::from_le_bytes(self.bytes()?))
    }

    /// A string reference: `i32` char offset into the table (None when out of range).
    fn string(&mut self) -> Result<Option<String>, TdtError> {
        let i = self.i32()?;
        if i < 0 || i as usize >= self.strings.len() {
            return Ok(None);
        }
        let s = &self.strings[i as usize..];
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        Ok(Some(String::from_utf16_lossy(&s[..end])))
    }
}

impl Tdt {
    pub fn parse(data: &[u8]) -> Result<Self, TdtError> {
        let mut r = Reader {
            data,
            pos: 0,
            strings: Vec::new(),
        };
        let version = u32::from_le_bytes(r.bytes()?);
        let chars = u32::from_le_bytes(r.bytes()?) as usize;
        let table = data.get(8..8 + chars * 2).ok_or(TdtError {
            offset: 8,
            what: "string table truncated",
        })?;
        r.strings = table
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        r.pos = 8 + chars * 2;
        let mut t = Tdt {
            version,
            ..Default::default()
        };
        t.inherits = r.string()?.filter(|s| !s.is_empty());
        let inherit = t.inherits.is_some();
        if inherit {
            t.inherit_flags = r.u8()?;
        }
        let iflags = t.inherit_flags;
        let has = move |bit: u8| !inherit || iflags & bit != 0;
        if has(1) {
            t.tgt = r.string()?.filter(|s| !s.is_empty());
        }
        if has(8) {
            t.feature = r.string()?.filter(|s| !s.is_empty());
            for e in &mut t.edges {
                *e = r.string()?.filter(|s| !s.is_empty());
            }
            t.size = (r.u8()?, r.u8()?);
            for g in &mut t.grounds {
                *g = r.string()?.filter(|s| !s.is_empty());
            }
            t.unk = r.bytes()?;
            t.edge_distances = r.bytes()?;
        } else if t.inherit_flags & 0x10 != 0 {
            t.size = (r.u8()?, r.u8()?);
        }
        t.rest = data[r.pos..].to_vec();
        if r.pos < data.len() {
            t.read_points(&mut r)?;
        }
        Ok(t)
    }

    /// This inheriting tile's overrides applied to its (resolved) parent.
    pub fn resolve(&self, parent: &Tdt) -> Tdt {
        let f = self.inherit_flags;
        let mut t = parent.clone();
        t.inherits = self.inherits.clone();
        t.inherit_flags = f;
        if f & 1 != 0 {
            t.tgt = self.tgt.clone();
        }
        if f & 8 != 0 {
            t.feature = self.feature.clone();
            t.edges = self.edges.clone();
            t.size = self.size;
            t.grounds = self.grounds.clone();
            t.unk = self.unk;
            t.edge_distances = self.edge_distances;
            t.point_grounds = self.point_grounds.clone();
            t.point_heights = self.point_heights.clone();
        } else if f & 0x10 != 0 {
            t.size = self.size;
        }
        if f & 0x10 != 0 {
            t.hull_masks = self.hull_masks.clone();
            t.subtiles = self.subtiles.clone();
        }
        if f & 0x20 != 0 && self.version > 9 {
            t.orientation_mask = self.orientation_mask;
        }
        t
    }

    /// Orientation mask `m` (bit per orientation) restricted by this tile's own mask.
    pub fn restrict_orientations(&self, m: u8) -> u8 {
        if (1..0xff).contains(&self.orientation_mask) { m & self.orientation_mask } else { m }
    }

    /// The body up to the corner points (`0x1429153f0`): flags, hull planes, sub-tiles (each
    /// with an optional compressed byte map and a byte list), then per-point grounds (flag 2)
    /// and heights (flag 1).
    fn read_points(&mut self, r: &mut Reader) -> Result<(), TdtError> {
        let v = self.version;
        let mut flags = u16::from_le_bytes(r.bytes()?);
        if v < 14 {
            flags &= !0x100;
        }
        if v < 13 {
            flags &= !0x200;
        }
        let hi = (flags >> 8) as u8;
        let (inherit, iflags) = (self.inherits.is_some(), self.inherit_flags);
        let has = move |bit: u8| !inherit || iflags & bit != 0;
        if v > 9 && has(0x20) {
            self.orientation_mask = r.u8()?;
        }
        if has(0x10) && v >= 10 {
            let n = r.u8()?;
            if n > 4 {
                return Err(TdtError { offset: r.pos, what: "invalid number of hull planes" });
            }
            self.hull_masks = vec![0; n.max(1) as usize];
            if n >= 2 {
                for i in 0..n as usize {
                    self.hull_masks[i] = r.u8()?;
                }
            }
        }
        if has(1) && hi & 2 != 0 {
            r.u8()?;
        }
        if has(8) || has(0x10) {
            let count = u16::from_le_bytes(r.bytes()?);
            let full = has(0x10) || (has(1) && hi & 2 != 0);
            for _ in 0..count {
                let mut st = SubTile::default();
                let w4 = match (full, v < 10) {
                    (false, _) => 0,
                    (true, true) => r.u8()?,
                    (true, false) => {
                        // The u16 is flags and a second flags byte (bit 0: left margin).
                        let [lo, hi] = r.bytes::<2>()?;
                        st.b1 = hi;
                        lo
                    }
                };
                if has(8) {
                    r.u8()?;
                }
                if has(1) && hi & 2 != 0 {
                    r.bytes::<3>()?;
                }
                if !has(0x10) {
                    continue;
                }
                st.flags = w4;
                if full && v < 10 && !(w4 & 1 != 0 && w4 & 2 != 0) {
                    // Old files: planes reach 4 cells past every side.
                    st.flags |= 0xe0;
                    st.b1 |= 1;
                }
                if w4 & 4 == 0 {
                    st.map = Some(read_byte_map(r, v)?);
                }
                if !(w4 & 1 != 0 && w4 & 2 != 0) {
                    let n = u16::from_le_bytes(r.bytes()?) as usize;
                    let end = r.pos.checked_add(n).filter(|&p| p <= r.data.len()).ok_or(TdtError {
                        offset: r.pos,
                        what: "sub-tile bytes truncated",
                    })?;
                    st.list = r.data[r.pos..end].to_vec();
                    r.pos = end;
                }
                self.subtiles.push(st);
            }
        }
        if !has(8) {
            return Ok(());
        }
        let n = (self.size.0 as usize + 1) * (self.size.1 as usize + 1);
        if flags & 2 != 0 {
            for _ in 0..n {
                let g = r.string()?.filter(|s| !s.is_empty());
                self.point_grounds.push(g);
            }
        }
        if flags & 1 != 0 {
            for _ in 0..n {
                self.point_heights.push(r.u8()? as i8);
            }
        }
        Ok(())
    }
}

/// A sub-tile byte map (`0x1429179c0`): raw 0x211 bytes before version 10, else a header byte
/// (kind in bits 0..2, extra size bytes in bits 3..4, size in bits 5..7) and, for kinds 1..4, an
/// LZ4 block of that size that decodes to 69 (kind 1: 2 palette bytes and 1 bit per cell),
/// 137 (kind 2: 4 palette bytes, 2 bits), 281 (kind 3: 16 palette bytes, 4 bits) or 529 bytes
/// (kind 4: one byte per cell). Kind 0 has no payload: the size field is the uniform value.
fn read_byte_map(r: &mut Reader, version: u32) -> Result<Vec<u8>, TdtError> {
    let trunc = |pos| TdtError { offset: pos, what: "byte map truncated" };
    if version < 10 {
        let end = r.pos + 0x211;
        let m = r.data.get(r.pos..end).ok_or(trunc(r.pos))?.to_vec();
        r.pos = end;
        return Ok(m);
    }
    let b = r.u8()?;
    let kind = b & 7;
    let extra = (b >> 3) & 3;
    let mut size = ((b >> 5) & 7) as usize;
    for k in 0..extra {
        size |= (r.u8()? as usize) << (8 * k as usize + 3);
    }
    if kind == 0 {
        return Ok(if size == 0 { Vec::new() } else { vec![size as u8] });
    }
    let end = r.pos.checked_add(size).filter(|&p| p <= r.data.len()).ok_or(trunc(r.pos))?;
    let payload = &r.data[r.pos..end];
    r.pos = end;
    let out = match kind {
        1 => 69,
        2 => 137,
        3 => 281,
        4 => 529,
        _ => return Err(TdtError { offset: r.pos, what: "unknown data compression" }),
    };
    let raw = lz4_block(payload, out).ok_or(TdtError { offset: r.pos, what: "failed to decompress byte map" })?;
    let bits = match kind {
        1 => 1,
        2 => 2,
        3 => 4,
        _ => return Ok(raw),
    };
    let pal = 1usize << bits;
    let (palette, data) = raw.split_at(pal);
    Ok((0..529usize)
        .map(|i| {
            let bit = i * bits;
            let v = (data[bit / 8] >> (bit % 8)) & ((1 << bits) - 1);
            palette[v as usize]
        })
        .collect())
}

/// LZ4 block decompression to exactly `out` bytes.
fn lz4_block(src: &[u8], out: usize) -> Option<Vec<u8>> {
    lz4_decode(src).filter(|d| d.len() == out)
}

/// LZ4 block decompression (the output length is whatever the block holds).
pub fn lz4_decode(src: &[u8]) -> Option<Vec<u8>> {
    let mut dst: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let token = src[i];
        i += 1;
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            loop {
                let b = *src.get(i)?;
                i += 1;
                lit += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        dst.extend_from_slice(src.get(i..i + lit)?);
        i += lit;
        if i >= src.len() {
            break;
        }
        let off = u16::from_le_bytes([*src.get(i)?, *src.get(i + 1)?]) as usize;
        i += 2;
        if off == 0 || off > dst.len() {
            return None;
        }
        let mut len = (token & 15) as usize;
        if len == 15 {
            loop {
                let b = *src.get(i)?;
                i += 1;
                len += b as usize;
                if b != 255 {
                    break;
                }
            }
        }
        len += 4;
        for _ in 0..len {
            let b = dst[dst.len() - off];
            dst.push(b);
        }
    }
    Some(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Metadata/Terrain/Blank.tdt` (0.5.5e), the whole file.
    #[test]
    fn blank() {
        let mut data = Vec::new();
        data.extend_from_slice(&14u32.to_le_bytes());
        let table = "Art/Models/Terrain/Blank.tgt\0";
        data.extend_from_slice(&(table.len() as u32).to_le_bytes());
        for u in table.encode_utf16() {
            data.extend_from_slice(&u.to_le_bytes());
        }
        // inherits "" (the table's final NUL), tgt 0, feature "", four edges none.
        for v in [28i32, 0, 28, -1, -1, -1, -1] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        data.extend_from_slice(&[1, 1]);
        for _ in 0..4 {
            data.extend_from_slice(&(-1i32).to_le_bytes());
        }
        data.extend_from_slice(&[0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 3, 3]);
        let t = Tdt::parse(&data).unwrap();
        assert_eq!(t.tgt.as_deref(), Some("Art/Models/Terrain/Blank.tgt"));
        assert_eq!(t.inherits, None);
        assert_eq!(t.size, (1, 1));
        assert_eq!(t.edge_distances, [3; 8]);
    }
}
