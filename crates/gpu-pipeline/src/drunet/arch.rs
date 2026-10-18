//! DRUNet's shape — the weights it needs, checked — and its program: every convolution in order, with the
//! buffers it reads and writes. Pure: no `wgpu`.

use std::collections::HashMap;

/// A weight as the GPU engine takes it: its shape and its values, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// DRUNet's weights by their names in the file.
pub type Weights = HashMap<String, Tensor>;

/// DRUNet's size: the channels at each of its four scales and the residual blocks a level — the file's
/// `[64, 128, 256, 512]` and 4; a test's much smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Arch {
    pub widths: [usize; 4],
    pub blocks: usize,
}

/// The kernel's three index modes — its `MODE` constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    /// 3 × 3, stride 1, zero-padded by 1.
    Conv3 = 0,
    /// 2 × 2, stride 2.
    Down2 = 1,
    /// 2 × 2 transposed, stride 2.
    Up2 = 2,
    /// Winograd's product (M10.16b5): 16 batched `[blocks × cin] · [cin × cout]`, one a dispatch's `z` — a
    /// Conv3 op becomes this between two transforms when the network runs Winograd; `program` never emits it.
    Product = 3,
}

/// A buffer of the program by its role: the tile's input and output, the skip scale `s` hands from its
/// down level to its up level, and three work buffers a scale — slot 0 the ReLU temporary, 1 and 2 the
/// block's input and output in turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Buf {
    Input,
    Output,
    Skip(usize),
    Work(usize, usize),
}

impl Buf {
    /// The scale the buffer lives at — the input and output at full size.
    pub(crate) fn scale(self) -> usize {
        match self {
            Buf::Input | Buf::Output => 0,
            Buf::Skip(s) | Buf::Work(s, _) => s,
        }
    }
}

/// One convolution of the program.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Op {
    pub mode: Mode,
    pub weight: String,
    /// The bias added to each output channel before ReLU — FFDNet's (M10.16b6); DRUNet has none.
    pub bias: Option<String>,
    pub src: Buf,
    pub dst: Buf,
    /// ReLU on the store (a block's first convolution).
    pub relu: bool,
    /// Added on the store: the block's input (its residual).
    pub add1: Option<Buf>,
    /// Added on the store too: the level's skip (a level's last block).
    pub add2: Option<Buf>,
    /// `src`'s scale.
    pub scale_in: usize,
    pub cin: usize,
    pub cout: usize,
}

fn res(prefix: &str, w: usize) -> [(String, Vec<usize>); 2] {
    [
        (format!("{prefix}.res.0.weight"), vec![w, w, 3, 3]),
        (format!("{prefix}.res.2.weight"), vec![w, w, 3, 3]),
    ]
}

/// Every weight `a` needs, with its shape — ConvTranspose's as ONNX writes it, `[in, out, kh, kw]`.
pub(crate) fn names(a: &Arch) -> Vec<(String, Vec<usize>)> {
    let (w, nb) = (a.widths, a.blocks);
    let mut out = vec![("m_head.weight".to_string(), vec![w[0], 4, 3, 3])];
    for l in 1..=3 {
        for b in 0..nb {
            out.extend(res(&format!("m_down{l}.{b}"), w[l - 1]));
        }
        out.push((format!("m_down{l}.{nb}.weight"), vec![w[l], w[l - 1], 2, 2]));
    }
    for b in 0..nb {
        out.extend(res(&format!("m_body.{b}"), w[3]));
    }
    for l in (1..=3).rev() {
        out.push((format!("m_up{l}.0.weight"), vec![w[l], w[l - 1], 2, 2]));
        for b in 0..nb {
            out.extend(res(&format!("m_up{l}.{}", b + 1), w[l - 1]));
        }
    }
    out.push(("m_tail.weight".to_string(), vec![3, w[0], 3, 3]));
    out
}

impl Arch {
    /// The shape `w` describes — every weight present with its expected shape, or the first that is
    /// not, named.
    pub(crate) fn of(w: &Weights) -> Result<Arch, String> {
        let dim = |name: &str, i: usize| -> Result<usize, String> {
            w.get(name)
                .and_then(|t| t.shape.get(i).copied())
                .ok_or_else(|| format!("{name}: missing"))
        };
        let w0 = dim("m_head.weight", 0)?;
        let blocks = (0..)
            .take_while(|b| w.contains_key(&format!("m_down1.{b}.res.0.weight")))
            .count();
        if blocks == 0 {
            return Err("m_down1.0.res.0.weight: missing".to_string());
        }
        let mut widths = [w0, 0, 0, 0];
        for (l, width) in widths.iter_mut().enumerate().skip(1) {
            *width = dim(&format!("m_down{l}.{blocks}.weight"), 0)?;
        }
        let a = Arch { widths, blocks };
        for (name, shape) in names(&a) {
            match w.get(&name) {
                None => return Err(format!("{name}: missing")),
                Some(t) if t.shape != shape => {
                    return Err(format!("{name}: {:?}, not {shape:?}", t.shape))
                }
                Some(t) if t.data.len() != shape.iter().product::<usize>() => {
                    return Err(format!("{name}: {} values", t.data.len()))
                }
                _ => {}
            }
        }
        Ok(a)
    }
}

fn conv(weight: String, mode: Mode, src: Buf, dst: Buf, cin: usize, cout: usize) -> Op {
    Op {
        mode,
        weight,
        bias: None,
        src,
        dst,
        relu: false,
        add1: None,
        add2: None,
        scale_in: src.scale(),
        cin,
        cout,
    }
}

/// A level's residual blocks at scale `s` from `cur` — `t = relu(conv(cur)); cur = cur + conv(t)` — the
/// last one's store also adding `skip`. The block's output never overwrites the buffer it reads.
fn blocks(
    ops: &mut Vec<Op>,
    a: &Arch,
    prefix: &dyn Fn(usize) -> String,
    s: usize,
    mut cur: Buf,
    skip: Option<Buf>,
) -> Buf {
    let w = a.widths[s];
    for b in 0..a.blocks {
        let p = prefix(b);
        let t = Buf::Work(s, 0);
        ops.push(Op {
            relu: true,
            ..conv(format!("{p}.res.0.weight"), Mode::Conv3, cur, t, w, w)
        });
        let next = if cur == Buf::Work(s, 1) {
            Buf::Work(s, 2)
        } else {
            Buf::Work(s, 1)
        };
        ops.push(Op {
            add1: Some(cur),
            add2: if b + 1 == a.blocks { skip } else { None },
            ..conv(format!("{p}.res.2.weight"), Mode::Conv3, t, next, w, w)
        });
        cur = next;
    }
    cur
}

/// The program `a` runs, in order — the graph as the file draws it: the head, three down levels (their
/// blocks, then a down-convolution into the next skip), the body (its last block adding its own input
/// skip), three up levels (a transposed convolution, the blocks, the last adding the level's skip), the
/// tail.
pub(crate) fn program(a: &Arch) -> Vec<Op> {
    let (w, nb) = (a.widths, a.blocks);
    let mut ops = vec![conv(
        "m_head.weight".to_string(),
        Mode::Conv3,
        Buf::Input,
        Buf::Skip(0),
        4,
        w[0],
    )];
    for l in 1..=3 {
        let s = l - 1;
        let cur = blocks(
            &mut ops,
            a,
            &|b| format!("m_down{l}.{b}"),
            s,
            Buf::Skip(s),
            None,
        );
        ops.push(conv(
            format!("m_down{l}.{nb}.weight"),
            Mode::Down2,
            cur,
            Buf::Skip(l),
            w[s],
            w[l],
        ));
    }
    let mut cur = blocks(
        &mut ops,
        a,
        &|b| format!("m_body.{b}"),
        3,
        Buf::Skip(3),
        Some(Buf::Skip(3)),
    );
    for l in (1..=3).rev() {
        let s = l - 1;
        ops.push(conv(
            format!("m_up{l}.0.weight"),
            Mode::Up2,
            cur,
            Buf::Work(s, 1),
            w[l],
            w[s],
        ));
        cur = blocks(
            &mut ops,
            a,
            &|b| format!("m_up{l}.{}", b + 1),
            s,
            Buf::Work(s, 1),
            Some(Buf::Skip(s)),
        );
    }
    ops.push(conv(
        "m_tail.weight".to_string(),
        Mode::Conv3,
        cur,
        Buf::Output,
        w[0],
        3,
    ));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fabricate(a: &Arch) -> Weights {
        names(a)
            .into_iter()
            .map(|(n, s)| {
                let len = s.iter().product();
                (
                    n,
                    Tensor {
                        shape: s,
                        data: vec![0.0; len],
                    },
                )
            })
            .collect()
    }

    #[test]
    fn drunets_own_shape_is_recognised() {
        let a = Arch {
            widths: [64, 128, 256, 512],
            blocks: 4,
        };
        assert_eq!(names(&a).len(), 64, "the file's 64 initializers");
        let got = Arch::of(&fabricate(&a)).expect("recognised");
        assert_eq!((got.widths, got.blocks), (a.widths, a.blocks));
    }

    #[test]
    fn a_missing_or_misshapen_weight_is_named() {
        let a = Arch {
            widths: [4, 8, 16, 32],
            blocks: 2,
        };
        let mut w = fabricate(&a);
        w.remove("m_up2.1.res.2.weight");
        assert!(Arch::of(&w).unwrap_err().contains("m_up2.1.res.2.weight"));
        let mut w = fabricate(&a);
        w.get_mut("m_tail.weight").unwrap().shape = vec![3, 4, 2, 2];
        assert!(Arch::of(&w).unwrap_err().contains("m_tail.weight"));
    }

    /// The program: 64 convolutions for DRUNet, each weight used once, no buffer read while it is
    /// written, a skip written once by its producer, and each of the four skips added exactly once.
    #[test]
    fn the_program_follows_the_graph() {
        let a = Arch {
            widths: [64, 128, 256, 512],
            blocks: 4,
        };
        let p = program(&a);
        assert_eq!(p.len(), 64);
        let mut used: Vec<&str> = p.iter().map(|o| o.weight.as_str()).collect();
        used.sort();
        used.dedup();
        assert_eq!(used.len(), 64, "a weight used twice");
        for o in &p {
            assert!(
                o.src != o.dst && o.add1 != Some(o.dst) && o.add2 != Some(o.dst),
                "{o:?}"
            );
        }
        for s in 0..4 {
            let writes = p.iter().filter(|o| o.dst == Buf::Skip(s)).count();
            let adds = p.iter().filter(|o| o.add2 == Some(Buf::Skip(s))).count();
            assert_eq!((writes, adds), (1, 1), "Skip({s})");
        }
        assert_eq!(p[0].src, Buf::Input);
        assert_eq!(p.last().unwrap().dst, Buf::Output);
    }
}
