//! Direct3D 11 backend (ADR-0024, ADR-0080). Windows only.
//!
//! Shaders are compiled from the generated HLSL with `D3DCompile` when a pipeline is
//! created. D3D11 already uses the HAL coordinates of ADR-0019 (clip-space y up, the
//! origin of the window and of render targets at the top left), so there is one vertex
//! stage per shader and nothing is flipped.
//!
//! Every buffer and texture lives in `D3D11_USAGE_DEFAULT` memory and is written with
//! `UpdateSubresource`: like `glBufferSubData`, the runtime orders the copy before
//! later draws without the caller tracking what the GPU still reads.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ops::Range;

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
    D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_FORMAT_R16_FLOAT, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R32G32_FLOAT,
    DXGI_FORMAT_R32G32B32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_SDK_COMPONENT_MISSING;
use windows::core::{Interface, PCSTR};

use crate::hal::{
    Blend, BufferDesc, BufferKind, Color, DescError, DeviceError, DrawError, Extent, IndexFormat,
    Limits, MAX_VERTEX_ATTRIBUTES, Origin, PipelineDesc, RenderTargetDesc, Shader, TextureDesc,
    TextureFormat, TextureUsage, VERTEX_ALIGNMENT, VertexFormat,
};

mod swapchain;

pub use swapchain::{NativeWindow, SwapchainSurface};

/// Texture slots: one sampler register per texture slot, and D3D11 has 16.
const TEXTURE_SLOTS: usize = D3D11_COMMONSHADER_SAMPLER_SLOT_COUNT as usize;

/// Uniform buffer slots: as on GL, 12 per stage is what GL 3.3, D3D11 and Vulkan all
/// guarantee.
const UNIFORM_SLOTS: usize = 12;

/// The window side of a [`D3d11Device`]: a swapchain, or an offscreen texture.
pub trait D3d11Surface {
    /// Called by [`crate::Device::begin_frame`] while the device holds no reference to
    /// the previous frame's view, so the surface may resize its buffers. Returns the
    /// view to draw the frame into and its size, or `None` while there is nothing to
    /// draw on (minimised). Errors are the surface's to report, from `present`.
    fn next_frame(&mut self, device: &ID3D11Device) -> Option<(ID3D11RenderTargetView, Extent)>;

    /// Shows the frame without waiting for vertical sync.
    fn present(&mut self) -> Result<(), String>;
}

/// Which adapter [`D3d11Device::new`] creates the device on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum D3d11Adapter {
    /// The default hardware adapter.
    Hardware,
    /// The WARP software rasterizer (tests, machines without a D3D11 driver).
    Warp,
}

/// Whether [`D3d11Device::new`] enables the D3D11 debug layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum D3d11Debug {
    Off,
    /// Validate calls with the debug layer and log its messages through `tracing` on
    /// `present`. Needs the Windows "Graphics Tools" feature; without it the device is
    /// created without the layer and a warning is logged. Meant for debug builds.
    On,
}

impl D3d11Debug {
    /// `On` in debug builds, `Off` in release.
    pub const fn for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::On
        } else {
            Self::Off
        }
    }
}

pub struct D3d11Texture {
    raw: ID3D11Texture2D,
    /// `Some` for `SAMPLED` textures; kept in an `Option` so binding it needs no
    /// reference count change.
    view: Option<ID3D11ShaderResourceView>,
    desc: TextureDesc,
}

impl D3d11Texture {
    pub fn desc(&self) -> &TextureDesc {
        &self.desc
    }

    /// Raw texture for tests and backend-internal code.
    #[doc(hidden)]
    pub fn raw(&self) -> &ID3D11Texture2D {
        &self.raw
    }

    fn view_ptr(&self) -> Option<*mut c_void> {
        self.view.as_ref().map(Interface::as_raw)
    }
}

pub struct D3d11Buffer {
    /// Always `Some`; an `Option` so binding it needs no reference count change.
    raw: Option<ID3D11Buffer>,
    desc: BufferDesc,
    /// Uniform buffers only: D3D11.0 updates constant buffers whole, so partial
    /// writes go through this copy of the contents.
    shadow: Option<RefCell<Box<[u8]>>>,
}

impl D3d11Buffer {
    pub fn desc(&self) -> &BufferDesc {
        &self.desc
    }

    fn interface(&self) -> &ID3D11Buffer {
        self.raw.as_ref().expect("`raw` is always `Some`")
    }

    fn ptr(&self) -> *mut c_void {
        self.interface().as_raw()
    }
}

pub struct D3d11Pipeline {
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    input_layout: ID3D11InputLayout,
    blend: ID3D11BlendState,
    stride: u32,
}

pub struct D3d11RenderTarget {
    /// Always `Some`; an `Option` so binding it needs no reference count change.
    view: Option<ID3D11RenderTargetView>,
    texture: D3d11Texture,
}

impl D3d11RenderTarget {
    pub fn size(&self) -> Extent {
        self.texture.desc.size
    }

    fn view_ptr(&self) -> *mut c_void {
        self.view
            .as_ref()
            .map_or(std::ptr::null_mut(), Interface::as_raw)
    }
}

/// What the output merger draws into, as last sent to D3D11.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    /// Nothing (also at the start of a frame, while the surface may resize).
    Nothing,
    Surface,
    /// A render target, by the raw pointer of its view. The output merger holds a
    /// reference to the bound view, so the pointer is not reused while bound.
    Target(*mut c_void),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BoundBuffer {
    raw: *mut c_void,
    size: u64,
    offset: u64,
}

/// Draw state as last sent to D3D11, so redundant calls are skipped.
struct State {
    target: Bound,
    /// The bound render target's texture view (a reference of its own: the target may
    /// be dropped while bound).
    target_texture: Option<ID3D11ShaderResourceView>,
    /// Raw pointers of the uniform buffers set per slot. D3D11 holds a reference to
    /// each bound one, so the pointers are not reused while bound.
    uniforms: [*mut c_void; UNIFORM_SLOTS],
    /// The surface's view for the current frame.
    surface: Option<ID3D11RenderTargetView>,
    surface_size: Extent,
    /// Raw pointer of the set pipeline's vertex shader, and its vertex stride.
    pipeline: Option<(*mut c_void, u32)>,
    blend: *mut c_void,
    vertex: Option<BoundBuffer>,
    /// The vertex buffer as an interface, to bind it at draw time.
    vertex_raw: Option<ID3D11Buffer>,
    index: Option<(BoundBuffer, IndexFormat)>,
    /// The vertex buffer, offset or stride changed since they were sent.
    vertex_dirty: bool,
    /// Textures set per slot. D3D11 refuses to sample a texture that is being drawn
    /// into, so a slot holding the bound target's texture is unbound in D3D11 and
    /// bound again when drawing moves elsewhere.
    textures: [Option<ID3D11ShaderResourceView>; TEXTURE_SLOTS],
}

/// Direct3D 11 device at feature level 11.0 (Shader Model 5.0). Created, used and
/// released on the draw thread.
///
/// The rasterizer state (no culling, depth clip on), the primitive topology (triangle
/// list) and one linear clamp-to-edge sampler in every sampler register of both stages
/// are set once at creation. Each uniform buffer and texture is bound to both stages.
pub struct D3d11Device<S: D3d11Surface> {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    info_queue: Option<ID3D11InfoQueue>,
    surface: S,
    limits: Limits,
    state: State,
}

impl<S: D3d11Surface> Drop for D3d11Device<S> {
    fn drop(&mut self) {
        // Release every view and binding and flush deferred destruction. The device
        // and context fields then drop before the surface, so a swapchain's buffers go
        // with the swapchain's own reference to the device, inside the surface's drop
        // and before the window it holds.
        self.state.surface = None;
        self.state.target_texture = None;
        self.state.textures = Default::default();
        self.state.vertex = None;
        self.state.vertex_raw = None;
        self.state.index = None;
        // SAFETY: the device's own immediate context.
        unsafe {
            self.context.ClearState();
            self.context.Flush();
        }
    }
}

impl<S: D3d11Surface> D3d11Device<S> {
    /// Creates the device on `adapter` and the surface from it with `surface`.
    pub fn new(
        adapter: D3d11Adapter,
        debug: D3d11Debug,
        surface: impl FnOnce(&ID3D11Device) -> Result<S, DeviceError>,
    ) -> Result<Self, DeviceError> {
        let driver = match adapter {
            D3d11Adapter::Hardware => D3D_DRIVER_TYPE_HARDWARE,
            D3d11Adapter::Warp => D3D_DRIVER_TYPE_WARP,
        };
        // Only the draw thread uses the device.
        let single = D3D11_CREATE_DEVICE_SINGLETHREADED;
        let (device, context) = match debug {
            D3d11Debug::On => match create_device(driver, single | D3D11_CREATE_DEVICE_DEBUG) {
                Err(e) if e.code() == DXGI_ERROR_SDK_COMPONENT_MISSING => {
                    tracing::warn!("D3D11 debug layer unavailable: install Graphics Tools");
                    create_device(driver, single)
                }
                result => result,
            },
            D3d11Debug::Off => create_device(driver, single),
        }
        .map_err(|e| DeviceError::Unsupported(format!("D3D11 device ({adapter:?}): {e}")))?;
        let info_queue = device.cast::<ID3D11InfoQueue>().ok();
        let surface = surface(&device)?;

        let rasterizer = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        };
        let sampler = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut rasterizer_state = None;
        let mut sampler_state = None;
        // SAFETY: the descriptions are valid and the out-pointers point to live
        // `Option`s; the states are set on the device's own immediate context.
        unsafe {
            device
                .CreateRasterizerState(&rasterizer, Some(&mut rasterizer_state))
                .map_err(backend("rasterizer state"))?;
            device
                .CreateSamplerState(&sampler, Some(&mut sampler_state))
                .map_err(backend("sampler state"))?;
            let samplers: [Option<ID3D11SamplerState>; TEXTURE_SLOTS] =
                std::array::from_fn(|_| sampler_state.clone());
            context.RSSetState(rasterizer_state.as_ref());
            context.VSSetSamplers(0, Some(&samplers));
            context.PSSetSamplers(0, Some(&samplers));
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        }
        let limits = Limits {
            max_texture_size: D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION,
            max_uniform_block_size: D3D11_REQ_CONSTANT_BUFFER_ELEMENT_COUNT * 16,
            max_uniform_buffer_slots: UNIFORM_SLOTS as u32,
            max_texture_slots: TEXTURE_SLOTS as u32,
        };
        Ok(Self {
            device,
            context,
            info_queue: (debug == D3d11Debug::On).then_some(info_queue).flatten(),
            surface,
            limits,
            state: State {
                target: Bound::Nothing,
                target_texture: None,
                uniforms: [std::ptr::null_mut(); UNIFORM_SLOTS],
                surface: None,
                surface_size: Extent::default(),
                pipeline: None,
                blend: std::ptr::null_mut(),
                vertex: None,
                vertex_raw: None,
                index: None,
                vertex_dirty: false,
                textures: Default::default(),
            },
        })
    }

    /// Adapter description for reports. Allocates.
    pub fn driver_info(&self) -> String {
        let adapter = self
            .device
            .cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice>()
            // SAFETY: plain queries on a live DXGI device and adapter.
            .and_then(|dxgi| unsafe { dxgi.GetAdapter()?.GetDesc() });
        match adapter {
            Ok(desc) => {
                let len = desc.Description.iter().position(|&c| c == 0);
                let name = String::from_utf16_lossy(&desc.Description[..len.unwrap_or(128)]);
                format!("{name}, Direct3D 11")
            }
            Err(e) => format!("unknown adapter ({e}), Direct3D 11"),
        }
    }

    /// The surface, e.g. to read an offscreen surface back in tests.
    pub fn surface(&self) -> &S {
        &self.surface
    }

    pub fn surface_mut(&mut self) -> &mut S {
        &mut self.surface
    }

    /// Whether the debug layer is active (asked for and installed).
    pub fn has_debug_layer(&self) -> bool {
        self.info_queue.is_some()
    }

    /// Raw device for tests and backend-internal code.
    #[doc(hidden)]
    pub fn raw_device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Copies a texture of this device back to memory, rows top first and tightly
    /// packed. Waits for the GPU; for tests and diagnostics. Allocates.
    pub fn read_texture(&mut self, texture: &ID3D11Texture2D) -> Result<Vec<u8>, DeviceError> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a live out-parameter.
        unsafe { texture.GetDesc(&mut desc) };
        let bytes_per_pixel = match desc.Format {
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => 4,
            DXGI_FORMAT_R16_FLOAT => 2,
            DXGI_FORMAT_R8_UNORM => 1,
            other => {
                return Err(DeviceError::Unsupported(format!(
                    "reading back DXGI format {}",
                    other.0
                )));
            }
        };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let row = desc.Width as usize * bytes_per_pixel;
        let mut pixels = vec![0u8; row * desc.Height as usize];
        let mut staging = None;
        // SAFETY: the staging texture matches the source in size, format and sample
        // count; the mapping is read within `RowPitch * Height` bytes and unmapped
        // before returning.
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(backend("staging texture"))?;
            let staging = staging.expect("CreateTexture2D succeeded");
            self.context.CopyResource(&staging, texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(backend("map staging texture"))?;
            for (y, out) in pixels.chunks_exact_mut(row).enumerate() {
                let src = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
                out.copy_from_slice(std::slice::from_raw_parts(src, row));
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(pixels)
    }

    /// Takes the debug layer's stored warnings and errors (empty without the layer).
    /// Allocates.
    pub fn take_debug_messages(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        self.drain_debug_messages(|severity, text| {
            if severity.0 <= D3D11_MESSAGE_SEVERITY_WARNING.0 {
                messages.push(text.to_owned());
            }
        });
        messages
    }

    fn drain_debug_messages(&mut self, mut f: impl FnMut(D3D11_MESSAGE_SEVERITY, &str)) {
        let Some(queue) = &self.info_queue else {
            return;
        };
        // SAFETY: each message is read into a buffer of the size the queue reports,
        // aligned for `D3D11_MESSAGE`; its description points inside that buffer.
        unsafe {
            for i in 0..queue.GetNumStoredMessages() {
                let mut len = 0;
                if queue.GetMessage(i, None, &mut len).is_err() {
                    continue;
                }
                let mut buffer = vec![0u64; len.div_ceil(8)];
                let message = buffer.as_mut_ptr().cast::<D3D11_MESSAGE>();
                if queue.GetMessage(i, Some(message), &mut len).is_err() {
                    continue;
                }
                let message = &*message;
                let text = std::slice::from_raw_parts(
                    message.pDescription,
                    message.DescriptionByteLength.saturating_sub(1),
                );
                f(message.Severity, &String::from_utf8_lossy(text));
            }
            queue.ClearStoredMessages();
        }
    }

    /// Binds `target` (with its `view`, and for a render target its `texture`) to the
    /// output merger unless already bound. Texture slots that hold the new target's
    /// texture are unbound in D3D11 first (it would unbind them itself, with a
    /// debug-layer warning); slots that hold the previous target's texture are bound
    /// again.
    fn bind_target(
        &mut self,
        target: Bound,
        view: Option<&Option<ID3D11RenderTargetView>>,
        texture: Option<&Option<ID3D11ShaderResourceView>>,
    ) {
        if self.state.target == target {
            return;
        }
        let new_texture = texture.and_then(|t| t.as_ref().map(Interface::as_raw));
        let old_texture = self.state.target_texture.as_ref().map(Interface::as_raw);
        let context = &self.context;
        let textures = &self.state.textures;
        // SAFETY: views and textures are live interfaces of this device.
        unsafe {
            if new_texture.is_some() {
                for (slot, bound) in textures.iter().enumerate() {
                    if bound.as_ref().map(Interface::as_raw) == new_texture {
                        let none = [None];
                        context.VSSetShaderResources(slot as u32, Some(&none));
                        context.PSSetShaderResources(slot as u32, Some(&none));
                    }
                }
            }
            match view {
                Some(view) => context.OMSetRenderTargets(Some(std::slice::from_ref(view)), None),
                None => context.OMSetRenderTargets(None, None),
            }
            if old_texture.is_some() && old_texture != new_texture {
                for (slot, bound) in textures.iter().enumerate() {
                    if bound.as_ref().map(Interface::as_raw) == old_texture {
                        let views = std::slice::from_ref(bound);
                        context.VSSetShaderResources(slot as u32, Some(views));
                        context.PSSetShaderResources(slot as u32, Some(views));
                    }
                }
            }
        }
        self.state.target = target;
        match texture {
            Some(texture) => self.state.target_texture.clone_from(texture),
            None => self.state.target_texture = None,
        }
    }

    /// Binds the surface's view for this frame, or nothing if there is none. Returns
    /// the size drawn into.
    fn bind_surface(&mut self) -> Extent {
        if self.state.surface.is_none() {
            self.bind_target(Bound::Nothing, None, None);
            return Extent::default();
        }
        let view = self.state.surface.take();
        self.bind_target(Bound::Surface, Some(&view), None);
        self.state.surface = view;
        self.state.surface_size
    }

    /// Checks that a pipeline and a vertex buffer are set and that `vertices` fit in
    /// the buffer, then sends the vertex buffer if it, its offset or the stride changed.
    fn prepare_vertices(&mut self, vertices: Range<u32>) -> Result<(), DrawError> {
        let out_of_range = DrawError::OutOfRange {
            start: vertices.start,
            end: vertices.end,
        };
        let (_, stride) = self.state.pipeline.ok_or(DrawError::NoPipeline)?;
        let vertex = self.state.vertex.ok_or(DrawError::NoVertexBuffer)?;
        let end = u64::from(vertices.end)
            .checked_mul(u64::from(stride))
            .and_then(|bytes| bytes.checked_add(vertex.offset));
        if vertices.start > vertices.end || end.is_none_or(|end| end > vertex.size) {
            return Err(out_of_range);
        }
        if self.state.vertex_dirty {
            // The offset is at most the buffer size, which fits in `u32`
            // (`create_buffer`).
            let offset = vertex.offset as u32;
            // SAFETY: the buffer is live (destroying it clears `state.vertex`); the
            // stride and offset pointers outlive the call.
            unsafe {
                self.context.IASetVertexBuffers(
                    0,
                    1,
                    Some(&raw const self.state.vertex_raw),
                    Some(&raw const stride),
                    Some(&raw const offset),
                );
            }
            self.state.vertex_dirty = false;
        }
        Ok(())
    }

    /// Writes `data` at `offset` with `UpdateSubresource`; for uniform buffers through
    /// the shadow copy, whole.
    fn update_buffer(&self, buffer: &D3d11Buffer, offset: u64, data: &[u8]) {
        let context = &self.context;
        // SAFETY: the range is inside the buffer (checked by the callers) and the
        // source outlives the call; constant buffers are updated whole, without a box.
        unsafe {
            if let Some(shadow) = &buffer.shadow {
                let mut shadow = shadow.borrow_mut();
                shadow[offset as usize..offset as usize + data.len()].copy_from_slice(data);
                context.UpdateSubresource(
                    buffer.interface(),
                    0,
                    None,
                    shadow.as_ptr().cast(),
                    0,
                    0,
                );
            } else {
                let region = D3D11_BOX {
                    left: offset as u32,
                    right: (offset + data.len() as u64) as u32,
                    top: 0,
                    bottom: 1,
                    front: 0,
                    back: 1,
                };
                context.UpdateSubresource(
                    buffer.interface(),
                    0,
                    Some(&region),
                    data.as_ptr().cast(),
                    0,
                    0,
                );
            }
        }
    }
}

fn create_device(
    driver: D3D_DRIVER_TYPE,
    flags: D3D11_CREATE_DEVICE_FLAG,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: the feature level slice and out-pointers outlive the call.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            flags,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(windows::core::Error::from_hresult(
            windows::Win32::Foundation::E_FAIL,
        )),
    }
}

fn backend(what: &'static str) -> impl Fn(windows::core::Error) -> DeviceError {
    move |e| DeviceError::Backend(format!("{what}: {e}"))
}

fn dxgi_format(format: TextureFormat) -> DXGI_FORMAT {
    match format {
        TextureFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM,
        TextureFormat::Rgba8Srgb => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        TextureFormat::R8 => DXGI_FORMAT_R8_UNORM,
        TextureFormat::R16Float => DXGI_FORMAT_R16_FLOAT,
    }
}

fn check_kind(buffer: &D3d11Buffer, expected: BufferKind) -> Result<(), DrawError> {
    if buffer.desc.kind == expected {
        Ok(())
    } else {
        Err(DrawError::BufferKind {
            expected,
            actual: buffer.desc.kind,
        })
    }
}

fn blend_desc(blend: Blend) -> D3D11_BLEND_DESC {
    let (enable, src, dst, src_alpha, dst_alpha, op) = match blend {
        Blend::Replace => (
            false,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ZERO,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ZERO,
            D3D11_BLEND_OP_ADD,
        ),
        Blend::Alpha => (
            true,
            D3D11_BLEND_SRC_ALPHA,
            D3D11_BLEND_INV_SRC_ALPHA,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ONE,
            D3D11_BLEND_OP_ADD,
        ),
        Blend::Premultiplied => (
            true,
            D3D11_BLEND_ONE,
            D3D11_BLEND_INV_SRC_ALPHA,
            D3D11_BLEND_ONE,
            D3D11_BLEND_INV_SRC_ALPHA,
            D3D11_BLEND_OP_ADD,
        ),
        Blend::Additive => (
            true,
            D3D11_BLEND_SRC_ALPHA,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ONE,
            D3D11_BLEND_OP_ADD,
        ),
        // Factors are ignored by MAX; `One` as in osu-framework (ADR-0041).
        Blend::Max => (
            true,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ONE,
            D3D11_BLEND_ONE,
            D3D11_BLEND_OP_MAX,
        ),
    };
    let mut desc = D3D11_BLEND_DESC::default();
    desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: enable.into(),
        SrcBlend: src,
        DestBlend: dst,
        BlendOp: op,
        SrcBlendAlpha: src_alpha,
        DestBlendAlpha: dst_alpha,
        BlendOpAlpha: op,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    desc
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: the blob owns `GetBufferSize` bytes at `GetBufferPointer` while alive,
    // and the slice borrows the blob.
    unsafe {
        let len = blob.GetBufferSize();
        if len == 0 {
            return &[];
        }
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), len)
    }
}

/// Compiled DXBC of both stages of a [`Shader`].
pub struct Bytecode {
    vertex: ID3DBlob,
    pixel: ID3DBlob,
}

impl Bytecode {
    pub fn vertex(&self) -> &[u8] {
        blob_bytes(&self.vertex)
    }

    pub fn pixel(&self) -> &[u8] {
        blob_bytes(&self.pixel)
    }
}

/// Compiles the shader's HLSL to Shader Model 5.0 with `D3DCompile`
/// (`d3dcompiler_47.dll`), as pipeline creation does. Compiler output is in the error.
pub fn compile_shader(shader: &Shader<'_>) -> Result<Bytecode, DeviceError> {
    let hlsl = &shader.hlsl;
    let compile = |entry: &str, target: &str| {
        let name = format!("{}.hlsl\0", shader.name);
        let entry = format!("{entry}\0");
        let target_z = format!("{target}\0");
        let mut code = None;
        let mut errors = None;
        // SAFETY: the source outlives the call, the strings are NUL-terminated and
        // both out-pointers point to live `Option<ID3DBlob>`s.
        let result = unsafe {
            D3DCompile(
                hlsl.source.as_ptr().cast(),
                hlsl.source.len(),
                PCSTR(name.as_ptr()),
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(target_z.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3 | D3DCOMPILE_ENABLE_STRICTNESS,
                0,
                &mut code,
                Some(&mut errors),
            )
        };
        match (result, code) {
            (Ok(()), Some(code)) => Ok(code),
            (result, _) => {
                let log = errors.as_ref().map(blob_bytes).unwrap_or_default();
                Err(DeviceError::Shader(format!(
                    "{} ({target}): {}: {}",
                    shader.name,
                    result.err().map_or_else(String::new, |e| e.to_string()),
                    String::from_utf8_lossy(log).trim()
                )))
            }
        }
    };
    Ok(Bytecode {
        vertex: compile(hlsl.vertex_entry, "vs_5_0")?,
        pixel: compile(hlsl.fragment_entry, "ps_5_0")?,
    })
}

impl<S: D3d11Surface> crate::hal::Device for D3d11Device<S> {
    type Texture = D3d11Texture;
    type Buffer = D3d11Buffer;
    type Pipeline = D3d11Pipeline;
    type RenderTarget = D3d11RenderTarget;

    fn limits(&self) -> Limits {
        self.limits
    }

    fn create_texture(
        &mut self,
        desc: &TextureDesc,
        data: Option<&[u8]>,
    ) -> Result<D3d11Texture, DeviceError> {
        desc.validate(&self.limits, data)?;
        let mut bind = 0;
        if desc.usage.contains(TextureUsage::SAMPLED) {
            bind |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }
        if desc.usage.contains(TextureUsage::RENDER_TARGET) {
            bind |= D3D11_BIND_RENDER_TARGET.0 as u32;
        }
        let raw_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.size.width,
            Height: desc.size.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgi_format(desc.format),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let initial = data.map(|data| D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr().cast(),
            SysMemPitch: desc.size.width * desc.format.bytes_per_pixel(),
            SysMemSlicePitch: 0,
        });
        let mut raw = None;
        let mut view = None;
        // SAFETY: `validate` checked the size against the limits and that `data` holds
        // exactly the texture's pixels, tightly packed (the pitch above); the
        // out-pointers point to live `Option`s.
        unsafe {
            self.device
                .CreateTexture2D(
                    &raw_desc,
                    initial.as_ref().map(|i| i as *const _),
                    Some(&mut raw),
                )
                .map_err(backend("texture"))?;
            let raw = raw.as_ref().expect("CreateTexture2D succeeded");
            if desc.usage.contains(TextureUsage::SAMPLED) {
                self.device
                    .CreateShaderResourceView(raw, None, Some(&mut view))
                    .map_err(backend("texture view"))?;
            }
        }
        Ok(D3d11Texture {
            raw: raw.expect("CreateTexture2D succeeded"),
            view,
            desc: *desc,
        })
    }

    fn destroy_texture(&mut self, texture: D3d11Texture) {
        let Some(view) = texture.view_ptr() else {
            return;
        };
        for (slot, bound) in self.state.textures.iter_mut().enumerate() {
            if bound.as_ref().map(Interface::as_raw) == Some(view) {
                *bound = None;
                let none = [None];
                // SAFETY: unbinding a slot below the limit.
                unsafe {
                    self.context.VSSetShaderResources(slot as u32, Some(&none));
                    self.context.PSSetShaderResources(slot as u32, Some(&none));
                }
            }
        }
    }

    fn write_texture(
        &mut self,
        texture: &D3d11Texture,
        origin: Origin,
        size: Extent,
        data: &[u8],
    ) -> Result<(), DescError> {
        texture.desc.check_write(origin, size, data.len())?;
        let region = D3D11_BOX {
            left: origin.x,
            top: origin.y,
            front: 0,
            right: origin.x + size.width,
            bottom: origin.y + size.height,
            back: 1,
        };
        // SAFETY: `check_write` keeps the region inside the texture and `data` exactly
        // the region's pixels, tightly packed (the row pitch).
        unsafe {
            self.context.UpdateSubresource(
                &texture.raw,
                0,
                Some(&region),
                data.as_ptr().cast(),
                size.width * texture.desc.format.bytes_per_pixel(),
                0,
            );
        }
        Ok(())
    }

    fn create_buffer(
        &mut self,
        desc: &BufferDesc,
        data: Option<&[u8]>,
    ) -> Result<D3d11Buffer, DeviceError> {
        desc.validate(&self.limits, data)?;
        let Ok(size) = u32::try_from(desc.size) else {
            return Err(DeviceError::Unsupported(format!(
                "buffer of {} bytes is larger than u32::MAX",
                desc.size
            )));
        };
        let bind = match desc.kind {
            BufferKind::Vertex => D3D11_BIND_VERTEX_BUFFER,
            BufferKind::Index => D3D11_BIND_INDEX_BUFFER,
            BufferKind::Uniform => D3D11_BIND_CONSTANT_BUFFER,
        };
        let raw_desc = D3D11_BUFFER_DESC {
            ByteWidth: size,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
            StructureByteStride: 0,
        };
        let initial = data.map(|data| D3D11_SUBRESOURCE_DATA {
            pSysMem: data.as_ptr().cast(),
            SysMemPitch: 0,
            SysMemSlicePitch: 0,
        });
        let mut raw = None;
        // SAFETY: `size` is positive and `data`, if given, holds exactly `size` bytes
        // (`validate`); the out-pointer points to a live `Option`.
        unsafe {
            self.device
                .CreateBuffer(
                    &raw_desc,
                    initial.as_ref().map(|i| i as *const _),
                    Some(&mut raw),
                )
                .map_err(backend("buffer"))?;
        }
        let shadow = (desc.kind == BufferKind::Uniform).then(|| {
            let contents = data.map_or_else(|| vec![0; size as usize], <[u8]>::to_vec);
            RefCell::new(contents.into_boxed_slice())
        });
        Ok(D3d11Buffer {
            raw,
            desc: *desc,
            shadow,
        })
    }

    fn destroy_buffer(&mut self, buffer: D3d11Buffer) {
        let raw = buffer.ptr();
        let state = &mut self.state;
        if state.vertex.is_some_and(|v| v.raw == raw) {
            state.vertex = None;
            state.vertex_raw = None;
            state.vertex_dirty = true;
            // SAFETY: unbinding vertex buffer slot 0.
            unsafe {
                let none: Option<ID3D11Buffer> = None;
                self.context
                    .IASetVertexBuffers(0, 1, Some(&raw const none), Some(&0), Some(&0));
            }
        }
        if state.index.is_some_and(|(i, _)| i.raw == raw) {
            state.index = None;
            // SAFETY: unbinding the index buffer.
            unsafe { self.context.IASetIndexBuffer(None, DXGI_FORMAT_R16_UINT, 0) };
        }
        for (slot, bound) in self.state.uniforms.iter_mut().enumerate() {
            if *bound == raw {
                *bound = std::ptr::null_mut();
                let none = [None];
                // SAFETY: unbinding a slot below the limit.
                unsafe {
                    self.context.VSSetConstantBuffers(slot as u32, Some(&none));
                    self.context.PSSetConstantBuffers(slot as u32, Some(&none));
                }
            }
        }
    }

    fn write_buffer(
        &mut self,
        buffer: &D3d11Buffer,
        offset: u64,
        data: &[u8],
    ) -> Result<(), DescError> {
        buffer.desc.check_write(offset, data.len())?;
        if !data.is_empty() {
            self.update_buffer(buffer, offset, data);
        }
        Ok(())
    }

    fn create_pipeline(&mut self, desc: &PipelineDesc<'_>) -> Result<D3d11Pipeline, DeviceError> {
        desc.validate(&self.limits)?;
        let bytecode = compile_shader(desc.shader)?;
        let name = desc.shader.name;
        let mut elements = [D3D11_INPUT_ELEMENT_DESC::default(); MAX_VERTEX_ATTRIBUTES];
        for (element, a) in elements.iter_mut().zip(desc.layout.attributes) {
            *element = D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(c"LOC".as_ptr().cast()),
                SemanticIndex: a.location,
                Format: match a.format {
                    VertexFormat::Float32x2 => DXGI_FORMAT_R32G32_FLOAT,
                    VertexFormat::Float32x3 => DXGI_FORMAT_R32G32B32_FLOAT,
                    VertexFormat::Float32x4 => DXGI_FORMAT_R32G32B32A32_FLOAT,
                    VertexFormat::Unorm8x4 => DXGI_FORMAT_R8G8B8A8_UNORM,
                },
                InputSlot: 0,
                AlignedByteOffset: a.offset,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            };
        }
        let elements = &elements[..desc.layout.attributes.len()];
        let shader_error = |what: &'static str| {
            move |e: windows::core::Error| DeviceError::Shader(format!("{name}: {what}: {e}"))
        };
        let (mut vertex, mut pixel, mut input_layout, mut blend) = (None, None, None, None);
        // SAFETY: the bytecode was produced by `D3DCompile` for these stages; the
        // element descriptions name a static semantic and are validated by
        // `VertexLayout::validate`; the out-pointers point to live `Option`s.
        unsafe {
            self.device
                .CreateVertexShader(bytecode.vertex(), None, Some(&mut vertex))
                .map_err(shader_error("vertex shader"))?;
            self.device
                .CreatePixelShader(bytecode.pixel(), None, Some(&mut pixel))
                .map_err(shader_error("pixel shader"))?;
            self.device
                .CreateInputLayout(elements, bytecode.vertex(), Some(&mut input_layout))
                .map_err(shader_error(
                    "vertex layout does not match the vertex inputs",
                ))?;
            self.device
                .CreateBlendState(&blend_desc(desc.blend), Some(&mut blend))
                .map_err(backend("blend state"))?;
        }
        let created = "the create calls succeeded";
        Ok(D3d11Pipeline {
            vertex: vertex.expect(created),
            pixel: pixel.expect(created),
            input_layout: input_layout.expect(created),
            blend: blend.expect(created),
            stride: desc.layout.stride,
        })
    }

    fn destroy_pipeline(&mut self, pipeline: D3d11Pipeline) {
        if self
            .state
            .pipeline
            .is_some_and(|(vertex, _)| vertex == pipeline.vertex.as_raw())
        {
            // SAFETY: unbinding shaders and input layout.
            unsafe {
                self.context.VSSetShader(None, None);
                self.context.PSSetShader(None, None);
                self.context.IASetInputLayout(None);
            }
            self.state.pipeline = None;
        }
    }

    fn create_render_target(
        &mut self,
        desc: &RenderTargetDesc,
    ) -> Result<D3d11RenderTarget, DeviceError> {
        desc.validate(&self.limits)?;
        let texture = self.create_texture(&desc.texture_desc(), None)?;
        let mut view = None;
        // SAFETY: the texture is live and was created with the render target bind flag
        // in a renderable format (`validate` rules out sRGB targets).
        unsafe {
            self.device
                .CreateRenderTargetView(&texture.raw, None, Some(&mut view))
                .map_err(backend("render target view"))?;
        }
        Ok(D3d11RenderTarget { view, texture })
    }

    fn destroy_render_target(&mut self, target: D3d11RenderTarget) {
        if self.state.target == Bound::Target(target.view_ptr()) {
            self.bind_surface();
        }
        self.destroy_texture(target.texture);
    }

    fn render_target_texture<'a>(&self, target: &'a D3d11RenderTarget) -> &'a D3d11Texture {
        &target.texture
    }

    fn begin_frame(&mut self, clear: Color) -> Extent {
        self.bind_target(Bound::Nothing, None, None);
        self.state.surface = None;
        let (view, size) = match self.surface.next_frame(&self.device) {
            Some((view, size)) => (Some(view), size),
            None => (None, Extent::default()),
        };
        self.state.surface = view;
        self.state.surface_size = size;
        self.begin_pass(None, Some(clear))
    }

    fn begin_pass(&mut self, target: Option<&D3d11RenderTarget>, clear: Option<Color>) -> Extent {
        let (size, view) = match target {
            Some(t) => {
                self.bind_target(
                    Bound::Target(t.view_ptr()),
                    Some(&t.view),
                    Some(&t.texture.view),
                );
                (t.size(), t.view.as_ref())
            }
            None => (self.bind_surface(), self.state.surface.as_ref()),
        };
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.width as f32,
            Height: size.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        // SAFETY: the view, if any, is live and bound.
        unsafe {
            self.context.RSSetViewports(Some(&[viewport]));
            if let (Some(c), Some(view)) = (clear, view) {
                self.context
                    .ClearRenderTargetView(view, &[c.r, c.g, c.b, c.a]);
            }
        }
        size
    }

    fn set_pipeline(&mut self, pipeline: &D3d11Pipeline) {
        let key = (pipeline.vertex.as_raw(), pipeline.stride);
        if self
            .state
            .pipeline
            .is_none_or(|(vertex, _)| vertex != key.0)
        {
            // SAFETY: the shaders and layout are live interfaces of this device.
            unsafe {
                self.context.VSSetShader(&pipeline.vertex, None);
                self.context.PSSetShader(&pipeline.pixel, None);
                self.context.IASetInputLayout(&pipeline.input_layout);
            }
            if self
                .state
                .pipeline
                .is_none_or(|(_, stride)| stride != key.1)
            {
                self.state.vertex_dirty = true;
            }
            self.state.pipeline = Some(key);
        }
        let blend = pipeline.blend.as_raw();
        if self.state.blend != blend {
            // SAFETY: the blend state is a live interface of this device.
            unsafe {
                self.context
                    .OMSetBlendState(&pipeline.blend, None, u32::MAX)
            };
            self.state.blend = blend;
        }
    }

    fn set_vertex_buffer(&mut self, buffer: &D3d11Buffer, offset: u64) -> Result<(), DrawError> {
        check_kind(buffer, BufferKind::Vertex)?;
        if !offset.is_multiple_of(u64::from(VERTEX_ALIGNMENT)) {
            return Err(DrawError::Unaligned { offset });
        }
        if offset > buffer.desc.size {
            return Err(DrawError::OffsetPastEnd {
                offset,
                size: buffer.desc.size,
            });
        }
        let bound = Some(BoundBuffer {
            raw: buffer.ptr(),
            size: buffer.desc.size,
            offset,
        });
        if self.state.vertex != bound {
            if self.state.vertex.is_none_or(|v| v.raw != buffer.ptr()) {
                self.state.vertex_raw.clone_from(&buffer.raw);
            }
            self.state.vertex = bound;
            self.state.vertex_dirty = true;
        }
        Ok(())
    }

    fn set_index_buffer(
        &mut self,
        buffer: &D3d11Buffer,
        format: IndexFormat,
    ) -> Result<(), DrawError> {
        check_kind(buffer, BufferKind::Index)?;
        let bound = BoundBuffer {
            raw: buffer.ptr(),
            size: buffer.desc.size,
            offset: 0,
        };
        if self.state.index != Some((bound, format)) {
            let dxgi = match format {
                IndexFormat::U16 => DXGI_FORMAT_R16_UINT,
                IndexFormat::U32 => DXGI_FORMAT_R32_UINT,
            };
            // SAFETY: the buffer is a live index buffer of this device.
            unsafe { self.context.IASetIndexBuffer(buffer.raw.as_ref(), dxgi, 0) };
            self.state.index = Some((bound, format));
        }
        Ok(())
    }

    fn set_uniform_buffer(&mut self, slot: u32, buffer: &D3d11Buffer) -> Result<(), DrawError> {
        check_kind(buffer, BufferKind::Uniform)?;
        let max = self.limits.max_uniform_buffer_slots;
        if slot >= max {
            return Err(DrawError::Slot { slot, max });
        }
        let bound = &mut self.state.uniforms[slot as usize];
        if *bound != buffer.ptr() {
            *bound = buffer.ptr();
            let buffers = std::slice::from_ref(&buffer.raw);
            // SAFETY: the slot is below the limit and the buffer is a live constant
            // buffer.
            unsafe {
                self.context.VSSetConstantBuffers(slot, Some(buffers));
                self.context.PSSetConstantBuffers(slot, Some(buffers));
            }
        }
        Ok(())
    }

    fn set_texture(&mut self, slot: u32, texture: &D3d11Texture) -> Result<(), DrawError> {
        if !texture.desc.usage.contains(TextureUsage::SAMPLED) {
            return Err(DrawError::NotSampled);
        }
        let max = self.limits.max_texture_slots;
        if slot >= max {
            return Err(DrawError::Slot { slot, max });
        }
        let bound = &mut self.state.textures[slot as usize];
        if bound.as_ref().map(Interface::as_raw) != texture.view_ptr() {
            bound.clone_from(&texture.view);
            // Sampling the texture being drawn into is undefined, and D3D11 refuses to
            // bind it: the slot stays empty until drawing moves to another target.
            let target = self.state.target_texture.as_ref().map(Interface::as_raw);
            let none = [None];
            let views = if target == texture.view_ptr() {
                &none
            } else {
                std::slice::from_ref(&texture.view)
            };
            // SAFETY: the slot is below the limit and the view, if any, is live.
            unsafe {
                self.context.VSSetShaderResources(slot, Some(views));
                self.context.PSSetShaderResources(slot, Some(views));
            }
        }
        Ok(())
    }

    fn draw(&mut self, vertices: Range<u32>) -> Result<(), DrawError> {
        self.prepare_vertices(vertices.clone())?;
        // Without a target (minimised window) there is nothing to draw into, and the
        // debug layer would warn on every draw.
        if !vertices.is_empty() && self.state.target != Bound::Nothing {
            // SAFETY: a pipeline and vertex buffer are bound and the range is inside
            // the buffer.
            unsafe { self.context.Draw(vertices.len() as u32, vertices.start) };
        }
        Ok(())
    }

    fn draw_indexed(&mut self, indices: Range<u32>) -> Result<(), DrawError> {
        let out_of_range = DrawError::OutOfRange {
            start: indices.start,
            end: indices.end,
        };
        let (index, format) = self.state.index.ok_or(DrawError::NoIndexBuffer)?;
        let size = u64::from(format.size());
        if indices.start > indices.end || u64::from(indices.end) * size > index.size {
            return Err(out_of_range);
        }
        self.prepare_vertices(0..0)?;
        if !indices.is_empty() && self.state.target != Bound::Nothing {
            // SAFETY: pipeline, vertex and index buffers are bound and the index range
            // is inside the index buffer. Index values are not checked (documented on
            // the trait); D3D11 reads out-of-range vertices as zero.
            unsafe {
                self.context
                    .DrawIndexed(indices.len() as u32, indices.start, 0)
            };
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), DeviceError> {
        if self.info_queue.is_some() {
            self.drain_debug_messages(|severity, text| match severity {
                D3D11_MESSAGE_SEVERITY_CORRUPTION | D3D11_MESSAGE_SEVERITY_ERROR => {
                    tracing::error!(target: "slam_render::d3d11", "{text}")
                }
                D3D11_MESSAGE_SEVERITY_WARNING => {
                    tracing::warn!(target: "slam_render::d3d11", "{text}")
                }
                _ => tracing::debug!(target: "slam_render::d3d11", "{text}"),
            });
        }
        self.surface.present().map_err(DeviceError::Surface)
    }
}

/// A surface that draws into a texture of its own, for tests and headless rendering.
/// `present` does nothing.
pub struct OffscreenSurface {
    texture: ID3D11Texture2D,
    view: ID3D11RenderTargetView,
    size: Extent,
    hidden: bool,
}

impl OffscreenSurface {
    /// An RGBA8 surface of `size`; use as `D3d11Device::new(.., |d| OffscreenSurface::new(d, size))`.
    pub fn new(device: &ID3D11Device, size: Extent) -> Result<Self, DeviceError> {
        if size.is_empty() {
            return Err(DescError::EmptySize.into());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.width,
            Height: size.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        let mut view = None;
        // SAFETY: a valid description; the out-pointers point to live `Option`s.
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(backend("offscreen surface"))?;
            let texture = texture.as_ref().expect("CreateTexture2D succeeded");
            device
                .CreateRenderTargetView(texture, None, Some(&mut view))
                .map_err(backend("offscreen surface view"))?;
        }
        Ok(Self {
            texture: texture.expect("CreateTexture2D succeeded"),
            view: view.expect("CreateRenderTargetView succeeded"),
            size,
            hidden: false,
        })
    }

    pub fn texture(&self) -> &ID3D11Texture2D {
        &self.texture
    }

    /// While hidden, frames have nothing to draw on, as with a minimised window.
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }
}

impl D3d11Surface for OffscreenSurface {
    fn next_frame(&mut self, _: &ID3D11Device) -> Option<(ID3D11RenderTargetView, Extent)> {
        (!self.hidden).then(|| (self.view.clone(), self.size))
    }

    fn present(&mut self) -> Result<(), String> {
        Ok(())
    }
}
