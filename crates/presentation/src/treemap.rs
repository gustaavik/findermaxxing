//! Squarified treemap layout (Bruls, Huizing, van Wijk 2000): rectangles
//! with areas proportional to sizes, as close to square as the order allows,
//! so labels fit and sizes compare at a glance.

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn area(&self) -> f64 {
        self.width * self.height
    }

    fn shorter_side(&self) -> f64 {
        self.width.min(self.height)
    }
}

/// One rectangle per size, in the same order. Sizes should be sorted
/// largest first (the algorithm's assumption); zero sizes get empty rects.
pub(crate) fn squarify(sizes: &[f64], bounds: Rect) -> Vec<Rect> {
    let total: f64 = sizes.iter().filter(|s| **s > 0.0).sum();
    let empty = Rect { x: bounds.x, y: bounds.y, width: 0.0, height: 0.0 };
    let mut rects = vec![empty; sizes.len()];
    if total <= 0.0 || bounds.area() <= 0.0 {
        return rects;
    }
    let scale = bounds.area() / total;
    let areas: Vec<(usize, f64)> =
        sizes.iter().enumerate().filter(|(_, s)| **s > 0.0).map(|(i, s)| (i, s * scale)).collect();
    let mut free = bounds;
    let mut row: Vec<(usize, f64)> = Vec::new();
    for &item in &areas {
        let side = free.shorter_side();
        let mut extended = row.clone();
        extended.push(item);
        if row.is_empty() || worst_ratio(&extended, side) <= worst_ratio(&row, side) {
            row = extended;
        } else {
            free = place_row(&row, free, &mut rects);
            row = vec![item];
        }
    }
    if !row.is_empty() {
        place_row(&row, free, &mut rects);
    }
    rects
}

/// The worst aspect ratio in a row laid along a side of length `side`.
fn worst_ratio(row: &[(usize, f64)], side: f64) -> f64 {
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    let (min, max) = row.iter().fold((f64::MAX, 0.0_f64), |(lo, hi), (_, a)| (lo.min(*a), hi.max(*a)));
    if sum <= 0.0 || min <= 0.0 {
        return f64::MAX;
    }
    let side_squared = side * side;
    let sum_squared = sum * sum;
    (side_squared * max / sum_squared).max(sum_squared / (side_squared * min))
}

/// Lays a row along the shorter side of `free`; returns what's left.
fn place_row(row: &[(usize, f64)], free: Rect, rects: &mut [Rect]) -> Rect {
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    if free.width >= free.height {
        // A column on the left.
        let width = if free.height > 0.0 { sum / free.height } else { 0.0 };
        let mut y = free.y;
        for (index, area) in row {
            let height = if width > 0.0 { area / width } else { 0.0 };
            rects[*index] = Rect { x: free.x, y, width, height };
            y += height;
        }
        Rect { x: free.x + width, y: free.y, width: (free.width - width).max(0.0), height: free.height }
    } else {
        // A row along the top.
        let height = if free.width > 0.0 { sum / free.width } else { 0.0 };
        let mut x = free.x;
        for (index, area) in row {
            let width = if height > 0.0 { area / height } else { 0.0 };
            rects[*index] = Rect { x, y: free.y, width, height };
            x += width;
        }
        Rect { x: free.x, y: free.y + height, width: free.width, height: (free.height - height).max(0.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Rect = Rect { x: 0.0, y: 0.0, width: 600.0, height: 400.0 };

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn areas_are_proportional_and_fill_the_bounds() {
        let sizes = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rects = squarify(&sizes, BOUNDS);
        let total: f64 = sizes.iter().sum();
        for (size, rect) in sizes.iter().zip(&rects) {
            assert!(close(rect.area(), BOUNDS.area() * size / total), "{rect:?}");
            assert!(rect.x >= -1e-9 && rect.y >= -1e-9);
            assert!(rect.x + rect.width <= BOUNDS.width + 1e-6 && rect.y + rect.height <= BOUNDS.height + 1e-6);
        }
        let covered: f64 = rects.iter().map(Rect::area).sum();
        assert!(close(covered, BOUNDS.area()));
    }

    #[test]
    fn the_classic_example_stays_reasonably_square() {
        let rects = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], BOUNDS);
        let worst = rects.iter().map(|r| (r.width / r.height).max(r.height / r.width)).fold(0.0, f64::max);
        assert!(worst < 3.0, "worst aspect {worst}");
    }

    #[test]
    fn zero_sizes_and_empty_bounds_are_harmless() {
        let rects = squarify(&[5.0, 0.0], BOUNDS);
        assert!(close(rects[0].area(), BOUNDS.area()));
        assert!(close(rects[1].area(), 0.0));
        assert!(squarify(&[1.0], Rect { width: 0.0, ..BOUNDS }).iter().all(|r| close(r.area(), 0.0)));
        assert!(squarify(&[], BOUNDS).is_empty());
    }
}
