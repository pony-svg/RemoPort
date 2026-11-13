//! Display state, scaling, and window placement for a remote session.
//!
//! Encapsulates desktop geometry, resolution options, resize negotiation,
//! and coordinate translations between local window points and remote pixels.

use std::time::{Duration, Instant};

use emath::{pos2, Pos2, Rect, Vec2};
use remoport_rdp::{Command, Rect as RemoteRect};

use crate::domain::ResolutionOption;
use crate::logging;

/// Time without a window size change before the PC is asked to match it.
pub const RESIZE_SETTLE: Duration = Duration::from_millis(400);

/// Limits on desktop size accepted from the PC and asked of it.
pub const MIN_SIDE: u16 = 200;
pub const MAX_SIDE: u16 = 8192;

/// How the picture sits in the space it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// The desktop follows the window: until the PC catches up, the picture is
    /// shown at its own size or smaller.
    Following,
    /// The desktop's size is fixed: larger or smaller, it fills the window.
    Fitted,
    /// At its own size, moved this far along a side that does not fit.
    Scrolled(Vec2),
}

/// The 11 fields of size, scale, and layout tracking for an active session.
#[derive(Debug)]
pub struct DisplayState {
    /// Main window drawing area in physical pixels.
    pub canvas_px: (u16, u16),
    /// Scale factor of the local main window as a percentage (e.g. 100, 125, 200).
    pub screen_scale: u32,
    /// Scale factor last sent to the remote PC.
    pub sent_scale: u32,
    /// Size target waiting for resize settle time.
    pub pending_size: Option<((u16, u16), Instant)>,
    /// Size target already requested from the PC.
    pub requested_size: Option<(u16, u16)>,
    /// Whether the server confirmed Display Control channel support.
    pub display_control: bool,
    /// Number of windows/screens spanned by the session.
    pub screens: usize,
    /// Physical pixel sizes of each screen window.
    pub screen_px: Vec<(u16, u16)>,
    /// Remote monitor layout reported by the PC.
    pub remote_monitors: Vec<RemoteRect>,
    /// Multi-monitor layout requested from the PC.
    pub requested_layout: Option<Vec<(u16, u16)>>,
    /// Multi-monitor layout waiting for resize settle time.
    pub pending_layout: Option<(Vec<(u16, u16)>, Instant)>,
}

impl DisplayState {
    /// Creates a new display state for a session spanning `screens` windows.
    #[must_use]
    pub fn new(screens: usize) -> Self {
        Self {
            canvas_px: (1280, 720),
            screen_scale: 100,
            sent_scale: 100,
            pending_size: None,
            requested_size: None,
            display_control: false,
            screens,
            screen_px: vec![(1280, 720); screens],
            remote_monitors: Vec::new(),
            requested_layout: None,
            pending_layout: None,
        }
    }

    /// Clears layout and resize requests upon disconnect or layout change.
    pub fn reset(&mut self) {
        self.remote_monitors.clear();
        self.requested_layout = None;
        self.pending_layout = None;
        self.requested_size = None;
        self.pending_size = None;
    }

    /// The desktop size to ask for: fixed size if configured, or measured size.
    #[must_use]
    pub fn wanted_size(&self, measured: (u16, u16), fixed_size: Option<(u16, u16)>) -> (u16, u16) {
        desktop_size(fixed_size.unwrap_or(measured))
    }

    /// The scale for the PC: user-selected scale or automatic scale from local screen.
    #[must_use]
    pub fn wanted_scale(
        &self,
        scale_percent: Option<u16>,
        resolution: &ResolutionOption,
        hidpi: bool,
    ) -> u32 {
        scale_percent.map_or_else(
            || resolution.automatic_scale(hidpi, self.screen_scale),
            u32::from,
        )
    }

    /// The part of the remote desktop window `index` shows.
    #[must_use]
    pub fn region(
        &self,
        index: usize,
        desktop_width: usize,
        desktop_height: usize,
    ) -> Option<RemoteRect> {
        let width = u16::try_from(desktop_width).unwrap_or(u16::MAX);
        let height = u16::try_from(desktop_height).unwrap_or(u16::MAX);
        Some(screen_region(
            (width, height),
            &self.remote_monitors,
            self.requested_layout.as_deref(),
            self.screens,
            index,
        ))
    }

    /// Checks if a resize command should be sent for single-monitor canvas.
    pub fn follow_canvas(
        &mut self,
        screen_size: Option<(usize, usize)>,
        target: (u16, u16),
        scale: u32,
        now: Instant,
    ) -> Option<Command> {
        if !self.display_control {
            return None;
        }
        let (screen_w, screen_h) = screen_size?;
        let close_enough = usize::from(target.0).abs_diff(screen_w) <= 8
            && usize::from(target.1).abs_diff(screen_h) <= 8;
        if scale == self.sent_scale && (close_enough || self.requested_size == Some(target)) {
            self.pending_size = None;
            return None;
        }
        match self.pending_size {
            Some((size, since)) if size == target => {
                if now.duration_since(since) >= RESIZE_SETTLE {
                    logging::event(format_args!(
                        "Asked the PC for {}×{} at {scale}%",
                        target.0, target.1
                    ));
                    self.requested_size = Some(target);
                    self.sent_scale = scale;
                    self.pending_size = None;
                    Some(Command::Resize {
                        width: target.0,
                        height: target.1,
                        scale,
                    })
                } else {
                    None
                }
            }
            _ => {
                self.pending_size = Some((target, now));
                None
            }
        }
    }

    /// Checks if a multi-monitor layout command should be sent.
    pub fn follow_layout(
        &mut self,
        has_screen: bool,
        target: Vec<(u16, u16)>,
        scale: u32,
        now: Instant,
    ) -> Option<Command> {
        if !self.display_control || !has_screen {
            return None;
        }
        let close_enough = self.remote_monitors.len() == target.len()
            && self
                .remote_monitors
                .iter()
                .zip(&target)
                .all(|(m, &(w, h))| m.width.abs_diff(w) <= 8 && m.height.abs_diff(h) <= 8);
        if scale == self.sent_scale
            && (close_enough || self.requested_layout.as_ref() == Some(&target))
        {
            self.pending_layout = None;
            return None;
        }
        match &self.pending_layout {
            Some((layout, since)) if *layout == target => {
                if now.duration_since(*since) >= RESIZE_SETTLE {
                    let screens = layout
                        .iter()
                        .map(|&(w, h)| format!("{w}×{h}"))
                        .collect::<Vec<_>>()
                        .join(" + ");
                    logging::event(format_args!(
                        "Asked the PC for {screens} at {scale}% across {} screens",
                        layout.len()
                    ));
                    self.requested_layout = Some(target.clone());
                    self.sent_scale = scale;
                    self.pending_layout = None;
                    Some(Command::Layout {
                        sizes: target,
                        scale,
                    })
                } else {
                    None
                }
            }
            _ => {
                self.pending_layout = Some((target, now));
                None
            }
        }
    }
}

/// Clamps desktop dimensions and ensures width is an even number.
#[must_use]
pub fn desktop_size((width, height): (u16, u16)) -> (u16, u16) {
    (
        width.clamp(MIN_SIDE, MAX_SIDE) & !1,
        height.clamp(MIN_SIDE, MAX_SIDE),
    )
}

/// Whether a picture fits the window, to within a pixel, with no scroll bars.
#[must_use]
pub fn fits_window(size: Vec2, room: Vec2) -> bool {
    size.x < room.x + 1.0 && size.y < room.y + 1.0
}

/// Where the picture goes: centered along any side that fits, keeping its shape.
#[must_use]
pub fn picture_rect(area: Rect, size: Vec2, placement: Placement) -> Rect {
    let fit = (area.width() / size.x).min(area.height() / size.y);
    let (scale, scrolled) = match placement {
        Placement::Following => (fit.min(1.0), Vec2::ZERO),
        Placement::Fitted => (fit, Vec2::ZERO),
        Placement::Scrolled(by) => (1.0, by),
    };
    let shown = size * scale;
    let start = |low: f32, room: f32, side: f32, by: f32| {
        if side > room {
            low - by
        } else {
            low + (room - side) / 2.0
        }
    };
    Rect::from_min_size(
        pos2(
            start(area.min.x, area.width(), shown.x, scrolled.x),
            start(area.min.y, area.height(), shown.y, scrolled.y),
        ),
        shown,
    )
}

/// The pixel of the remote desktop under `pos`, on a picture of `region`
/// shown at `image`.
#[must_use]
pub fn remote_point(pos: Pos2, image: Rect, region: RemoteRect) -> Option<(u16, u16)> {
    if !image.contains(pos) {
        return None;
    }
    let (width, height) = (f32::from(region.width), f32::from(region.height));
    let x = (pos.x - image.min.x) / image.width() * width;
    let y = (pos.y - image.min.y) / image.height() * height;
    Some((
        region.x.saturating_add(to_u16(x.min(width - 1.0))),
        region.y.saturating_add(to_u16(y.min(height - 1.0))),
    ))
}

/// Where the PC's pointer picture goes for a pointer at `pos`: its hotspot on
/// that spot, in a picture scaled by `points_per_pixel` like the desktop's.
#[must_use]
pub fn pointer_rect(pos: Pos2, size: Vec2, hotspot: Vec2, points_per_pixel: f32) -> Rect {
    Rect::from_min_size(pos - hotspot * points_per_pixel, size * points_per_pixel)
}

/// Window `index`'s part of a desktop of `size`: where the PC says that
/// screen lies when it said so for every screen; else the sizes asked for,
/// side by side, once the desktop has taken them; else equal slices.
#[must_use]
pub fn screen_region(
    size: (u16, u16),
    reported: &[RemoteRect],
    asked: Option<&[(u16, u16)]>,
    screens: usize,
    index: usize,
) -> RemoteRect {
    let (width, height) = size;
    let whole = RemoteRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    if screens <= 1 {
        return whole;
    }
    let fits = |r: &RemoteRect| {
        u32::from(r.x) + u32::from(r.width) <= u32::from(width)
            && u32::from(r.y) + u32::from(r.height) <= u32::from(height)
    };
    if reported.len() == screens && reported.iter().all(fits) {
        return reported[index];
    }
    if let Some(asked) = asked.filter(|asked| asked.len() == screens) {
        let total: u32 = asked.iter().map(|&(w, _)| u32::from(w)).sum();
        if total == u32::from(width) {
            let x: u32 = asked[..index].iter().map(|&(w, _)| u32::from(w)).sum();
            let (w, h) = asked[index];
            return RemoteRect {
                x: u16::try_from(x).unwrap_or(u16::MAX),
                y: 0,
                width: w,
                height: h.min(height),
            };
        }
    }
    let count = u16::try_from(screens).unwrap_or(1);
    let slice = width / count;
    let x = slice * u16::try_from(index).unwrap_or(0);
    RemoteRect {
        x,
        y: 0,
        width: if index + 1 == screens {
            width - x
        } else {
            slice
        },
        height,
    }
}

/// Rounds and clamps to the `u16` range, so the cast cannot wrap.
#[must_use]
pub fn to_u16(value: f32) -> u16 {
    value.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use emath::vec2;

    #[test]
    fn desktop_sizes_are_even_and_within_limits() {
        assert_eq!(desktop_size((1920, 1080)), (1920, 1080));
        assert_eq!(desktop_size((1921, 1080)), (1920, 1080));
        assert_eq!(desktop_size((100, 100)), (MIN_SIDE, MIN_SIDE));
        assert_eq!(desktop_size((9000, 9000)), (MAX_SIDE, MAX_SIDE));
    }

    #[test]
    fn a_picture_that_fits_the_window_needs_no_scroll_bars() {
        let room = vec2(1536.0, 960.0);
        assert!(fits_window(room, room));
        assert!(fits_window(vec2(1536.4, 960.4), room));
        assert!(fits_window(vec2(800.0, 600.0), room));
        assert!(!fits_window(vec2(1537.0, 960.0), room));
        assert!(!fits_window(vec2(1536.0, 961.0), room));
    }

    #[test]
    fn the_picture_sits_as_the_options_say() {
        let area = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0));

        // Following the window: smaller if it must, never larger.
        let small = picture_rect(area, vec2(400.0, 300.0), Placement::Following);
        assert_eq!(small.size(), vec2(400.0, 300.0));
        assert_eq!(small.center(), area.center());
        assert_eq!(
            picture_rect(area, vec2(1600.0, 1200.0), Placement::Following),
            area
        );

        // A fixed size fills the window either way, keeping its shape.
        assert_eq!(
            picture_rect(area, vec2(400.0, 300.0), Placement::Fitted),
            area
        );
        let wide = picture_rect(area, vec2(1600.0, 900.0), Placement::Fitted);
        assert_eq!(wide.size(), vec2(800.0, 450.0));
        assert_eq!(wide.center(), area.center());

        // Without fitting: its own size, scrolled only along the side that does not fit.
        let scrolled = picture_rect(
            area,
            vec2(1600.0, 300.0),
            Placement::Scrolled(vec2(100.0, 50.0)),
        );
        assert_eq!(scrolled.size(), vec2(1600.0, 300.0));
        assert_eq!(scrolled.min, pos2(10.0 - 100.0, area.center().y - 150.0));
    }

    #[test]
    fn a_scrolled_picture_still_points_at_the_right_pixel() {
        let region = RemoteRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1200,
        };
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 480.0));
        let image = picture_rect(
            area,
            vec2(1536.0, 960.0),
            Placement::Scrolled(vec2(200.0, 100.0)),
        );
        assert_eq!(
            remote_point(pos2(0.0, 0.0), image, region),
            Some((250, 125))
        );
        assert_eq!(remote_point(pos2(1400.0, 0.0), image, region), None);
    }

    #[test]
    fn the_pointer_picture_lands_on_its_hotspot() {
        let rect = pointer_rect(pos2(100.0, 50.0), vec2(32.0, 32.0), vec2(4.0, 8.0), 0.5);
        assert_eq!(rect.min, pos2(98.0, 46.0));
        assert_eq!(rect.size(), vec2(16.0, 16.0));
    }

    #[test]
    fn each_window_shows_its_own_screen() {
        let rect = |x, width, height| RemoteRect {
            x,
            y: 0,
            width,
            height,
        };
        assert_eq!(
            screen_region((1280, 720), &[], None, 1, 0),
            rect(0, 1280, 720)
        );
        let reported = [rect(0, 1920, 1080), rect(1920, 1280, 1024)];
        assert_eq!(
            screen_region((3200, 1080), &reported, None, 2, 0),
            reported[0]
        );
        assert_eq!(
            screen_region((3200, 1080), &reported, None, 2, 1),
            reported[1]
        );
    }

    #[test]
    fn display_state_follow_canvas_settling() {
        let mut display = DisplayState::new(1);
        display.display_control = true;
        let t0 = Instant::now();
        let target = (1280, 720);

        // First call sets pending_size
        let cmd = display.follow_canvas(Some((1280, 720)), target, 125, t0);
        assert!(cmd.is_none());
        assert_eq!(display.sent_scale, 100);

        // Before settle: still None
        let cmd = display.follow_canvas(
            Some((1280, 720)),
            target,
            125,
            t0 + Duration::from_millis(200),
        );
        assert!(cmd.is_none());

        // After settle: sends Command::Resize
        let cmd = display.follow_canvas(Some((1280, 720)), target, 125, t0 + RESIZE_SETTLE);
        assert_eq!(
            cmd,
            Some(Command::Resize {
                width: 1280,
                height: 720,
                scale: 125,
            })
        );
        assert_eq!(display.sent_scale, 125);
    }

    #[test]
    fn display_state_follow_layout_settling() {
        let mut display = DisplayState::new(2);
        display.display_control = true;
        let t0 = Instant::now();
        let target = vec![(640, 720), (640, 720)];

        display.requested_layout = Some(target.clone());
        let cmd = display.follow_layout(true, target.clone(), 150, t0);
        assert!(cmd.is_none());

        let cmd = display.follow_layout(true, target.clone(), 150, t0 + RESIZE_SETTLE);
        assert_eq!(
            cmd,
            Some(Command::Layout {
                sizes: target,
                scale: 150,
            })
        );
        assert_eq!(display.sent_scale, 150);
    }
}
