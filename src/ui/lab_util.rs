//! Helpers shared by the Lab pages: engine calls off the UI thread, and video frames as GPUI images.

use crate::engine::client::EngineClient;
use gpui_kit::{App, AsyncWindowContext, Context, RenderImage, Window};
use std::sync::Arc;

/// Run blocking engine work on the background executor, then hand the result to `done` on the UI thread
/// (with the window, so inputs can be updated).
pub fn call<P: 'static, T: Send + 'static>(
    client: EngineClient,
    window: &mut Window,
    cx: &mut Context<P>,
    work: impl FnOnce(EngineClient) -> T + Send + 'static,
    done: impl FnOnce(&mut P, T, &mut Window, &mut Context<P>) + 'static,
) {
    let task = cx.background_executor().spawn(async move { work(client) });
    cx.spawn_in(window, async move |this, cx: &mut AsyncWindowContext| {
        let out = task.await;
        let _ = this.update_in(cx, |page, window, cx| done(page, out, window, cx));
    })
    .detach();
}

/// Decode a JPEG into a GPUI image (BGRA, as GPUI expects).
pub fn decode_jpeg(bytes: &[u8]) -> anyhow::Result<Arc<RenderImage>> {
    let mut rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)?.into_rgba8();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}

/// Free the GPU copy of an image that is no longer shown (scrubbing would otherwise grow without bound).
pub fn release_image(image: Option<Arc<RenderImage>>, window: &mut Window, cx: &mut App) {
    if let Some(image) = image {
        cx.drop_image(image, Some(window));
    }
}

/// `1234567` -> `1.2M`, for step counters and frame totals.
pub fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// Seconds as `m:ss`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_clocks_are_short() {
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(46_765), "46.8K");
        assert_eq!(short_count(2_500_000), "2.5M");
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(clock(-3.0), "0:00");
    }

    #[test]
    fn a_jpeg_decodes_to_a_bgra_image() {
        // 2x2 JPEG produced by the engine's cv2 encoder for a solid red image.
        let mut buf = std::io::Cursor::new(Vec::new());
        let rgb = image::RgbImage::from_pixel(8, 8, image::Rgb([255, 0, 0]));
        rgb.write_to(&mut buf, image::ImageFormat::Jpeg).unwrap();
        let img = decode_jpeg(buf.get_ref()).unwrap();
        let px = img.as_bytes(0).unwrap();
        assert!(px[2] > 200 && px[0] < 60, "BGRA: red lands in the third byte, got {:?}", &px[..4]);
    }
}
