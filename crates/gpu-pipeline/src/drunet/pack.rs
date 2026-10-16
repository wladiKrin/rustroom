//! Each weight repacked into the product's `[K][N]` layout — the kernel reads row `k`, column `n` at
//! `k · N + n`. Pure: no `wgpu`.

use crate::drunet::arch::{Mode, Tensor};

/// `t` as the product's `[K][N]` matrix for `mode`. Conv3 and Down2 (`W[co][ci][ky][kx]`): row
/// `k = ci·taps + tap`, column `co`. Up2 (`W[ci][co][ky][kx]`, ONNX's ConvTranspose): row `ci`, column
/// `(ky·2 + kx)·cout + co` — the four output quadrants side by side.
pub(crate) fn pack(mode: Mode, t: &Tensor) -> Vec<f32> {
    let s = &t.shape;
    match mode {
        Mode::Conv3 | Mode::Down2 => {
            let (n, k) = (s[0], s[1] * s[2] * s[3]);
            let mut b = vec![0.0; k * n];
            for co in 0..n {
                for kk in 0..k {
                    b[kk * n + co] = t.data[co * k + kk];
                }
            }
            b
        }
        Mode::Up2 => {
            let (cin, cout) = (s[0], s[1]);
            let n = 4 * cout;
            let mut b = vec![0.0; cin * n];
            for ci in 0..cin {
                for co in 0..cout {
                    for q in 0..4 {
                        b[ci * n + q * cout + co] = t.data[(ci * cout + co) * 4 + q];
                    }
                }
            }
            b
        }
        // ⚠ Winograd's weights are transformed (`winograd::transform_weights`), never packed.
        Mode::Product => unreachable!("Winograd's product has no packing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drunet::arch::{Mode, Tensor};

    /// One value per index rule, each landing where the product's row and column say.
    #[test]
    fn packing_puts_each_weight_where_the_product_reads_it() {
        let conv = Tensor {
            shape: vec![2, 3, 3, 3],
            data: (0..54).map(|i| i as f32).collect(),
        };
        let b = pack(Mode::Conv3, &conv); // K = 27, N = 2
                                          // W[co=1][ci=2][ky=1][kx=0] = 27 + 18 + 3 + 0 = 48 → row k = 2·9 + 3 + 0 = 21, column 1
        assert_eq!(b[21 * 2 + 1], 48.0);
        let down = Tensor {
            shape: vec![2, 3, 2, 2],
            data: (0..24).map(|i| i as f32).collect(),
        };
        let b = pack(Mode::Down2, &down); // K = 12, N = 2
                                          // W[co=1][ci=2][ky=1][kx=1] = 12 + 8 + 2 + 1 = 23 → row k = 2·4 + 2 + 1 = 11
        assert_eq!(b[11 * 2 + 1], 23.0);
        let up = Tensor {
            shape: vec![3, 2, 2, 2],
            data: (0..24).map(|i| i as f32).collect(),
        };
        let b = pack(Mode::Up2, &up); // K = 3, N = 8
                                      // W[ci=2][co=1][ky=1][kx=0] = 16 + 4 + 2 + 0 = 22 → column n = (1·2 + 0)·2 + 1 = 5
        assert_eq!(b[2 * 8 + 5], 22.0);
    }
}
