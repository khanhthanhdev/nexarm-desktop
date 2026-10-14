//! A small multi-series line chart drawn on a GPUI canvas (joint curves, loss and learning rate),
//! with an optional vertical marker for the playhead of the episode viewer.

use gpui_kit::{Bounds, IntoElement, PathBuilder, Styled as _, canvas, fill, point, px, rgb, size};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Series {
    pub label: String,
    pub color: u32,
    pub points: Vec<(f32, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct ChartSpec {
    pub series: Vec<Series>,
    /// Fixed axis ranges; `None` fits the data.
    pub x_range: Option<(f32, f32)>,
    pub y_range: Option<(f32, f32)>,
    /// Vertical marker in x units (the playhead).
    pub marker_x: Option<f32>,
    /// Plot y on a log10 scale (learning rate, loss spanning decades). Non-positive values are dropped.
    pub log_y: bool,
}

/// Palette for up to six series (the six NexArm joints).
pub const SERIES_COLORS: [u32; 6] = [0xFA8F01, 0x4CAF50, 0x2196F3, 0xE91E63, 0x9C27B0, 0x00BCD4];

/// Keep at most `max` points by taking the min and max of each bucket, so spikes survive.
pub fn decimate(points: &[(f32, f32)], max: usize) -> Vec<(f32, f32)> {
    if max < 4 || points.len() <= max {
        return points.to_vec();
    }
    let buckets = max / 2;
    let size = points.len().div_ceil(buckets);
    let mut out = Vec::with_capacity(max + 2);
    for chunk in points.chunks(size) {
        let lo = chunk.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let hi = chunk.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        if lo.0 <= hi.0 {
            out.extend([lo, hi]);
        } else {
            out.extend([hi, lo]);
        }
    }
    out
}

fn transform_y(spec: &ChartSpec, y: f32) -> Option<f32> {
    if spec.log_y { (y > 0.0).then(|| y.log10()) } else { Some(y) }
}

/// Axis ranges after applying the log scale; falls back to a unit range for empty data.
pub fn fit_ranges(spec: &ChartSpec) -> ((f32, f32), (f32, f32)) {
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for s in &spec.series {
        for &(x, y) in &s.points {
            let Some(y) = transform_y(spec, y) else { continue };
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
    }
    let widen = |lo: f32, hi: f32| if lo > hi { (0.0, 1.0) } else if lo == hi { (lo - 0.5, hi + 0.5) } else { (lo, hi) };
    let xr = spec.x_range.unwrap_or_else(|| widen(x0, x1));
    let yr = spec
        .y_range
        .map(|(a, b)| if spec.log_y { (a.max(f32::MIN_POSITIVE).log10(), b.max(f32::MIN_POSITIVE).log10()) } else { (a, b) })
        .unwrap_or_else(|| {
            let (lo, hi) = widen(y0, y1);
            let pad = (hi - lo) * 0.05;
            (lo - pad, hi + pad)
        });
    (xr, yr)
}

/// Map a data point into a rectangle of `w` by `h` (origin top-left, y grows downward).
pub fn to_plot(spec: &ChartSpec, x: f32, y: f32, w: f32, h: f32) -> Option<(f32, f32)> {
    let ((x0, x1), (y0, y1)) = fit_ranges(spec);
    let y = transform_y(spec, y)?;
    let fx = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.5 };
    let fy = if y1 > y0 { (y - y0) / (y1 - y0) } else { 0.5 };
    Some((fx.clamp(0.0, 1.0) * w, (1.0 - fy.clamp(0.0, 1.0)) * h))
}

/// The chart element; give it a height with `.h(px(..))` on the wrapper.
pub fn line_chart(spec: ChartSpec, grid: u32, marker: u32) -> impl IntoElement {
    let spec = Arc::new(spec);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            if w <= 4.0 || h <= 4.0 {
                return;
            }
            let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
            for i in 0..=4 {
                let y = h * i as f32 / 4.0;
                window.paint_quad(fill(Bounds::new(at(0.0, y), size(px(w), px(1.0))), rgb(grid)));
            }
            for s in &spec.series {
                let points = decimate(&s.points, (w as usize * 2).max(8));
                let mut path = PathBuilder::stroke(px(1.5));
                let mut started = false;
                for &(x, y) in &points {
                    let Some((px_, py_)) = to_plot(&spec, x, y, w, h) else { continue };
                    if started {
                        path.line_to(at(px_, py_));
                    } else {
                        path.move_to(at(px_, py_));
                        started = true;
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(s.color));
                }
            }
            if let Some(mx) = spec.marker_x
                && let Some((x, _)) = marker_x(&spec, mx, w)
            {
                window.paint_quad(fill(Bounds::new(at(x, 0.0), size(px(2.0), px(h))), rgb(marker)));
            }
        },
    )
    .size_full()
}

/// x position of the marker (independent of any y value).
fn marker_x(spec: &ChartSpec, x: f32, w: f32) -> Option<(f32, f32)> {
    let ((x0, x1), _) = fit_ranges(spec);
    let fx = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.5 };
    Some((fx.clamp(0.0, 1.0) * w, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(points: &[(f32, f32)]) -> Series {
        Series { label: "s".into(), color: 0, points: points.to_vec() }
    }

    #[test]
    fn data_maps_into_the_plot_rectangle() {
        let spec = ChartSpec { series: vec![series(&[(0.0, 0.0), (10.0, 100.0)])], y_range: Some((0.0, 100.0)), ..Default::default() };
        assert_eq!(to_plot(&spec, 0.0, 0.0, 200.0, 100.0), Some((0.0, 100.0)));
        assert_eq!(to_plot(&spec, 10.0, 100.0, 200.0, 100.0), Some((200.0, 0.0)));
        assert_eq!(to_plot(&spec, 5.0, 50.0, 200.0, 100.0), Some((100.0, 50.0)));
        // outside the range clamps to the edge
        assert_eq!(to_plot(&spec, 20.0, 500.0, 200.0, 100.0), Some((200.0, 0.0)));
    }

    #[test]
    fn empty_and_flat_data_get_a_usable_range() {
        let empty = ChartSpec::default();
        let (xr, (ylo, yhi)) = fit_ranges(&empty);
        assert_eq!(xr, (0.0, 1.0));
        assert!(ylo <= 0.0 && yhi >= 1.0);
        let flat = ChartSpec { series: vec![series(&[(0.0, 5.0), (1.0, 5.0)])], ..Default::default() };
        let (_, (lo, hi)) = fit_ranges(&flat);
        assert!(lo < 5.0 && hi > 5.0);
    }

    #[test]
    fn log_scale_drops_non_positive_values_and_spaces_decades_evenly() {
        let spec = ChartSpec { series: vec![series(&[(0.0, 1e-5), (1.0, 1e-4), (2.0, 1e-3)])], log_y: true, ..Default::default() };
        let (_, a) = to_plot(&spec, 0.0, 1e-5, 100.0, 100.0).unwrap();
        let (_, b) = to_plot(&spec, 1.0, 1e-4, 100.0, 100.0).unwrap();
        let (_, c) = to_plot(&spec, 2.0, 1e-3, 100.0, 100.0).unwrap();
        assert!(((a - b) - (b - c)).abs() < 0.01, "{a} {b} {c}");
        assert_eq!(to_plot(&spec, 0.0, 0.0, 100.0, 100.0), None);
    }

    #[test]
    fn decimation_keeps_spikes_and_the_point_budget() {
        let mut pts: Vec<(f32, f32)> = (0..10_000).map(|i| (i as f32, 1.0)).collect();
        pts[4321].1 = 99.0;
        let out = decimate(&pts, 200);
        assert!(out.len() <= 210, "{}", out.len());
        assert!(out.iter().any(|p| p.1 == 99.0), "the spike survives");
        assert!(out.windows(2).all(|w| w[0].0 <= w[1].0), "x stays ordered");
        assert_eq!(decimate(&pts[..50], 200).len(), 50);
    }

    #[test]
    fn the_marker_position_does_not_depend_on_y() {
        let spec = ChartSpec { series: vec![series(&[(0.0, 1.0), (10.0, 2.0)])], ..Default::default() };
        assert_eq!(marker_x(&spec, 5.0, 200.0), Some((100.0, 0.0)));
    }
}
