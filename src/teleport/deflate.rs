//! RFC 1951 raw DEFLATE — hand-written, stdlib-only (the Fuaran teleport bundle
//! rides `base64url(deflate(canonical-JSON))`, and the Rust standard library has
//! no compression). The **encoder** emits a single fixed-Huffman block with
//! deterministic greedy LZ77 (32 KB window), so the same bytes always compress
//! to the same stream. The **decoder** accepts the full RFC 1951 range — stored,
//! fixed, and dynamic Huffman blocks — so a bundle produced by any conformant
//! deflate library inflates here, and caps its output to defuse a decompression
//! bomb.

/// The decoder's hard cap on inflated output (a decompression-bomb guard). The
/// teleport layer never needs more; the reference ceiling is the same 1 MB.
pub const MAX_INFLATE: usize = 1 << 20;

/// Inflate failed — corrupt stream, an over-cap output (a bomb), or a truncated
/// input. Surfaced to the teleport layer as `InvalidFormat`.
#[derive(Debug, Clone, PartialEq)]
pub struct InflateError(pub &'static str);

// ─── length / distance code tables (RFC 1951 §3.2.5) ─────────────────────────

// (extra-bits, base-length) for length codes 257..=285.
const LEN_EXTRA: [(u32, u32); 29] = [
    (0, 3),
    (0, 4),
    (0, 5),
    (0, 6),
    (0, 7),
    (0, 8),
    (0, 9),
    (0, 10),
    (1, 11),
    (1, 13),
    (1, 15),
    (1, 17),
    (2, 19),
    (2, 23),
    (2, 27),
    (2, 31),
    (3, 35),
    (3, 43),
    (3, 51),
    (3, 59),
    (4, 67),
    (4, 83),
    (4, 99),
    (4, 115),
    (5, 131),
    (5, 163),
    (5, 195),
    (5, 227),
    (0, 258),
];

// (extra-bits, base-distance) for distance codes 0..=29.
const DIST_EXTRA: [(u32, u32); 30] = [
    (0, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (1, 5),
    (1, 7),
    (2, 9),
    (2, 13),
    (3, 17),
    (3, 25),
    (4, 33),
    (4, 49),
    (5, 65),
    (5, 97),
    (6, 129),
    (6, 193),
    (7, 257),
    (7, 385),
    (8, 513),
    (8, 769),
    (9, 1025),
    (9, 1537),
    (10, 2049),
    (10, 3073),
    (11, 4097),
    (11, 6145),
    (12, 8193),
    (12, 12289),
    (13, 16385),
    (13, 24577),
];

// The fixed literal/length code lengths (RFC 1951 §3.2.6).
fn fixed_litlen_lengths() -> Vec<u8> {
    let mut l = vec![8u8; 288];
    for e in l.iter_mut().take(256).skip(144) {
        *e = 9;
    }
    for e in l.iter_mut().take(280).skip(256) {
        *e = 7;
    }
    l
}

fn fixed_dist_lengths() -> Vec<u8> {
    vec![5u8; 30]
}

// ─── canonical Huffman code assignment (RFC 1951 §3.2.2) ─────────────────────

/// Assign canonical codes from a code-length array — `(code, len)` per symbol
/// (a zero-length symbol is unused, `(0, 0)`). Shared by the encoder (to emit
/// bits) and the decoder table build.
fn assign_canonical(lengths: &[u8]) -> Vec<(u32, u8)> {
    let max_len = lengths.iter().copied().max().unwrap_or(0) as usize;
    let mut bl_count = vec![0u32; max_len + 1];
    for &l in lengths {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next_code = vec![0u32; max_len + 2];
    let mut code = 0u32;
    for bits in 1..=max_len {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            if l == 0 {
                (0, 0)
            } else {
                let c = next_code[l as usize];
                next_code[l as usize] += 1;
                (c, l)
            }
        })
        .collect()
}

// ─── bit writer (LSB-first stream; Huffman codes packed MSB-first) ───────────

struct BitWriter {
    bytes: Vec<u8>,
    cur: u32,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            bytes: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    fn append_bit(&mut self, bit: u32) {
        self.cur |= (bit & 1) << self.nbits;
        self.nbits += 1;
        if self.nbits == 8 {
            self.bytes.push(self.cur as u8);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    /// Write `n` low bits of `v`, least-significant first (extra bits, LEN/NLEN).
    fn write_bits(&mut self, v: u32, n: u32) {
        for i in 0..n {
            self.append_bit((v >> i) & 1);
        }
    }

    /// Write a Huffman `code` of `len` bits, most-significant first (§3.1.1).
    fn write_code(&mut self, code: u32, len: u8) {
        for i in (0..len).rev() {
            self.append_bit((code >> i) & 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.bytes.push(self.cur as u8);
        }
        self.bytes
    }
}

// ─── encoder — single fixed-Huffman block, greedy LZ77 ───────────────────────

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const WINDOW: usize = 32768;

fn length_code(len: usize) -> (usize, u32, u32) {
    // Returns (symbol 257.., extra-value, extra-bits).
    for (i, &(extra, base)) in LEN_EXTRA.iter().enumerate() {
        let hi = if i + 1 < LEN_EXTRA.len() {
            LEN_EXTRA[i + 1].1
        } else {
            259
        };
        if (len as u32) >= base && (len as u32) < hi {
            return (257 + i, len as u32 - base, extra);
        }
    }
    // len == 258 exact (last code, zero extra).
    (285, 0, 0)
}

fn dist_code(dist: usize) -> (usize, u32, u32) {
    for (i, &(extra, base)) in DIST_EXTRA.iter().enumerate() {
        let hi = if i + 1 < DIST_EXTRA.len() {
            DIST_EXTRA[i + 1].1
        } else {
            32769
        };
        if (dist as u32) >= base && (dist as u32) < hi {
            return (i, dist as u32 - base, extra);
        }
    }
    (29, 0, 0)
}

/// Deflate `input` into a raw RFC 1951 stream — one fixed-Huffman block,
/// deterministic greedy LZ77. Deterministic: identical input → identical bytes.
pub fn deflate(input: &[u8]) -> Vec<u8> {
    let litlen = assign_canonical(&fixed_litlen_lengths());
    let dist = assign_canonical(&fixed_dist_lengths());
    let mut w = BitWriter::new();
    w.write_bits(1, 1); // BFINAL = 1 (single block)
    w.write_bits(1, 2); // BTYPE = 01 (fixed Huffman)

    let n = input.len();
    let mut i = 0usize;
    while i < n {
        // Greedy longest match within the window, min length 3.
        let (mut best_len, mut best_dist) = (0usize, 0usize);
        if i + MIN_MATCH <= n {
            let start = i.saturating_sub(WINDOW);
            let max_here = MAX_MATCH.min(n - i);
            let mut j = start;
            while j < i {
                let mut l = 0usize;
                while l < max_here && input[j + l] == input[i + l] {
                    l += 1;
                }
                if l >= MIN_MATCH && l > best_len {
                    best_len = l;
                    best_dist = i - j;
                    if l == max_here {
                        break;
                    }
                }
                j += 1;
            }
        }
        if best_len >= MIN_MATCH {
            let (sym, ev, eb) = length_code(best_len);
            let (code, len) = litlen[sym];
            w.write_code(code, len);
            if eb > 0 {
                w.write_bits(ev, eb);
            }
            let (dsym, dev, deb) = dist_code(best_dist);
            let (dcode, dlen) = dist[dsym];
            w.write_code(dcode, dlen);
            if deb > 0 {
                w.write_bits(dev, deb);
            }
            i += best_len;
        } else {
            let (code, len) = litlen[input[i] as usize];
            w.write_code(code, len);
            i += 1;
        }
    }
    // End-of-block (symbol 256).
    let (code, len) = litlen[256];
    w.write_code(code, len);
    w.finish()
}

// ─── decoder — stored / fixed / dynamic ──────────────────────────────────────

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bitbuf: 0,
            bitcnt: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u32, InflateError> {
        if self.bitcnt == 0 {
            if self.pos >= self.data.len() {
                return Err(InflateError("truncated deflate stream"));
            }
            self.bitbuf = self.data[self.pos] as u32;
            self.pos += 1;
            self.bitcnt = 8;
        }
        let bit = self.bitbuf & 1;
        self.bitbuf >>= 1;
        self.bitcnt -= 1;
        Ok(bit)
    }

    fn read_bits(&mut self, n: u32) -> Result<u32, InflateError> {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.read_bit()? << i;
        }
        Ok(v)
    }

    fn align(&mut self) {
        self.bitcnt = 0;
        self.bitbuf = 0;
    }
}

// A canonical-Huffman decode table (puff-style: per-length counts + sorted symbols).
struct Huffman {
    counts: Vec<u32>,
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Huffman {
        let max_len = lengths.iter().copied().max().unwrap_or(0) as usize;
        let mut counts = vec![0u32; max_len + 1];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // Symbols sorted by (length, symbol value).
        let mut offsets = vec![0u32; max_len + 2];
        for len in 1..=max_len {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offsets[l as usize] as usize] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    fn decode(&self, r: &mut BitReader) -> Result<u16, InflateError> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..self.counts.len() {
            code |= r.read_bit()? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError("invalid Huffman code"))
    }
}

fn inflate_block_body(
    r: &mut BitReader,
    litlen: &Huffman,
    dist: &Huffman,
    out: &mut Vec<u8>,
) -> Result<(), InflateError> {
    loop {
        let sym = litlen.decode(r)?;
        if sym == 256 {
            return Ok(());
        }
        if sym < 256 {
            if out.len() >= MAX_INFLATE {
                return Err(InflateError("inflate output exceeds cap"));
            }
            out.push(sym as u8);
        } else {
            let li = (sym - 257) as usize;
            if li >= LEN_EXTRA.len() {
                return Err(InflateError("invalid length symbol"));
            }
            let (lextra, lbase) = LEN_EXTRA[li];
            let length = lbase + r.read_bits(lextra)?;
            let dsym = dist.decode(r)? as usize;
            if dsym >= DIST_EXTRA.len() {
                return Err(InflateError("invalid distance symbol"));
            }
            let (dextra, dbase) = DIST_EXTRA[dsym];
            let distance = (dbase + r.read_bits(dextra)?) as usize;
            if distance == 0 || distance > out.len() {
                return Err(InflateError("distance beyond output"));
            }
            let start = out.len() - distance;
            for k in 0..length as usize {
                if out.len() >= MAX_INFLATE {
                    return Err(InflateError("inflate output exceeds cap"));
                }
                out.push(out[start + k]);
            }
        }
    }
}

// The 19-entry code-length alphabet order (RFC 1951 §3.2.7).
const CL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn read_dynamic_tables(r: &mut BitReader) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = r.read_bits(5)? as usize + 257;
    let hdist = r.read_bits(5)? as usize + 1;
    let hclen = r.read_bits(4)? as usize + 4;
    let mut cl_lengths = [0u8; 19];
    for &slot in CL_ORDER.iter().take(hclen) {
        cl_lengths[slot] = r.read_bits(3)? as u8;
    }
    let cl_huff = Huffman::from_lengths(&cl_lengths);

    let total = hlit + hdist;
    let mut lengths: Vec<u8> = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_huff.decode(r)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let prev = *lengths
                    .last()
                    .ok_or(InflateError("repeat with no prior length"))?;
                let n = 3 + r.read_bits(2)?;
                lengths.extend(std::iter::repeat_n(prev, n as usize));
            }
            17 => {
                let n = 3 + r.read_bits(3)?;
                lengths.extend(std::iter::repeat_n(0u8, n as usize));
            }
            18 => {
                let n = 11 + r.read_bits(7)?;
                lengths.extend(std::iter::repeat_n(0u8, n as usize));
            }
            _ => return Err(InflateError("invalid code-length symbol")),
        }
    }
    if lengths.len() != total {
        return Err(InflateError("code-length overrun"));
    }
    let lit = Huffman::from_lengths(&lengths[..hlit]);
    let dst = Huffman::from_lengths(&lengths[hlit..]);
    Ok((lit, dst))
}

/// Inflate a raw RFC 1951 stream (no zlib/gzip wrapper). Accepts stored, fixed,
/// and dynamic Huffman blocks; caps output at [`MAX_INFLATE`].
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    let mut r = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();
    let fixed_lit = Huffman::from_lengths(&fixed_litlen_lengths());
    let fixed_dist = Huffman::from_lengths(&fixed_dist_lengths());
    loop {
        let bfinal = r.read_bit()?;
        let btype = r.read_bits(2)?;
        match btype {
            0 => {
                r.align();
                if r.pos + 4 > r.data.len() {
                    return Err(InflateError("truncated stored block header"));
                }
                let len = r.data[r.pos] as usize | ((r.data[r.pos + 1] as usize) << 8);
                let nlen = r.data[r.pos + 2] as usize | ((r.data[r.pos + 3] as usize) << 8);
                r.pos += 4;
                if len ^ 0xFFFF != nlen {
                    return Err(InflateError("stored block length check failed"));
                }
                if r.pos + len > r.data.len() {
                    return Err(InflateError("truncated stored block"));
                }
                if out.len() + len > MAX_INFLATE {
                    return Err(InflateError("inflate output exceeds cap"));
                }
                out.extend_from_slice(&r.data[r.pos..r.pos + len]);
                r.pos += len;
            }
            1 => inflate_block_body(&mut r, &fixed_lit, &fixed_dist, &mut out)?,
            2 => {
                let (lit, dst) = read_dynamic_tables(&mut r)?;
                inflate_block_body(&mut r, &lit, &dst, &mut out)?;
            }
            _ => return Err(InflateError("reserved block type")),
        }
        if bfinal == 1 {
            return Ok(out);
        }
    }
}
