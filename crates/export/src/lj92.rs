//! Lossless JPEG (ITU T.81, process 14 — "LJ92"), the compression DNG readers expect for linear data
//! (M9.10).
//!
//! ⚠ Written here because no Rust crate encodes it; decoders are everywhere (libraw, the DNG SDK,
//! darktable's rawspeed), so what matters is conformance, which the tests pin with a decoder of
//! their own and the DNG tests with libraw.
//!
//! One frame per call: `components` interleaved samples of `precision` bits, predictor 1 (the
//! sample to the left; above for the first column; `2^(P−1)` for the very first), point transform
//! 0, and ONE Huffman table shared by the components, built from this frame's own statistics
//! (Annex K.2, limited to 16-bit codes).

/// Encode `samples` — `width × height × components`, row-major, interleaved — as one lossless JPEG.
pub fn encode(samples: &[u16], width: usize, height: usize, components: usize) -> Vec<u8> {
    assert_eq!(samples.len(), width * height * components);
    assert!((1..=4).contains(&components) && width > 0 && height > 0);
    const P: u32 = 16;

    // Pass 1: the differences, as (category, extra bits).
    let at = |x: usize, y: usize, c: usize| samples[(y * width + x) * components + c] as i32;
    let mut diffs: Vec<(u8, u16)> = Vec::with_capacity(samples.len());
    let mut freq = [0u32; 17];
    for y in 0..height {
        for x in 0..width {
            for c in 0..components {
                let pred = match (x, y) {
                    (0, 0) => 1 << (P - 1),
                    (0, _) => at(0, y - 1, c),
                    _ => at(x - 1, y, c),
                };
                // Modulo 2^16, read as a signed value in [−32768, 32767].
                let d = ((at(x, y, c) - pred) as i16) as i32;
                let (cat, bits) = category(d);
                freq[cat as usize] += 1;
                diffs.push((cat, bits));
            }
        }
    }

    let (lengths, symbols) = huffman_table(&freq);
    let codes = canonical_codes(&lengths, &symbols);

    let mut out = Vec::new();
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
                                          // DHT: class 0 (DC/lossless), table 0.
    let dht_len = 2 + 1 + 16 + symbols.len();
    out.extend_from_slice(&[0xFF, 0xC4]);
    out.extend_from_slice(&(dht_len as u16).to_be_bytes());
    out.push(0x00);
    for n in lengths {
        out.push(n);
    }
    out.extend_from_slice(&symbols);
    // SOF3.
    out.extend_from_slice(&[0xFF, 0xC3]);
    out.extend_from_slice(&((8 + 3 * components) as u16).to_be_bytes());
    out.push(P as u8);
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.push(components as u8);
    for c in 0..components {
        out.extend_from_slice(&[c as u8 + 1, 0x11, 0]);
    }
    // SOS: every component on table 0; Ss = predictor 1, Se = 0, Ah/Al = 0.
    out.extend_from_slice(&[0xFF, 0xDA]);
    out.extend_from_slice(&((6 + 2 * components) as u16).to_be_bytes());
    out.push(components as u8);
    for c in 0..components {
        out.extend_from_slice(&[c as u8 + 1, 0x00]);
    }
    out.extend_from_slice(&[1, 0, 0]);

    let mut w = BitWriter::new(&mut out);
    for (cat, bits) in diffs {
        let (code, len) = codes[cat as usize];
        w.put(code as u32, len as u32);
        // ⚠ Category 16 carries no extra bits: its only value, −32768, is implied (H.1.2.2).
        if cat > 0 && cat < 16 {
            w.put(bits as u32, cat as u32);
        }
    }
    w.flush();
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}

/// A difference's category (bit length of its magnitude) and its extra bits (one's complement for
/// a negative value, H.1.2.2).
fn category(d: i32) -> (u8, u16) {
    if d == 0 {
        return (0, 0);
    }
    if d == -32768 {
        return (16, 0);
    }
    let m = d.unsigned_abs();
    let cat = 32 - m.leading_zeros();
    let bits = if d > 0 { d as u32 } else { (d - 1) as u32 } & ((1 << cat) - 1);
    (cat as u8, bits as u16)
}

/// Code lengths (`BITS`, counts per length 1..=16) and symbols ordered by length, for `freq` —
/// T.81 Annex K.2, which reserves the all-ones code and limits lengths to 16.
fn huffman_table(freq: &[u32; 17]) -> ([u8; 16], Vec<u8>) {
    // K.2 works on 257 slots: the real symbols plus one reserved pseudo-symbol that guarantees no
    // code is all ones.
    let mut f: Vec<i64> = freq.iter().map(|&v| v as i64).collect();
    f.resize(257, 0);
    f[256] = 1;
    let mut codesize = vec![0u32; 257];
    let mut others = vec![-1i64; 257];
    loop {
        // v1: least frequency; v2: next least (ties → larger index, per K.2).
        let mut v1 = -1i64;
        let mut v2 = -1i64;
        for i in 0..257 {
            if f[i] > 0 && (v1 < 0 || f[i] <= f[v1 as usize]) {
                v1 = i as i64;
            }
        }
        for i in 0..257 {
            if f[i] > 0 && i as i64 != v1 && (v2 < 0 || f[i] <= f[v2 as usize]) {
                v2 = i as i64;
            }
        }
        if v2 < 0 {
            break;
        }
        let (a, b) = (v1 as usize, v2 as usize);
        f[a] += f[b];
        f[b] = 0;
        let mut k = a;
        codesize[k] += 1;
        while others[k] >= 0 {
            k = others[k] as usize;
            codesize[k] += 1;
        }
        others[k] = b as i64;
        let mut k = b;
        codesize[k] += 1;
        while others[k] >= 0 {
            k = others[k] as usize;
            codesize[k] += 1;
        }
    }
    let mut bits = [0i64; 33];
    for &cs in &codesize {
        if cs > 0 {
            bits[cs as usize] += 1;
        }
    }
    // Adjust to at most 16 bits (K.2, figure K.3).
    let mut i = 32;
    while i > 16 {
        while bits[i] > 0 {
            let mut j = i - 2;
            while bits[j] == 0 {
                j -= 1;
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
        }
        i -= 1;
    }
    // Remove the reserved code point.
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1;
    let mut out_bits = [0u8; 16];
    for (k, b) in out_bits.iter_mut().enumerate() {
        *b = bits[k + 1] as u8;
    }
    // Symbols in order of code size, then value.
    let mut symbols = Vec::new();
    for size in 1..=32u32 {
        for (s, &cs) in codesize.iter().enumerate().take(17) {
            if cs == size {
                symbols.push(s as u8);
            }
        }
    }
    (out_bits, symbols)
}

/// `(code, length)` per symbol 0..=16 from a table (C.1/C.2).
fn canonical_codes(bits: &[u8; 16], symbols: &[u8]) -> [(u16, u8); 17] {
    let mut table = [(0u16, 0u8); 17];
    let mut code = 0u32;
    let mut k = 0;
    for (len_minus_1, &n) in bits.iter().enumerate() {
        for _ in 0..n {
            table[symbols[k] as usize] = (code as u16, len_minus_1 as u8 + 1);
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    table
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u64,
    n: u32,
}

impl<'a> BitWriter<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out, acc: 0, n: 0 }
    }

    fn put(&mut self, v: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (v as u64 & ((1u64 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            self.n -= 8;
            let byte = (self.acc >> self.n) as u8;
            self.out.push(byte);
            // ⚠ Byte stuffing: a 0xFF in the entropy-coded data is followed by 0x00 (F.1.2.3).
            if byte == 0xFF {
                self.out.push(0);
            }
        }
    }

    /// Pad the last byte with ones (F.1.2.3).
    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal decoder for what [`encode`] writes — the round trip that pins conformance of the
    /// bitstream to its own tables. (libraw is the independent reader, in the DNG tests.)
    pub(crate) fn decode(data: &[u8]) -> (Vec<u16>, usize, usize, usize) {
        let mut i = 2;
        let (mut bits, mut symbols) = ([0u8; 16], Vec::new());
        let (mut w, mut h, mut nc) = (0, 0, 0);
        loop {
            assert_eq!(data[i], 0xFF);
            let marker = data[i + 1];
            let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            let seg = &data[i + 4..i + 2 + len];
            match marker {
                0xC4 => {
                    bits.copy_from_slice(&seg[1..17]);
                    symbols = seg[17..].to_vec();
                }
                0xC3 => {
                    h = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                    w = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                    nc = seg[5] as usize;
                }
                0xDA => {
                    i += 2 + len;
                    break;
                }
                _ => {}
            }
            i += 2 + len;
        }
        let codes = canonical_codes(&bits, &symbols);
        // Unstuff.
        let mut stream = Vec::new();
        let mut k = i;
        while k < data.len() {
            if data[k] == 0xFF {
                if data[k + 1] == 0 {
                    stream.push(0xFF);
                    k += 2;
                    continue;
                }
                break;
            }
            stream.push(data[k]);
            k += 1;
        }
        let mut pos = 0usize;
        let bit = |pos: &mut usize| -> u32 {
            let b = (stream[*pos / 8] >> (7 - *pos % 8)) & 1;
            *pos += 1;
            b as u32
        };
        let mut out = vec![0u16; w * h * nc];
        for y in 0..h {
            for x in 0..w {
                for c in 0..nc {
                    let (mut code, mut len) = (0u32, 0u8);
                    let cat = loop {
                        code = (code << 1) | bit(&mut pos);
                        len += 1;
                        if let Some(s) = (0..17).find(|&s| codes[s] == (code as u16, len)) {
                            break s as u32;
                        }
                        assert!(len <= 16, "no code matched");
                    };
                    let d: i32 = match cat {
                        0 => 0,
                        16 => -32768,
                        n => {
                            let mut v = 0u32;
                            for _ in 0..n {
                                v = (v << 1) | bit(&mut pos);
                            }
                            if v < (1 << (n - 1)) {
                                v as i32 - (1 << n) + 1
                            } else {
                                v as i32
                            }
                        }
                    };
                    let at = |x: usize, y: usize| out[(y * w + x) * nc + c] as i32;
                    let pred = match (x, y) {
                        (0, 0) => 1 << 15,
                        (0, _) => at(0, y - 1),
                        _ => at(x - 1, y),
                    };
                    out[(y * w + x) * nc + c] = (pred + d) as u16;
                }
            }
        }
        (out, w, h, nc)
    }

    fn round_trip(samples: &[u16], w: usize, h: usize, nc: usize) {
        let enc = encode(samples, w, h, nc);
        let (dec, dw, dh, dnc) = decode(&enc);
        assert_eq!((dw, dh, dnc), (w, h, nc));
        assert_eq!(dec, samples, "the round trip must be bit-exact");
    }

    #[test]
    fn a_smooth_ramp_round_trips_and_compresses() {
        let (w, h) = (64, 32);
        let s: Vec<u16> = (0..w * h * 3)
            .map(|i| ((i / 3 % w) * 900 + (i / 3 / w) * 37 + (i % 3) * 5000) as u16)
            .collect();
        round_trip(&s, w, h, 3);
        let ratio = encode(&s, w, h, 3).len() as f64 / (s.len() * 2) as f64;
        // ⚠ Measured 0.690, and derivable: every difference is the ramp's step of 900, category
        // 10 — ten extra bits plus a one-bit code — so 11 of 16 bits, plus the headers. A real
        // photograph's differences are smaller; its ratio is measured in the DNG work.
        assert!(ratio < 0.72, "{ratio}");
    }

    /// ⚠⚠ The extremes: 0 and 65535 side by side produce the ±32768-wrapped difference and the
    /// category-16 code, which an encoder can get wrong while every ordinary image round-trips.
    #[test]
    fn extremes_and_the_category_16_difference_round_trip() {
        let s: Vec<u16> = vec![
            0, 65535, 32768, 65535, 0, 1, 32767, 32768, 0, 0, 65535, 65535, 1, 65534, 12345, 54321,
            0, 32768,
        ];
        round_trip(&s, 3, 2, 3);
        assert_eq!(category(-32768), (16, 0));
    }

    #[test]
    fn noise_round_trips_with_many_categories() {
        let mut x: u32 = 12345;
        let s: Vec<u16> = (0..40 * 40)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (x >> 8) as u16
            })
            .collect();
        round_trip(&s, 40, 40, 1);
    }
}
