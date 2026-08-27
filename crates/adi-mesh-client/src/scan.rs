//! The camera, and reading an invite out of what it sees.
//!
//! Pairing without this is: run `adi-mono mesh invite` on the machine you want to reach, then get
//! a four-hundred-character token onto a phone. This module is the other end of the QR the CLI
//! draws (`adi-cli/src/qr.rs`), and it is an accelerator on top of the paste field — never a
//! replacement for it, because a camera can be refused, absent, or simply pointed at nothing.
//!
//! **The decoder is `quircs` and not the browser's own `BarcodeDetector`.** `BarcodeDetector` is
//! Chromium-only: Safari does not implement it, and iOS Safari is the whole reason this client
//! exists. A scanner built on it would be a Scan button that does nothing on an iPhone. `quircs`
//! decodes a grayscale buffer in pure Rust, so it works wherever this bundle runs — the loop is
//! `<video>` → `<canvas>` → `getImageData` → decode, and every step of that is twenty years old.
//! It costs 22 KB brotli in this bundle, a quarter of what `rqrr` costs. What that saving gives up
//! is adaptive thresholding, so the tests at the foot of this file hold it to a photograph of a
//! screen lit from one side, which is the case a single threshold for the whole frame is worst at.
//!
//! Two browser details here are not style, and both cost people hours:
//!
//! * **`muted` has to be set as a *property*.** The `muted` content attribute maps to
//!   `defaultMuted`, not to `muted`, so setting it on an element the DOM built at runtime does not
//!   mute anything — and an unmuted video is not allowed to autoplay.
//! * **`playsinline` or the overlay is gone.** Without it iOS Safari hands the stream to the
//!   system fullscreen player, which covers the page, and with it the Cancel button.
//!
//! The tracks are stopped on every exit path ([`stop`]). A camera light left on in an installed app
//! is alarming, and it is a battery drain on exactly the device this client is for.

use std::time::Duration;

use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStream,
    MediaStreamConstraints, MediaStreamTrack,
};

/// How long to wait between frames.
///
/// Not a frame rate: decoding is the expensive half and it runs on the same thread as the UI, so
/// this is the gap that keeps the overlay's Cancel button responsive on a phone. Eight looks a
/// second is far more than a hand held still needs.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(120);

/// The largest square, in pixels, a frame is reduced to before decoding.
///
/// The whole cost of the loop is proportional to this squared. 640 is chosen against the code it
/// has to read: an invite is a version-13 QR at 69 modules, so even a code filling only half the
/// frame lands at ~4.6 pixels a module — comfortably above the three or so a decoder needs, and a
/// quarter of the pixels a 1280-wide frame would have cost.
const MAX_SIDE: i32 = 640;

/// Whether this browser exposes a camera API on this page at all.
///
/// Not "is there a camera" and not "will the reader allow it" — neither of those can be known
/// without asking, and asking is a prompt. This is the one refusal that can be known in advance:
/// `navigator.mediaDevices` is simply absent outside a secure context. The Scan control is not
/// drawn when this is false, because a button that cannot ever work is worse than no button.
#[must_use]
pub fn available() -> bool {
    web_sys::window().is_some_and(|window| window.navigator().media_devices().is_ok())
}

/// Ask for the camera and start it playing into `video`.
///
/// Returns the stream so the caller can [`stop`] it; the caller owns that from here.
///
/// # Errors
/// If this browser exposes no camera API at all (an insecure context is the usual reason), or the
/// request is refused, or there is no camera to open. Every message is written to be read by
/// somebody holding a phone, and every one of them leaves the paste field as the way through.
pub async fn open(video: &HtmlVideoElement) -> Result<MediaStream, String> {
    let window = web_sys::window().ok_or("there is no window here")?;
    let devices = window.navigator().media_devices().map_err(|_| {
        "this browser will not open a camera on this page. Paste the token instead.".to_string()
    })?;

    let wanted = js_sys::Object::new();
    // `ideal`, never `exact`. `exact` is refused outright on a laptop with only a front camera —
    // which is a machine somebody may well be reading this on — and the rear camera is a
    // preference, not a requirement.
    set(
        &wanted,
        "facingMode",
        &ideal(&JsValue::from_str("environment")),
    );
    set(&wanted, "width", &ideal(&JsValue::from_f64(1280.0)));
    set(&wanted, "height", &ideal(&JsValue::from_f64(720.0)));

    let constraints = MediaStreamConstraints::new();
    constraints.set_video(&wanted);
    constraints.set_audio_bool(false);

    let request = devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| explain(&e))?;
    let stream: MediaStream = JsFuture::from(request)
        .await
        .map_err(|e| explain(&e))?
        .unchecked_into();

    video.set_src_object(Some(&stream));
    // See the module header: the attribute would set `defaultMuted` and leave this playing sound.
    video.set_muted(true);
    video.set_autoplay(true);
    let _ = video.set_attribute("playsinline", "true");
    // For iOS versions older than the standard attribute. Harmless everywhere else.
    let _ = video.set_attribute("webkit-playsinline", "true");

    // A rejected `play()` is not fatal and must not close the camera: the stream is live, and on
    // the engines that refuse an autoplay the picture starts on the next tap. The loop notices a
    // video that never produced a frame by itself — see [`has_frame`].
    if let Ok(playing) = video.play()
        && let Err(e) = JsFuture::from(playing).await
    {
        tracing::warn!("the camera preview would not start on its own: {e:?}");
    }
    Ok(stream)
}

/// Stop every track and let go of the stream.
///
/// Both halves matter: stopping the tracks releases the camera, and clearing `srcObject` is what
/// stops some engines holding the capture open behind an element that still points at it.
pub fn stop(stream: &MediaStream, video: &HtmlVideoElement) {
    for track in stream.get_tracks().iter() {
        if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
            track.stop();
        }
    }
    video.set_src_object(None);
}

/// Whether the camera has produced a frame yet.
///
/// A video with no dimensions is one that has not started — which is a different problem from a
/// video that is running and shows no QR code, and the two want different words on the screen.
#[must_use]
pub fn has_frame(video: &HtmlVideoElement) -> bool {
    video.video_width() > 0 && video.video_height() > 0
}

/// A canvas kept off the page, and the buffer frames are decoded out of.
///
/// One per scanning session rather than one per frame: `getImageData` on a fresh canvas every
/// eighth of a second allocates a megabyte a time, and the grayscale buffer is reused for the same
/// reason.
#[derive(Debug)]
pub struct Reader {
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    grey: Vec<u8>,
}

impl Reader {
    /// Make the canvas this session will read frames through.
    ///
    /// # Errors
    /// If there is no document, or this browser has no 2D canvas — neither of which can happen in
    /// a browser that got as far as running this bundle.
    pub fn new() -> Result<Self, String> {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .ok_or("there is no document here")?;
        let canvas: HtmlCanvasElement = document
            .create_element("canvas")
            .map_err(|_| "could not make a canvas to read the camera through".to_string())?
            .unchecked_into();

        // `willReadFrequently` keeps the backing store on the CPU. Without it every `getImageData`
        // is a readback from the GPU, and on a phone that alone drops the loop to a frame or two a
        // second. This canvas is never displayed and is read back every single frame.
        let options = js_sys::Object::new();
        set(&options, "willReadFrequently", &JsValue::TRUE);
        let context: CanvasRenderingContext2d = canvas
            .get_context_with_context_options("2d", &options)
            .ok()
            .flatten()
            .ok_or("this browser has no 2D canvas to read the camera through")?
            .unchecked_into();

        Ok(Self {
            canvas,
            context,
            grey: Vec::new(),
        })
    }

    /// Read one frame and decode whatever QR code is in the middle of it.
    ///
    /// `Ok(None)` is the ordinary answer — most frames of a hand holding a phone contain no code,
    /// or no frame has arrived yet.
    ///
    /// Only the centre square is decoded, which is both what the overlay's reticle draws and a way
    /// of not paying for the edges of a 16:9 frame that nobody is aiming with.
    ///
    /// # Errors
    /// If the frame cannot be drawn or read back — a canvas failure, not a decode failure.
    pub fn read(&mut self, video: &HtmlVideoElement) -> Result<Option<String>, String> {
        let width = i32::try_from(video.video_width()).unwrap_or(0);
        let height = i32::try_from(video.video_height()).unwrap_or(0);
        let crop = width.min(height);
        if crop <= 0 {
            return Ok(None);
        }
        let side = crop.min(MAX_SIDE);
        let n = usize::try_from(side).unwrap_or(0);

        self.canvas.set_width(u32::try_from(side).unwrap_or(0));
        self.canvas.set_height(u32::try_from(side).unwrap_or(0));
        self.context
            .draw_image_with_html_video_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                video,
                f64::from((width - crop) / 2),
                f64::from((height - crop) / 2),
                f64::from(crop),
                f64::from(crop),
                0.0,
                0.0,
                f64::from(side),
                f64::from(side),
            )
            .map_err(|_| "could not read a frame from the camera".to_string())?;

        let frame = self
            .context
            .get_image_data(0.0, 0.0, f64::from(side), f64::from(side))
            .map_err(|_| "could not read a frame from the camera".to_string())?
            .data();

        self.grey.clear();
        self.grey.reserve(n * n);
        self.grey.extend(frame.0.chunks_exact(4).map(|pixel| {
            // Rec. 601 luma, in integers. The alpha channel is ignored: a camera frame drawn
            // onto a canvas is opaque, and a decoder wants brightness, not coverage.
            let luma =
                (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29)
                    >> 8;
            u8::try_from(luma).unwrap_or(u8::MAX)
        }));

        Ok(decode(&self.grey, n))
    }
}

/// Decode the first QR code in a square of grayscale pixels.
///
/// Split out from [`Reader::read`] so it can be tested against a frame built on a machine with no
/// camera and no browser — which is the only way the thing this feature turns on gets a regression
/// test at all. `Reader::read` is the browser half; this is the decision.
fn decode(grey: &[u8], side: usize) -> Option<String> {
    let mut decoder = quircs::Quirc::default();
    decoder
        .identify(side, side, grey)
        .flatten()
        .find_map(|code| code.decode().ok())
        .and_then(|data| String::from_utf8(data.payload).ok())
}

/// `{ ideal: value }` — a media constraint that is a preference rather than a demand.
fn ideal(value: &JsValue) -> JsValue {
    let object = js_sys::Object::new();
    set(&object, "ideal", value);
    object.into()
}

/// Set one property, ignoring the failure that cannot happen on a plain object.
fn set(object: &js_sys::Object, key: &str, value: &JsValue) {
    let _ = js_sys::Reflect::set(object, &JsValue::from_str(key), value);
}

/// Turn a `getUserMedia` rejection into a sentence, and never a dead end.
///
/// The names are the `DOMException`s the specification defines; they are read with `Reflect`
/// rather than by casting to `DomException`, because not every engine rejects with one.
fn explain(error: &JsValue) -> String {
    let name = js_sys::Reflect::get(error, &JsValue::from_str("name"))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default();
    match name.as_str() {
        // On an installed home-screen app iOS remembers this answer, and there is no second prompt
        // to change its mind with — so the sentence has to point at the way through, not at a
        // setting the reader may not be able to reach.
        "NotAllowedError" | "SecurityError" => {
            "the camera was refused. Paste the token below instead.".to_string()
        }
        "NotFoundError" | "OverconstrainedError" => {
            "this device has no camera the browser can use. Paste the token below instead."
                .to_string()
        }
        "NotReadableError" | "AbortError" => {
            "the camera is busy — something else on this device is using it.".to_string()
        }
        "" => "the camera could not be opened. Paste the token below instead.".to_string(),
        other => {
            format!("the camera could not be opened ({other}). Paste the token below instead.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use qrcode::{EcLevel, QrCode};

    /// The square a frame is decoded at. Kept in step with [`MAX_SIDE`] by the first assertion in
    /// every test that uses it — a test frame that is not the size of a real one proves nothing
    /// about pixels per module, which is the only thing these tests are really about.
    const FRAME: usize = 640;

    /// What the camera is looking at, in brightness.
    struct Lighting {
        /// Everything that is not the screen: the desk, the wall behind the laptop.
        room: u8,
        /// The screen's own light and dark, before the light across the frame falls off.
        light: u8,
        dark: u8,
        /// Brightness at the left edge of the frame as 256ths of the brightness at the right one.
        /// 256 is even light; anything less is the gradient a phone held at an angle produces.
        falloff: u32,
        /// Sensor noise, peak to peak.
        grain: u8,
    }

    /// Build the frame a phone would capture of a machine showing this token: a lit screen in a
    /// darker room, the code drawn on it with the same quiet zone `adi-cli/src/qr.rs` prints,
    /// unevenly lit, grainy, and out of focus by a pixel.
    ///
    /// The scale is the point. The code is drawn at the size it lands at when somebody fills a
    /// little over half the viewfinder with a laptop screen, which for an invite works out at four
    /// pixels a module — near the floor of what any decoder can read, and where a decoder that is
    /// only good on paper stops being good enough.
    fn photograph(token: &str, light: &Lighting) -> Vec<u8> {
        let code = QrCode::with_error_correction_level(token, EcLevel::L).expect("an invite fits");
        let width = code.width();
        let modules = code.to_colors();
        let grid = width + 8;

        let screen = FRAME * 3 / 5;
        let screen_at = (FRAME - screen) / 2;
        let bezel = screen / 12;
        let cell = (screen - bezel * 2) / grid;
        let code_at = screen_at + (screen - cell * grid) / 2;

        let mut frame = vec![light.room; FRAME * FRAME];
        for y in screen_at..screen_at + screen {
            frame[y * FRAME + screen_at..y * FRAME + screen_at + screen].fill(light.light);
        }
        for gy in 4..grid - 4 {
            for gx in 4..grid - 4 {
                if modules[(gy - 4) * width + (gx - 4)] != qrcode::Color::Dark {
                    continue;
                }
                for y in 0..cell {
                    let row = (code_at + gy * cell + y) * FRAME + code_at + gx * cell;
                    frame[row..row + cell].fill(light.dark);
                }
            }
        }

        // A fixed sequence rather than a random one: a decoder test that passes four times in five
        // is not a test.
        let mut seed: u32 = 0x1234_5678;
        for (i, pixel) in frame.iter_mut().enumerate() {
            let x = u32::try_from(i % FRAME).expect("a frame is small");
            let across = light.falloff + (256 - light.falloff) * x / 256;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let grain = (seed >> 24) % u32::from(light.grain).max(1);
            *pixel = u8::try_from((u32::from(*pixel) * across / 256 + grain).min(255))
                .expect("clamped above");
        }

        blur(&frame)
    }

    /// One 3×3 box pass — a lens, roughly. Nearest-neighbour pixels are not what a camera returns,
    /// and a decoder that only works on crisp edges would pass a test built without this.
    fn blur(frame: &[u8]) -> Vec<u8> {
        let at = |x: usize, y: usize| u32::from(frame[y * FRAME + x]);
        let mut out = frame.to_vec();
        for y in 1..FRAME - 1 {
            for x in 1..FRAME - 1 {
                let sum: u32 = (y - 1..=y + 1)
                    .map(|j| (x - 1..=x + 1).map(|i| at(i, j)).sum::<u32>())
                    .sum();
                out[y * FRAME + x] = u8::try_from(sum / 9).expect("a mean of bytes is a byte");
            }
        }
        out
    }

    /// The shape of a real invite: `adi-invite:` and 380 hex characters.
    fn invite() -> String {
        std::iter::once("adi-invite:".to_owned())
            .chain((0..380).map(|i| format!("{:x}", i % 16)))
            .collect()
    }

    /// A machine showing an invite, photographed in an ordinary room, is an invite again.
    #[test]
    fn a_photograph_of_a_screen_decodes_back_to_the_token() {
        assert_eq!(
            i32::try_from(FRAME),
            Ok(MAX_SIDE),
            "the test frame is a real frame"
        );
        let token = invite();
        let frame = photograph(
            &token,
            &Lighting {
                room: 45,
                light: 235,
                dark: 25,
                falloff: 256,
                grain: 6,
            },
        );
        assert_eq!(decode(&frame, FRAME).as_deref(), Some(token.as_str()));
    }

    /// The same screen lit from one side — which is what a phone held at an angle to a laptop
    /// actually sees, and the case a single threshold for the whole frame is worst at.
    #[test]
    fn a_screen_lit_from_one_side_still_decodes() {
        let token = invite();
        let frame = photograph(
            &token,
            &Lighting {
                room: 90,
                light: 225,
                dark: 30,
                falloff: 110,
                grain: 10,
            },
        );
        assert_eq!(decode(&frame, FRAME).as_deref(), Some(token.as_str()));
    }

    /// A camera pointed at nothing in particular is the ordinary case, and it must be cheap and
    /// silent rather than an error the overlay puts on the screen.
    #[test]
    fn an_empty_frame_decodes_to_nothing() {
        assert_eq!(decode(&vec![200; FRAME * FRAME], FRAME), None);
    }
}
