//! PyTorch's legacy weights format (M10.16b6, FFDNet's `ffdnet_color.pth`): five protocol-2 pickles — the
//! magic number, the format version, the system's facts, the state dict, the storage keys — then each
//! storage as its element count and its float32 values.
//!
//! ⚠⚠ **A pickle is a program, and this never runs one.** The unpickler is a closed subset: the opcodes a
//! state dict uses (read off the real file with `pickletools`) and exactly three globals — the dict, the
//! tensor's rebuild and the float storage — each a value this reader builds itself. Anything else is an
//! error naming it.

use crate::denoise::Weight;
use crate::infer::InferError;

/// The magic number torch writes first, `0x1950a86a20f9469cfc6c`, little-endian as `LONG1` carries it.
const MAGIC: [u8; 10] = [0x6c, 0xfc, 0x9c, 0x46, 0xf9, 0x20, 0x6a, 0xa8, 0x50, 0x19];
/// The legacy format's version.
const VERSION: i64 = 1001;

/// The weights of a legacy `.pth` file at `path`, in the state dict's order.
pub fn read(path: &std::path::Path) -> Result<Vec<Weight>, InferError> {
    parse(&std::fs::read(path).map_err(|e| bad(format!("{}: {e}", path.display())))?)
}

/// [`read`] from the file's bytes.
pub fn parse(bytes: &[u8]) -> Result<Vec<Weight>, InferError> {
    let mut r = Reader { b: bytes, at: 0 };
    match r.pickle()? {
        V::Long(m) if m == MAGIC => {}
        _ => return Err(bad("not a legacy PyTorch file (no magic number)")),
    }
    match r.pickle()? {
        V::Int(VERSION) => {}
        v => return Err(bad(format!("format version {v:?}, not {VERSION}"))),
    }
    let little = match r.pickle()? {
        V::Dict(d) => d
            .iter()
            .any(|(k, v)| matches!((k, v), (V::Str(k), V::Bool(true)) if k == "little_endian")),
        _ => false,
    };
    if !little {
        return Err(bad("a big-endian file"));
    }
    let state = match r.pickle()? {
        V::Dict(d) => d,
        _ => return Err(bad("the state is not a dict")),
    };
    let keys = match r.pickle()? {
        V::List(k) => k,
        _ => return Err(bad("no storage keys")),
    };
    let mut storages: Vec<(String, Vec<f32>)> = Vec::new();
    for k in keys {
        let V::Str(k) = k else {
            return Err(bad("a storage key is not a string"));
        };
        let n = usize::try_from(i64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes")))
            .map_err(|_| bad("a negative storage size"))?;
        let raw = r.take(n.checked_mul(4).ok_or_else(|| bad("a storage too large"))?)?;
        let floats = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        storages.push((k, floats));
    }
    state
        .into_iter()
        .map(|(name, t)| {
            let (V::Str(name), V::Tensor(t)) = (name, t) else {
                return Err(bad("a state entry is not a named tensor"));
            };
            let data = storages
                .iter()
                .find(|(k, _)| *k == t.key)
                .ok_or_else(|| bad(format!("{name}: storage {} missing", t.key)))?;
            let n: usize = t.size.iter().product();
            let mut want = vec![1usize; t.size.len()];
            for i in (0..t.size.len().saturating_sub(1)).rev() {
                want[i] = want[i + 1] * t.size[i + 1];
            }
            if n > 1 && want != t.stride {
                return Err(bad(format!("{name}: not contiguous")));
            }
            let values = data
                .1
                .get(t.offset..t.offset + n)
                .ok_or_else(|| bad(format!("{name}: past its storage's end")))?
                .to_vec();
            Ok((name, t.size, values))
        })
        .collect()
}

fn bad(e: impl std::fmt::Display) -> InferError {
    InferError::Model(format!("weights file: {e}"))
}

/// The three globals a state dict names — nothing else is ever constructed.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Global {
    OrderedDict,
    RebuildTensor,
    FloatStorage,
}

#[derive(Debug, Clone)]
struct TensorRef {
    key: String,
    offset: usize,
    size: Vec<usize>,
    stride: Vec<usize>,
}

#[derive(Debug, Clone)]
enum V {
    Mark,
    None,
    Bool(bool),
    Int(i64),
    Long(Vec<u8>),
    Str(String),
    Tuple(Vec<V>),
    List(Vec<V>),
    Dict(Vec<(V, V)>),
    Global(Global),
    Storage(String),
    Tensor(TensorRef),
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], InferError> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len());
        let end = end.ok_or_else(|| bad("truncated"))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, InferError> {
        Ok(self.take(1)?[0])
    }

    fn line(&mut self) -> Result<String, InferError> {
        let rest = &self.b[self.at..];
        let n = rest
            .iter()
            .position(|c| *c == b'\n')
            .ok_or_else(|| bad("truncated"))?;
        let s = String::from_utf8_lossy(&rest[..n]).into_owned();
        self.at += n + 1;
        Ok(s)
    }

    /// One pickle, up to its STOP.
    fn pickle(&mut self) -> Result<V, InferError> {
        let mut stack: Vec<V> = Vec::new();
        let mut memo: std::collections::HashMap<u32, V> = std::collections::HashMap::new();
        let pop = |stack: &mut Vec<V>| stack.pop().ok_or_else(|| bad("stack underflow"));
        let to_mark = |stack: &mut Vec<V>| -> Result<Vec<V>, InferError> {
            let m = stack
                .iter()
                .rposition(|v| matches!(v, V::Mark))
                .ok_or_else(|| bad("no mark"))?;
            let items = stack.split_off(m + 1);
            stack.pop();
            Ok(items)
        };
        loop {
            let op = self.u8()?;
            match op {
                0x80 => {
                    self.u8()?;
                }
                b'.' => return pop(&mut stack),
                b'(' => stack.push(V::Mark),
                b'N' => stack.push(V::None),
                0x88 => stack.push(V::Bool(true)),
                0x89 => stack.push(V::Bool(false)),
                b'K' => stack.push(V::Int(self.u8()? as i64)),
                b'M' => {
                    let b = self.take(2)?;
                    stack.push(V::Int(u16::from_le_bytes([b[0], b[1]]) as i64));
                }
                b'J' => {
                    let b = self.take(4)?;
                    stack.push(V::Int(i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as i64));
                }
                0x8a => {
                    let n = self.u8()? as usize;
                    stack.push(V::Long(self.take(n)?.to_vec()));
                }
                b'X' => {
                    let b = self.take(4)?;
                    let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
                    stack.push(V::Str(String::from_utf8_lossy(self.take(n)?).into_owned()));
                }
                b'U' => {
                    let n = self.u8()? as usize;
                    stack.push(V::Str(String::from_utf8_lossy(self.take(n)?).into_owned()));
                }
                b')' => stack.push(V::Tuple(Vec::new())),
                b'}' => stack.push(V::Dict(Vec::new())),
                b']' => stack.push(V::List(Vec::new())),
                b't' => {
                    let items = to_mark(&mut stack)?;
                    stack.push(V::Tuple(items));
                }
                0x85..=0x87 => {
                    let n = (op - 0x84) as usize;
                    let at = stack
                        .len()
                        .checked_sub(n)
                        .ok_or_else(|| bad("stack underflow"))?;
                    let items = stack.split_off(at);
                    stack.push(V::Tuple(items));
                }
                b'q' => {
                    let k = self.u8()? as u32;
                    memo.insert(k, stack.last().cloned().ok_or_else(|| bad("empty"))?);
                }
                b'r' => {
                    let b = self.take(4)?;
                    let k = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    memo.insert(k, stack.last().cloned().ok_or_else(|| bad("empty"))?);
                }
                b'h' => {
                    let k = self.u8()? as u32;
                    stack.push(memo.get(&k).cloned().ok_or_else(|| bad("memo miss"))?);
                }
                b'j' => {
                    let b = self.take(4)?;
                    let k = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    stack.push(memo.get(&k).cloned().ok_or_else(|| bad("memo miss"))?);
                }
                b'c' => {
                    let (module, name) = (self.line()?, self.line()?);
                    stack.push(V::Global(match (module.as_str(), name.as_str()) {
                        ("collections", "OrderedDict") => Global::OrderedDict,
                        ("torch._utils", "_rebuild_tensor_v2") => Global::RebuildTensor,
                        ("torch", "FloatStorage") => Global::FloatStorage,
                        _ => return Err(bad(format!("refused global {module}.{name}"))),
                    }));
                }
                b'Q' => {
                    let pid = pop(&mut stack)?;
                    stack.push(storage(pid)?);
                }
                b'R' => {
                    let args = pop(&mut stack)?;
                    let f = pop(&mut stack)?;
                    stack.push(reduce(f, args)?);
                }
                b's' | b'u' => {
                    let items = if op == b'u' {
                        to_mark(&mut stack)?
                    } else {
                        let v = pop(&mut stack)?;
                        let k = pop(&mut stack)?;
                        vec![k, v]
                    };
                    let Some(V::Dict(d)) = stack.last_mut() else {
                        return Err(bad("items set on a non-dict"));
                    };
                    let mut it = items.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        d.push((k, v));
                    }
                }
                b'a' | b'e' => {
                    let items = if op == b'e' {
                        to_mark(&mut stack)?
                    } else {
                        vec![pop(&mut stack)?]
                    };
                    let Some(V::List(l)) = stack.last_mut() else {
                        return Err(bad("items appended to a non-list"));
                    };
                    l.extend(items);
                }
                _ => return Err(bad(format!("refused opcode 0x{op:02x}"))),
            }
        }
    }
}

/// A persistent id: `('storage', FloatStorage, key, location, count, None)`.
fn storage(pid: V) -> Result<V, InferError> {
    match pid {
        V::Tuple(t) => match t.as_slice() {
            [V::Str(kind), V::Global(Global::FloatStorage), V::Str(key), V::Str(_), V::Int(_), V::None]
                if kind == "storage" =>
            {
                Ok(V::Storage(key.clone()))
            }
            _ => Err(bad(format!("refused persistent id {t:?}"))),
        },
        v => Err(bad(format!("refused persistent id {v:?}"))),
    }
}

fn dims(v: &V) -> Result<Vec<usize>, InferError> {
    match v {
        V::Tuple(t) => t
            .iter()
            .map(|d| match d {
                V::Int(n) if *n >= 0 => Ok(*n as usize),
                _ => Err(bad("a size is not a count")),
            })
            .collect(),
        _ => Err(bad("a size is not a tuple")),
    }
}

/// The only two calls a state dict makes: an empty dict, and a tensor's rebuild.
fn reduce(f: V, args: V) -> Result<V, InferError> {
    let V::Tuple(a) = args else {
        return Err(bad("call arguments are not a tuple"));
    };
    match (f, a.as_slice()) {
        (V::Global(Global::OrderedDict), []) => Ok(V::Dict(Vec::new())),
        (V::Global(Global::RebuildTensor), [V::Storage(key), V::Int(offset), size, stride, ..]) => {
            Ok(V::Tensor(TensorRef {
                key: key.clone(),
                offset: usize::try_from(*offset).map_err(|_| bad("a negative offset"))?,
                size: dims(size)?,
                stride: dims(stride)?,
            }))
        }
        (f, _) => Err(bad(format!("refused call of {f:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A legacy file written opcode by opcode, as `torch.save` does: two tensors, each its own storage.
    struct File(Vec<u8>);

    impl File {
        fn op(&mut self, b: &[u8]) -> &mut Self {
            self.0.extend_from_slice(b);
            self
        }
        fn s(&mut self, t: &str) -> &mut Self {
            self.0.push(b'X');
            self.0.extend_from_slice(&(t.len() as u32).to_le_bytes());
            self.op(t.as_bytes())
        }
        fn global(&mut self, module: &str, name: &str) -> &mut Self {
            self.op(format!("c{module}\n{name}\n").as_bytes())
        }
        fn tensor(&mut self, name: &str, key: &str, size: &[u8], storage_global: (&str, &str)) {
            self.s(name)
                .global("torch._utils", "_rebuild_tensor_v2")
                .op(b"((");
            self.s("storage")
                .global(storage_global.0, storage_global.1)
                .s(key)
                .s("cpu");
            let count: u8 = size.iter().product();
            self.op(&[b'K', count, b'N', b't', b'Q', b'K', 0, b'(']);
            for d in size {
                self.op(&[b'K', *d]);
            }
            self.op(b"t(");
            // Row-major strides.
            let mut stride = vec![1u8; size.len()];
            for i in (0..size.len().saturating_sub(1)).rev() {
                stride[i] = stride[i + 1] * size[i + 1];
            }
            for s in stride {
                self.op(&[b'K', s]);
            }
            self.op(&[b't', 0x89])
                .global("collections", "OrderedDict")
                .op(b")Rt")
                .op(b"R");
        }
    }

    fn constructed(storage_global: (&str, &str)) -> Vec<u8> {
        let mut f = File(Vec::new());
        f.op(&[0x80, 2, 0x8a, 10]).op(&MAGIC).op(b".");
        f.op(&[0x80, 2, b'M', 0xe9, 0x03, b'.']);
        f.op(&[0x80, 2, b'}', b'('])
            .s("little_endian")
            .op(&[0x88, b'u', b'.']);
        f.op(&[0x80, 2])
            .global("collections", "OrderedDict")
            .op(b")R(");
        f.tensor("a.weight", "0", &[2, 3], storage_global);
        f.tensor("a.bias", "1", &[2], storage_global);
        f.op(b"u.");
        f.op(&[0x80, 2, b']', b'(']).s("0").s("1").op(b"e.");
        for values in [&[1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0][..], &[-0.5, 0.25][..]] {
            f.op(&(values.len() as i64).to_le_bytes());
            for v in values {
                f.op(&v.to_le_bytes());
            }
        }
        f.0
    }

    #[test]
    fn a_constructed_state_dict_reads_back() {
        let w = parse(&constructed(("torch", "FloatStorage"))).expect("parse");
        assert_eq!(w.len(), 2);
        assert_eq!(
            w[0],
            (
                "a.weight".to_string(),
                vec![2, 3],
                vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
            )
        );
        assert_eq!(w[1], ("a.bias".to_string(), vec![2], vec![-0.5, 0.25]));
    }

    #[test]
    fn a_foreign_global_is_refused() {
        let e = parse(&constructed(("os", "system"))).expect_err("refused");
        assert!(e.to_string().contains("os.system"), "{e}");
    }

    #[test]
    fn a_truncated_storage_is_refused() {
        let mut b = constructed(("torch", "FloatStorage"));
        b.truncate(b.len() - 4);
        assert!(parse(&b).is_err());
    }

    /// The captured file: `RUSTROOM_FFDNET` names `ffdnet_color.pth` (KAIR v1.0).
    #[test]
    #[ignore = "needs the downloaded model; set RUSTROOM_FFDNET"]
    fn the_real_ffdnet_file_reads() {
        let path = std::env::var("RUSTROOM_FFDNET").expect("RUSTROOM_FFDNET");
        let w = read(std::path::Path::new(&path)).expect("read");
        assert_eq!(w.len(), 24);
        let shape = |n: &str| w.iter().find(|x| x.0 == n).map(|x| x.1.clone());
        assert_eq!(shape("model.0.weight"), Some(vec![96, 13, 3, 3]));
        assert_eq!(shape("model.22.weight"), Some(vec![12, 96, 3, 3]));
        assert_eq!(shape("model.22.bias"), Some(vec![12]));
        assert_eq!(w.iter().map(|x| x.2.len()).sum::<usize>(), 852_108);
    }
}
