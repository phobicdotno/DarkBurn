//! Binary-image helpers for tracing: boundary loops along pixel edges, connected components
//! and skeletons. All written from scratch.

/// Directions on the pixel-corner lattice (y down): right, down, left, up.
const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

/// Traces every boundary between ink and background along pixel edges. Each loop is a list
/// of lattice corners `(x, y)` with the ink on the right-hand side when walking (y down), so
/// outer boundaries run clockwise on screen and holes anticlockwise. Diagonally touching ink
/// pixels count as connected.
pub(crate) fn trace_loops(ink: &[bool], width: u32, height: u32) -> Vec<Vec<(i32, i32)>> {
    let (w, h) = (width as i32, height as i32);
    let vw = (w + 1) as usize;
    let is_ink = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && ink[(y * w + x) as usize];
    let mut out_dirs = vec![0u8; vw * (h + 1) as usize];
    let vi = |x: i32, y: i32| y as usize * vw + x as usize;
    for y in 0..h {
        for x in 0..w {
            if !ink[(y * w + x) as usize] {
                continue;
            }
            if !is_ink(x, y - 1) {
                out_dirs[vi(x, y)] |= 1 << 0;
            }
            if !is_ink(x + 1, y) {
                out_dirs[vi(x + 1, y)] |= 1 << 1;
            }
            if !is_ink(x, y + 1) {
                out_dirs[vi(x + 1, y + 1)] |= 1 << 2;
            }
            if !is_ink(x - 1, y) {
                out_dirs[vi(x, y + 1)] |= 1 << 3;
            }
        }
    }
    let mut used = vec![0u8; out_dirs.len()];
    let mut loops = Vec::new();
    for sy in 0..=h {
        for sx in 0..=w {
            let start = vi(sx, sy);
            loop {
                let free = out_dirs[start] & !used[start];
                if free == 0 {
                    break;
                }
                let mut d = free.trailing_zeros() as usize;
                let (mut x, mut y) = (sx, sy);
                let mut pts = Vec::new();
                loop {
                    let v = vi(x, y);
                    used[v] |= 1 << d;
                    pts.push((x, y));
                    x += DIRS[d].0;
                    y += DIRS[d].1;
                    let nv = vi(x, y);
                    let opts = out_dirs[nv];
                    let next = if opts.count_ones() == 2 {
                        // Saddle: turn so that diagonal ink stays connected.
                        let left = (d + 3) % 4;
                        if opts & (1 << left) != 0 { left } else { (d + 1) % 4 }
                    } else {
                        opts.trailing_zeros() as usize
                    };
                    if next >= 4 || used[nv] & (1 << next) != 0 {
                        break;
                    }
                    d = next;
                }
                loops.push(pts);
            }
        }
    }
    loops
}

/// Labels connected components of `mask` and returns (label per pixel, size per label).
/// Label 0 means "not in mask"; labels start at 1.
pub(crate) fn components(mask: &[bool], width: u32, height: u32, eight: bool) -> (Vec<u32>, Vec<usize>) {
    let (w, h) = (width as i32, height as i32);
    let mut labels = vec![0u32; mask.len()];
    let mut sizes = vec![0usize];
    let mut stack = Vec::new();
    let n4: &[(i32, i32)] = &[(1, 0), (-1, 0), (0, 1), (0, -1)];
    let n8: &[(i32, i32)] = &[(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
    let nb = if eight { n8 } else { n4 };
    for start in 0..mask.len() {
        if !mask[start] || labels[start] != 0 {
            continue;
        }
        let label = sizes.len() as u32;
        let mut size = 0;
        labels[start] = label;
        stack.push(start);
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i as i32) % w, (i as i32) / w);
            for &(dx, dy) in nb {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let j = (ny * w + nx) as usize;
                if mask[j] && labels[j] == 0 {
                    labels[j] = label;
                    stack.push(j);
                }
            }
        }
        sizes.push(size);
    }
    (labels, sizes)
}

/// Removes ink specks smaller than `min_px` (8-connected) and fills holes smaller than
/// `min_px` (4-connected, not touching the border).
pub(crate) fn remove_small(ink: &mut [bool], width: u32, height: u32, min_px: usize) {
    if min_px <= 1 {
        return;
    }
    let (labels, sizes) = components(ink, width, height, true);
    for (p, &l) in ink.iter_mut().zip(&labels) {
        if l != 0 && sizes[l as usize] < min_px {
            *p = false;
        }
    }
    let bg: Vec<bool> = ink.iter().map(|&v| !v).collect();
    let (labels, sizes) = components(&bg, width, height, false);
    let (w, h) = (width as usize, height as usize);
    let mut touches = vec![false; sizes.len()];
    for x in 0..w {
        touches[labels[x] as usize] = true;
        touches[labels[(h - 1) * w + x] as usize] = true;
    }
    for y in 0..h {
        touches[labels[y * w] as usize] = true;
        touches[labels[y * w + w - 1] as usize] = true;
    }
    for (p, &l) in ink.iter_mut().zip(&labels) {
        if l != 0 && !touches[l as usize] && sizes[l as usize] < min_px {
            *p = true;
        }
    }
}

/// Thins a binary image to one-pixel-wide lines (Zhang & Suen, "A fast parallel algorithm
/// for thinning digital patterns", CACM 1984).
pub(crate) fn thin(mask: &mut [bool], width: u32, height: u32) {
    let (w, h) = (width as i32, height as i32);
    let get = |m: &[bool], x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && m[(y * w + x) as usize];
    let mut remove = Vec::new();
    loop {
        let mut changed = false;
        for pass in 0..2 {
            remove.clear();
            for y in 0..h {
                for x in 0..w {
                    if !mask[(y * w + x) as usize] {
                        continue;
                    }
                    // P2..P9 clockwise from north.
                    let p = [
                        get(mask, x, y - 1),
                        get(mask, x + 1, y - 1),
                        get(mask, x + 1, y),
                        get(mask, x + 1, y + 1),
                        get(mask, x, y + 1),
                        get(mask, x - 1, y + 1),
                        get(mask, x - 1, y),
                        get(mask, x - 1, y - 1),
                    ];
                    let b = p.iter().filter(|&&v| v).count();
                    if !(2..=6).contains(&b) {
                        continue;
                    }
                    let a = (0..8).filter(|&i| !p[i] && p[(i + 1) % 8]).count();
                    if a != 1 {
                        continue;
                    }
                    let (n, e, s, wst) = (p[0], p[2], p[4], p[6]);
                    let ok = if pass == 0 { !(n && e && s) && !(e && s && wst) } else { !(n && e && wst) && !(n && s && wst) };
                    if ok {
                        remove.push((y * w + x) as usize);
                    }
                }
            }
            if !remove.is_empty() {
                changed = true;
                for &i in &remove {
                    mask[i] = false;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Splits a one-pixel-wide skeleton into chains of pixel centres. Chains run between end
/// points and junctions; closed rings come out as chains whose ends meet.
pub(crate) fn skeleton_chains(mask: &[bool], width: u32, height: u32) -> Vec<Vec<(i32, i32)>> {
    let (w, h) = (width as i32, height as i32);
    let on = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && mask[(y * w + x) as usize];
    // Neighbours, ignoring a diagonal when a 4-neighbour already links to it.
    let neighbours = |x: i32, y: i32| -> Vec<(i32, i32)> {
        let mut v = Vec::with_capacity(8);
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            if on(x + dx, y + dy) {
                v.push((x + dx, y + dy));
            }
        }
        for (dx, dy) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
            if on(x + dx, y + dy) && !on(x + dx, y) && !on(x, y + dy) {
                v.push((x + dx, y + dy));
            }
        }
        v
    };
    let idx = |p: (i32, i32)| (p.1 * w + p.0) as usize;
    let mut visited_pixel = vec![false; mask.len()];
    let mut visited_edge = std::collections::HashSet::new();
    let mut chains = Vec::new();
    let degree = |p: (i32, i32)| neighbours(p.0, p.1).len();
    let walk = |start: (i32, i32),
                first: (i32, i32),
                visited_pixel: &mut Vec<bool>,
                visited_edge: &mut std::collections::HashSet<(usize, usize)>|
     -> Vec<(i32, i32)> {
        let mut chain = vec![start, first];
        let key = |a: usize, b: usize| if a < b { (a, b) } else { (b, a) };
        visited_edge.insert(key(idx(start), idx(first)));
        let (mut prev, mut cur) = (start, first);
        while degree(cur) == 2 && cur != start {
            visited_pixel[idx(cur)] = true;
            let next = neighbours(cur.0, cur.1).into_iter().find(|&n| n != prev);
            let Some(next) = next else { break };
            if !visited_edge.insert(key(idx(cur), idx(next))) {
                break;
            }
            chain.push(next);
            prev = cur;
            cur = next;
        }
        chain
    };
    for y in 0..h {
        for x in 0..w {
            if !on(x, y) {
                continue;
            }
            let d = degree((x, y));
            if d == 2 {
                continue;
            }
            visited_pixel[idx((x, y))] = true;
            if d == 0 {
                chains.push(vec![(x, y)]);
                continue;
            }
            for n in neighbours(x, y) {
                let key = if idx((x, y)) < idx(n) { (idx((x, y)), idx(n)) } else { (idx(n), idx((x, y))) };
                if visited_edge.contains(&key) {
                    continue;
                }
                chains.push(walk((x, y), n, &mut visited_pixel, &mut visited_edge));
            }
        }
    }
    // Rings: only degree-2 pixels, never visited.
    for y in 0..h {
        for x in 0..w {
            if on(x, y) && !visited_pixel[idx((x, y))] {
                let n = neighbours(x, y);
                visited_pixel[idx((x, y))] = true;
                let mut chain = walk((x, y), n[0], &mut visited_pixel, &mut visited_edge);
                if chain.len() > 2 && chain.last() != Some(&(x, y)) {
                    // Close the ring explicitly.
                    chain.push((x, y));
                }
                chains.push(chain);
            }
        }
    }
    chains
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&str]) -> (Vec<bool>, u32, u32) {
        let h = rows.len() as u32;
        let w = rows[0].len() as u32;
        (rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect(), w, h)
    }

    fn signed_area(l: &[(i32, i32)]) -> i64 {
        let n = l.len();
        (0..n).map(|i| (l[i].0 * l[(i + 1) % n].1 - l[(i + 1) % n].0 * l[i].1) as i64).sum::<i64>() / 2
    }

    #[test]
    fn square_with_hole() {
        let (m, w, h) = grid(&["#####", "#...#", "#...#", "#####"]);
        let loops = trace_loops(&m, w, h);
        assert_eq!(loops.len(), 2);
        let areas: Vec<i64> = loops.iter().map(|l| signed_area(l)).collect();
        // Outer clockwise on screen (positive in y-down), hole the other way.
        assert!(areas.contains(&20) && areas.contains(&-6), "{areas:?}");
    }

    #[test]
    fn diagonal_pixels_join() {
        let (m, w, h) = grid(&["#.", ".#"]);
        let loops = trace_loops(&m, w, h);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].len(), 8);
    }

    #[test]
    fn remove_small_specks_and_holes() {
        let (mut m, w, h) = grid(&["#.......", "........", "..####..", "..#.##..", "..####..", "........"]);
        remove_small(&mut m, w, h, 2);
        assert!(!m[0]);
        assert!(m[(3 * w + 3) as usize]);
    }

    #[test]
    fn thinning_makes_a_line() {
        let (mut m, w, h) = grid(&["..........", ".########.", ".########.", ".########.", ".........."]);
        thin(&mut m, w, h);
        let count = m.iter().filter(|&&v| v).count();
        assert!((4..=8).contains(&count), "{count}");
        let chains = skeleton_chains(&m, w, h);
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].len(), count);
    }

    #[test]
    fn ring_chain() {
        let (m, w, h) = grid(&[".###.", "#...#", "#...#", "#...#", ".###."]);
        let chains = skeleton_chains(&m, w, h);
        assert_eq!(chains.len(), 1);
        let c = &chains[0];
        assert_eq!(c.first(), c.last());
        assert_eq!(c.len(), 13);
    }
}
