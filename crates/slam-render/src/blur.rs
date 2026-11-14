//! A Gaussian blur of an image into a render target (ADR-0123): the image drawn across
//! into one target and down into another with `shaders/blur.wgsl`, at a size of the
//! caller's choosing (smaller than the window: the blur is cheaper and wider for it). Run
//! once per image, not per frame; the result is sampled like any texture.

use crate::atlas::UvRect;
use crate::hal::{Blend, Color, Device, DeviceError, Extent, RenderTargetDesc, TextureFormat};
use crate::shaders;
use crate::sprite::{Projection, Sprite, SpriteBatch, SpriteError, SpriteRenderer};
use crate::textures::TextureId;

/// Taps on each side of a pass's centre: the shader's.
const TAPS: f32 = 4.0;

/// Blurs images into its pair of render targets.
pub struct Blur<D: Device> {
    renderer: SpriteRenderer<D>,
    batch: SpriteBatch,
    /// The two targets and their size: across into the first, down into the second.
    targets: Option<([D::RenderTarget; 2], Extent)>,
}

/// Draws the part `uv` of `from` over the whole of `into`, blurred along `step`.
#[allow(clippy::too_many_arguments)]
fn pass<D: Device>(
    renderer: &mut SpriteRenderer<D>,
    batch: &mut SpriteBatch,
    device: &mut D,
    into: &D::RenderTarget,
    from: &D::Texture,
    uv: UvRect,
    projection: &Projection,
    step: [f32; 2],
) -> Result<(), SpriteError> {
    let size = device.begin_pass(Some(into), Some(Color::TRANSPARENT));
    batch.clear();
    batch.push(&Sprite {
        colour: Color {
            r: step[0],
            g: step[1],
            b: 0.0,
            a: 0.0,
        },
        blend: Blend::Replace,
        ..Sprite::new(
            TextureId(0),
            uv,
            [0.0, 0.0],
            [size.width as f32, size.height as f32],
        )
    });
    renderer.draw(device, batch, projection, |_| Some(from))?;
    Ok(())
}

impl<D: Device> Blur<D> {
    pub fn new(device: &mut D) -> Result<Self, DeviceError> {
        Ok(Self {
            // Two draws a run.
            renderer: SpriteRenderer::with_shader(device, 2, &shaders::BLUR)?,
            batch: SpriteBatch::with_capacity(1),
            targets: None,
        })
    }

    /// Blurs the part `uv` of `source` into an image of `size`, about `radius` pixels of
    /// that size wide (taps up to `radius` away). Makes the targets when the size changes;
    /// leaves the drawing in the second target, so the caller switches back to its own
    /// (`Device::begin_pass`).
    pub fn run(
        &mut self,
        device: &mut D,
        source: &D::Texture,
        uv: UvRect,
        size: Extent,
        radius: f32,
    ) -> Result<(), SpriteError> {
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        if self.targets.as_ref().is_none_or(|(_, made)| *made != size) {
            if let Some((old, _)) = self.targets.take() {
                old.into_iter()
                    .for_each(|t| device.destroy_render_target(t));
            }
            let desc = RenderTargetDesc {
                size,
                format: TextureFormat::Rgba8,
            };
            let first = device.create_render_target(&desc)?;
            let second = match device.create_render_target(&desc) {
                Ok(second) => second,
                Err(e) => {
                    device.destroy_render_target(first);
                    return Err(e.into());
                }
            };
            self.targets = Some(([first, second], size));
        }
        let Self {
            renderer,
            batch,
            targets,
        } = self;
        let Some(([first, second], _)) = targets else {
            unreachable!("made above")
        };
        renderer.begin_frame();
        let projection = Projection::pixels(size);
        let spacing = radius.max(0.0) / TAPS;
        let (w, h) = (size.width as f32, size.height as f32);
        // Across: the source's texture coordinates per output pixel, times the spacing.
        let across = (uv.u1 - uv.u0) / w * spacing;
        pass(
            renderer,
            batch,
            device,
            first,
            source,
            uv,
            &projection,
            [across, 0.0],
        )?;
        // Down: the first target's, whole.
        let down = spacing / h;
        let blurred_across = device.render_target_texture(first);
        pass(
            renderer,
            batch,
            device,
            second,
            blurred_across,
            UvRect::FULL,
            &projection,
            [0.0, down],
        )
    }

    /// The blurred image of the last run, if any.
    pub fn result<'a>(&'a self, device: &D) -> Option<&'a D::Texture> {
        self.targets
            .as_ref()
            .map(|([_, second], _)| device.render_target_texture(second))
    }

    /// Frees the device resources.
    pub fn destroy(self, device: &mut D) {
        self.renderer.destroy(device);
        if let Some((targets, _)) = self.targets {
            targets
                .into_iter()
                .for_each(|t| device.destroy_render_target(t));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_device::{Call, Fake};

    #[test]
    fn targets_are_made_again_only_for_another_size() {
        let mut d = Fake::default();
        let mut blur = Blur::new(&mut d).expect("blur");
        assert!(blur.result(&d).is_none());
        let source = 7;
        blur.run(&mut d, &source, UvRect::FULL, Extent::new(64, 36), 8.0)
            .expect("run");
        let first = *blur.result(&d).expect("a result");
        // Two passes, each one quad with its own pipeline (replacing, not blending).
        let draws = d
            .log
            .iter()
            .filter(|c| matches!(c, Call::DrawIndexed(_)))
            .count();
        assert_eq!(draws, 2);
        assert!(d.log.contains(&Call::Pipeline(Blend::Replace)));
        blur.run(&mut d, &source, UvRect::FULL, Extent::new(64, 36), 8.0)
            .expect("run");
        assert_eq!(*blur.result(&d).unwrap(), first, "same size, same targets");
        blur.run(&mut d, &source, UvRect::FULL, Extent::new(32, 18), 8.0)
            .expect("run");
        assert_ne!(*blur.result(&d).unwrap(), first, "new targets");
        // An empty size draws nothing and keeps the last result.
        let before = d.log.len();
        blur.run(&mut d, &source, UvRect::FULL, Extent::new(0, 18), 8.0)
            .expect("run");
        assert_eq!(d.log.len(), before);
        blur.destroy(&mut d);
    }

    #[test]
    fn a_target_that_cannot_be_made_fails_the_run() {
        let mut d = Fake::default();
        let mut blur = Blur::new(&mut d).expect("blur");
        d.fail_after = Some(1);
        assert!(
            blur.run(&mut d, &1, UvRect::FULL, Extent::new(8, 8), 2.0)
                .is_err()
        );
        assert!(blur.result(&d).is_none());
    }
}
