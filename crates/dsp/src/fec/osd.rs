//! Ordered-statistics decoding (OSD) over the LDPC generator — the "last
//! ~2 dB" after belief propagation.
//!
//! OSD-`order` reprocessing: sort variable positions by reliability `|Llr|`,
//! Gaussian-eliminate the generator columns over GF(2) to find the `k` most
//! reliable independent positions (the Most Reliable Basis), take the hard
//! decisions there as the message, re-encode, then flip up to `order` of the
//! least-reliable basis bits and keep the re-encoded codeword with minimum
//! soft (correlation) distance to the received LLRs.

use crate::types::Llr;

use super::ldpc::Ldpc;

/// OSD decode. Returns the recovered `n`-bit codeword, or `None` if no basis
/// could be formed. `order` is the reprocessing order (0, 1, 2, …).
pub fn osd_decode(code: &Ldpc, llrs: &[Llr], order: usize) -> Option<Vec<u8>> {
    let n = code.n();
    let k = code.k();
    assert_eq!(llrs.len(), n);

    // 1. Reliability order (most reliable first).
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| llrs[b].abs().partial_cmp(&llrs[a].abs()).unwrap());

    // 2. Build the generator matrix permuted into reliability order, then row-
    //    reduce to find k independent columns (the Most Reliable Basis).
    //    Work with the generator rows as a k×n bit matrix `G`; we want columns
    //    (positions) that are linearly independent. Equivalent: reduce Gᵀ.
    //
    //    Form a k×k system by greedily selecting reliable positions whose
    //    generator columns are independent, performing GF(2) elimination on the
    //    accumulated k×k matrix.
    let gen = code.generator_rows(); // k rows, each n bits

    // columns as k-bit vectors (one u128-ish via Vec<u8> of length k)
    let column = |pos: usize| -> Vec<u8> { (0..k).map(|r| gen[r][pos] & 1).collect() };

    // Gaussian elimination to pick k independent columns in reliability order.
    let mut basis_rows: Vec<Vec<u8>> = Vec::with_capacity(k); // reduced columns
    let mut basis_pos: Vec<usize> = Vec::with_capacity(k);
    let mut pivots: Vec<usize> = Vec::with_capacity(k); // pivot bit index per basis col

    for &pos in &idx {
        let mut col = column(pos);
        // reduce against existing basis
        for (bi, brow) in basis_rows.iter().enumerate() {
            if col[pivots[bi]] == 1 {
                xor_into(&mut col, brow);
            }
        }
        // find a pivot
        if let Some(piv) = col.iter().position(|&b| b == 1) {
            basis_rows.push(col);
            basis_pos.push(pos);
            pivots.push(piv);
            if basis_pos.len() == k {
                break;
            }
        }
    }
    if basis_pos.len() < k {
        return None; // generator columns not full rank over chosen set
    }

    // 3. Solve for the message that reproduces the hard decisions on the basis
    //    positions. Build the k×k matrix B (rows = basis columns of G in their
    //    original, un-reduced form) and target vector y = hard bits at basis.
    //    Solve B·mᵀ-style: actually codeword = m·G, and at basis positions
    //    cw[basis] = m · G[:,basis]. We invert G[:,basis] (k×k, invertible by
    //    construction) to get m = y · (G[:,basis])⁻¹.
    let hard: Vec<u8> = llrs.iter().map(|&l| u8::from(l < 0.0)).collect();
    // a[r][c] = G[r][basis_pos[c]]; codeword bit at basis c is Σ_r m[r]·a[r][c],
    // so y = m·a and we solve aᵀ·mᵀ = yᵀ for the message m.
    let a: Vec<Vec<u8>> = (0..k)
        .map(|r| basis_pos.iter().map(|&p| gen[r][p] & 1).collect::<Vec<u8>>())
        .collect();
    let y: Vec<u8> = basis_pos.iter().map(|&p| hard[p]).collect();
    let m0 = solve_gf2(&transpose(&a, k, k), &y)?;

    // 4. Order-0 candidate and order-`order` flips of least-reliable basis bits.
    let mut best_cw = code.reencode(&m0);
    let mut best_dist = soft_distance(&best_cw, llrs);

    // least-reliable basis positions are at the tail of basis_pos (since idx is
    // most-reliable first and we appended in that order).
    let tail_start = k.saturating_sub(flip_window(order, k));
    let flip_indices: Vec<usize> = (tail_start..k).collect();

    for combo in flip_combinations(&flip_indices, order) {
        let mut m = m0.clone();
        for &bi in &combo {
            m[bi] ^= 1;
        }
        let cw = code.reencode(&m);
        let d = soft_distance(&cw, llrs);
        if d < best_dist {
            best_dist = d;
            best_cw = cw;
        }
    }

    Some(best_cw)
}

/// How many least-reliable basis bits to consider flipping for a given order.
fn flip_window(order: usize, k: usize) -> usize {
    match order {
        0 => 0,
        _ => (order * 12).min(k), // a modest window keeps OSD-1/2 cheap
    }
}

/// Soft (correlation) distance: sum of `|Llr|` over positions where the
/// codeword bit disagrees with the LLR sign. Minimised by ML.
fn soft_distance(cw: &[u8], llrs: &[Llr]) -> f32 {
    cw.iter()
        .zip(llrs)
        .map(|(&b, &l)| {
            let hard = u8::from(l < 0.0);
            if b != hard {
                l.abs()
            } else {
                0.0
            }
        })
        .sum()
}

fn xor_into(a: &mut [u8], b: &[u8]) {
    for (x, &y) in a.iter_mut().zip(b) {
        *x ^= y;
    }
}

fn transpose(a: &[Vec<u8>], rows: usize, cols: usize) -> Vec<Vec<u8>> {
    let mut t = vec![vec![0u8; rows]; cols];
    for (r, row) in a.iter().enumerate().take(rows) {
        for (c, &v) in row.iter().enumerate().take(cols) {
            t[c][r] = v;
        }
    }
    t
}

/// Solve `M · x = b` over GF(2) for a square invertible `M` (n×n). Returns the
/// solution vector or `None` if singular.
fn solve_gf2(m: &[Vec<u8>], b: &[u8]) -> Option<Vec<u8>> {
    let n = m.len();
    let mut aug: Vec<Vec<u8>> = m
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut r = row.clone();
            r.push(b[i]);
            r
        })
        .collect();
    let mut row = 0;
    for col in 0..n {
        // find pivot
        let piv = (row..n).find(|&r| aug[r][col] == 1)?;
        aug.swap(row, piv);
        for r in 0..n {
            if r != row && aug[r][col] == 1 {
                let pivrow = aug[row].clone();
                xor_into(&mut aug[r], &pivrow);
            }
        }
        row += 1;
        if row == n {
            break;
        }
    }
    Some((0..n).map(|i| aug[i][n]).collect())
}

/// All combinations of up to `order` flip indices drawn from `pool`.
fn flip_combinations(pool: &[usize], order: usize) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    fn rec(pool: &[usize], start: usize, order: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if !cur.is_empty() {
            out.push(cur.clone());
        }
        if cur.len() == order {
            return;
        }
        for i in start..pool.len() {
            cur.push(pool[i]);
            rec(pool, i + 1, order, cur, out);
            cur.pop();
        }
    }
    if order > 0 {
        rec(pool, 0, order, &mut Vec::new(), &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Rng;

    fn msg_and_cw(code: &Ldpc, rng: &mut Rng) -> (Vec<u8>, Vec<u8>) {
        let msg: Vec<u8> = (0..code.k()).map(|_| (rng.next_u64() & 1) as u8).collect();
        let cw = code.encode(&msg);
        (msg, cw)
    }

    #[test]
    fn osd0_recovers_clean() {
        let code = Ldpc::ft8();
        let mut rng = Rng::new(11);
        let (_, cw) = msg_and_cw(&code, &mut rng);
        let llr: Vec<Llr> = cw.iter().map(|&b| if b == 0 { 3.0 } else { -3.0 }).collect();
        let out = osd_decode(&code, &llr, 0).unwrap();
        assert_eq!(out, cw);
    }

    #[test]
    fn osd_recovers_where_bp_fails() {
        let code = Ldpc::ft8();
        let mut rng = Rng::new(123);

        // OSD's strength: it only needs the k most-reliable positions to be
        // correct. We make a transmitted codeword whose low-confidence (weak)
        // positions carry the channel errors and crank the error count past
        // BP's correcting power, then show OSD re-encodes the right codeword.
        let mut found = false;
        for _ in 0..400 {
            let (_, cw) = msg_and_cw(&code, &mut rng);
            // Base confidence high everywhere so most positions are reliable.
            let mut llr: Vec<Llr> = cw
                .iter()
                .map(|&b| if b == 0 { 6.0 } else { -6.0 })
                .collect();
            // Choose a set of positions to make weak, and flip (corrupt) some
            // of them with *small* magnitude so they rank lowest in |LLR| and
            // fall outside OSD's most-reliable basis.
            let mut weak: Vec<usize> = (0..code.n()).collect();
            // deterministic shuffle
            for i in (1..weak.len()).rev() {
                let j = (rng.next_u64() as usize) % (i + 1);
                weak.swap(i, j);
            }
            let weak = &weak[..18];
            for (idx, &pos) in weak.iter().enumerate() {
                // Make weak; corrupt (flip sign) the first ~10 of them.
                let mag = 0.2f32;
                let correct = if cw[pos] == 0 { mag } else { -mag };
                llr[pos] = if idx < 10 { -correct } else { correct };
            }

            let (_, bp_errs) = code.decode_minsum(&llr, 30);
            if bp_errs == 0 {
                continue; // not a BP-failure case; keep searching
            }
            if let Some(out) = osd_decode(&code, &llr, 2) {
                if out == cw {
                    found = true;
                    break;
                }
            }
        }
        assert!(found, "no case found where OSD beats a failed BP");
    }
}
