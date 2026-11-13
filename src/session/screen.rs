//! Remote desktop screen buffer, texture upload, and remote pointer state.
//!
//! Translates BGRA frame updates from the RDP engine into RGBA pixel buffers
//! and egui textures, and holds the current remote cursor bitmap.

use egui::{vec2, ColorImage, Context, TextureHandle, TextureOptions};
use remoport_rdp::{PointerShape, Rect as RemoteRect, RemotePointer};

/// What the pointer looks like over the remote desktop.
pub enum PointerLook {
    /// This computer's own arrow.
    System,
    Hidden,
    /// The PC's picture, with its size and hotspot in the PC's pixels.
    Drawn {
        texture: TextureHandle,
        size: egui::Vec2,
        hotspot: egui::Vec2,
    },
}

impl PointerLook {
    /// Loads a pointer bitmap into an egui texture.
    pub fn load(ctx: &Context, shape: &PointerShape) -> Self {
        let size = [usize::from(shape.width), usize::from(shape.height)];
        let image = ColorImage::from_rgba_unmultiplied(size, &shape.rgba);
        Self::Drawn {
            texture: ctx.load_texture("remote-pointer", image, TextureOptions::LINEAR),
            size: vec2(f32::from(shape.width), f32::from(shape.height)),
            hotspot: vec2(f32::from(shape.hotspot_x), f32::from(shape.hotspot_y)),
        }
    }
}

/// The remote desktop as last drawn, in RGBA, and its texture.
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    texture: Option<TextureHandle>,
}

impl Screen {
    /// Creates an opaque black screen buffer of the given dimensions.
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        let (width, height) = (usize::from(width), usize::from(height));
        let mut rgba = vec![0; width * height * 4];
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Self {
            width,
            height,
            rgba,
            texture: None,
        }
    }

    /// Copies a BGRA region from the engine into the screen and its texture.
    /// A region that does not fit the screen is dropped.
    pub fn apply(&mut self, rect: RemoteRect, bgra: &[u8]) {
        let (x, y) = (usize::from(rect.x), usize::from(rect.y));
        let (w, h) = (usize::from(rect.width), usize::from(rect.height));
        if w == 0 || h == 0 || x + w > self.width || y + h > self.height || bgra.len() < w * h * 4 {
            return;
        }
        let region: Vec<u8> = bgra[..w * h * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[2], px[1], px[0], 255])
            .collect();
        for row in 0..h {
            let dst = ((y + row) * self.width + x) * 4;
            self.rgba[dst..dst + w * 4].copy_from_slice(&region[row * w * 4..(row + 1) * w * 4]);
        }
        if let Some(texture) = &mut self.texture {
            texture.set_partial(
                [x, y],
                ColorImage::from_rgba_unmultiplied([w, h], &region),
                TextureOptions::LINEAR,
            );
        }
    }

    /// Returns the egui texture, uploading the full buffer if not yet loaded.
    pub fn texture(&mut self, ctx: &Context) -> &TextureHandle {
        let (width, height, rgba) = (self.width, self.height, &self.rgba);
        self.texture.get_or_insert_with(|| {
            ctx.load_texture(
                "remote-screen",
                ColorImage::from_rgba_unmultiplied([width, height], rgba),
                TextureOptions::LINEAR,
            )
        })
    }
}

/// Groups the remote desktop screen buffer, graphics mode label, and pointer state.
pub struct ScreenState {
    /// Current screen buffer, if connected and sized.
    pub screen: Option<Screen>,
    /// Graphics codec/pipeline description reported by the engine.
    pub graphics: Option<String>,
    /// What the pointer looks like over the remote desktop.
    pub pointer: PointerLook,
    /// A new picture for the pointer, pending texture upload.
    pub new_pointer: Option<PointerShape>,
}

impl Default for ScreenState {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenState {
    /// Creates an empty screen state with the system cursor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            screen: None,
            graphics: None,
            pointer: PointerLook::System,
            new_pointer: None,
        }
    }

    /// Clears the screen buffer and resets the pointer to the system arrow.
    pub fn reset(&mut self) {
        self.screen = None;
        self.pointer = PointerLook::System;
        self.new_pointer = None;
    }

    /// Allocates a new screen buffer of the given dimensions.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.screen = Some(Screen::new(width, height));
    }

    /// Applies a BGRA region update to the current screen if present.
    pub fn apply_frame(&mut self, rect: RemoteRect, bgra: &[u8]) {
        if let Some(screen) = &mut self.screen {
            screen.apply(rect, bgra);
        }
    }

    /// Updates the remote pointer appearance based on an engine event.
    pub fn set_pointer(&mut self, pointer: RemotePointer) {
        match pointer {
            RemotePointer::Default => {
                self.pointer = PointerLook::System;
                self.new_pointer = None;
            }
            RemotePointer::Hidden => {
                self.pointer = PointerLook::Hidden;
                self.new_pointer = None;
            }
            RemotePointer::Shape(shape) => {
                self.new_pointer = Some(shape);
            }
        }
    }

    /// Uploads any pending pointer shape into a texture for this UI frame.
    pub fn flush_pointer(&mut self, ctx: &Context) {
        if let Some(shape) = self.new_pointer.take() {
            self.pointer = PointerLook::load(ctx, &shape);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_land_in_place_as_rgba() {
        let mut screen = Screen::new(4, 3);
        // A 2×1 BGRA region at (1, 2): blue then red.
        screen.apply(
            RemoteRect {
                x: 1,
                y: 2,
                width: 2,
                height: 1,
            },
            &[255, 0, 0, 0, 0, 0, 255, 0],
        );
        let at = |x: usize, y: usize| &screen.rgba[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(1, 2), &[0, 0, 255, 255]);
        assert_eq!(at(2, 2), &[255, 0, 0, 255]);
        assert_eq!(at(0, 0), &[0, 0, 0, 255]);
    }

    #[test]
    fn a_region_outside_the_screen_is_dropped() {
        let mut screen = Screen::new(4, 3);
        let before = screen.rgba.clone();
        screen.apply(
            RemoteRect {
                x: 3,
                y: 0,
                width: 2,
                height: 1,
            },
            &[1; 8],
        );
        screen.apply(
            RemoteRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            &[1; 4],
        );
        assert_eq!(screen.rgba, before);
    }

    #[test]
    fn screen_state_tracks_pointer_and_reset() {
        let mut state = ScreenState::new();
        state.resize(800, 600);
        assert!(state.screen.is_some());

        state.set_pointer(RemotePointer::Hidden);
        assert!(matches!(state.pointer, PointerLook::Hidden));

        let shape = PointerShape {
            width: 1,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            rgba: vec![255, 255, 255, 255],
        };
        state.set_pointer(RemotePointer::Shape(shape));
        assert!(state.new_pointer.is_some());

        state.reset();
        assert!(state.screen.is_none());
        assert!(matches!(state.pointer, PointerLook::System));
        assert!(state.new_pointer.is_none());
    }
}
