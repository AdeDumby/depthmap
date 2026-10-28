/// Calls `f` for every cell on the Bresenham line from `a` to `b` inclusive.
/// Used to join mouse positions so fast strokes never leave gaps.
pub fn line_cells(a: (i64, i64), b: (i64, i64), mut f: impl FnMut(i64, i64)) {
    let (mut x, mut y) = a;
    let dx = (b.0 - a.0).abs();
    let dy = -(b.1 - a.1).abs();
    let sx = if a.0 < b.0 { 1 } else { -1 };
    let sy = if a.1 < b.1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        f(x, y);
        if x == b.0 && y == b.1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Calls `f` for every cell of the `(2*half+1)`² square centred on `c`.
pub fn square_cells(c: (i64, i64), half: i64, mut f: impl FnMut(i64, i64)) {
    for y in c.1 - half..=c.1 + half {
        for x in c.0 - half..=c.0 + half {
            f(x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_includes_both_endpoints_and_is_connected() {
        for &(a, b) in &[
            ((0, 0), (10, 3)),
            ((5, 5), (-4, -9)),
            ((0, 0), (0, 0)),
            ((-3, 2), (-3, 20)),
            ((7, -7), (-7, 7)),
        ] {
            let mut pts = Vec::new();
            line_cells(a, b, |x, y| pts.push((x, y)));
            assert_eq!(pts.first(), Some(&a));
            assert_eq!(pts.last(), Some(&b));
            for w in pts.windows(2) {
                let (dx, dy) = ((w[1].0 - w[0].0).abs(), (w[1].1 - w[0].1).abs());
                assert!(dx <= 1 && dy <= 1 && dx + dy > 0, "gap in line {a:?}->{b:?}");
            }
            let n = ((b.0 - a.0).abs()).max((b.1 - a.1).abs()) as usize + 1;
            assert_eq!(pts.len(), n);
        }
    }

    #[test]
    fn square_covers_expected_cells() {
        let mut n = 0;
        square_cells((10, -10), 2, |_, _| n += 1);
        assert_eq!(n, 25);
    }
}
