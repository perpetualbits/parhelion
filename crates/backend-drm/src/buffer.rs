//! Dumb scanout buffers: the pixels the CRTC actually reads (M2 T1).
//!
//! Governing doc: `docs/scene_graph_v1.md` §13.3.
//!
//! A *dumb buffer* is the kernel's lowest common denominator — plain CPU-visible
//! memory that any KMS driver can scan out, with no GPU, no allocator, and no
//! format negotiation. That is exactly the right thing for this task: M2 T1
//! presents the *existing CPU-rendered* frames on real hardware, and the GPU path
//! (dmabuf, modifiers, explicit sync) is deliberately T4–T6. Dumb buffers are how
//! we get to glass without pretending to have a renderer we have not written yet.
//!
//! # Why this bypasses Smithay's `DumbAllocator`
//!
//! Smithay wraps dumb buffers, but its wrapper exposes only
//! `handle(&self) -> &Handle`, while `map_dumb_buffer` needs `&mut` — so the
//! wrapper cannot map the buffer it allocated, and the one thing we do every
//! frame is write into that mapping. Four ioctls through `smithay::reexports::drm`
//! are smaller and more readable than a workaround. The seam rule is untouched:
//! no `smithay::backend::renderer` anywhere.
//!
//! # Ownership
//!
//! Every [`ScanoutBuffer`] belongs to **T-commit** (CORE-BOUNDARY §7), because it
//! is created from, mapped through, and destroyed with the DRM fd that thread
//! owns. Nothing here crosses a thread boundary; what crosses is the converted
//! pixel bytes, over a channel.

use std::io;

use smithay::backend::drm::DrmDeviceFd;
use smithay::reexports::drm::buffer::{Buffer as DrmBufferTrait, DrmFourcc};
use smithay::reexports::drm::control::{dumbbuffer::DumbBuffer, framebuffer, Device as ControlDevice};

use crate::present;

/// Colour depth and bits per pixel for `XRGB8888` — 24 bits of colour in a
/// 32-bit word. `add_framebuffer` wants them separately, and getting the pair
/// wrong is a mode-set that fails with `EINVAL` and no further explanation.
const XRGB8888_DEPTH: u32 = 24;
/// Bits per pixel for `XRGB8888` (see [`XRGB8888_DEPTH`]).
const XRGB8888_BPP: u32 = 32;

/// One dumb buffer plus the KMS framebuffer object that names it for scanout.
///
/// Held in pairs: while the CRTC scans one out, the compositor writes the other
/// (see [`crate::commit`]). Dropping it destroys both kernel objects.
#[derive(Debug)]
pub(crate) struct ScanoutBuffer {
    /// The DRM fd, cloned (ref-counted) so `Drop` can free the kernel objects
    /// without the caller having to remember to.
    device: DrmDeviceFd,
    /// The kernel's dumb buffer: CPU-visible memory with a driver-chosen stride.
    buffer: DumbBuffer,
    /// The framebuffer object an atomic commit points a plane at.
    fb: framebuffer::Handle,
}

impl ScanoutBuffer {
    /// Allocate a `width × height` `XRGB8888` dumb buffer and wrap it in a KMS
    /// framebuffer.
    ///
    /// `XRGB8888` and not `ARGB8888`: the primary plane is the bottom of the
    /// stack, there is nothing behind it to blend with, and asking the display
    /// engine to blend against nothing costs bandwidth on some hardware for a
    /// result that is identical.
    pub(crate) fn new(device: &DrmDeviceFd, width: u32, height: u32) -> io::Result<Self> {
        let buffer = device.create_dumb_buffer((width, height), DrmFourcc::Xrgb8888, XRGB8888_BPP)?;
        let fb = match device.add_framebuffer(&buffer, XRGB8888_DEPTH, XRGB8888_BPP) {
            Ok(fb) => fb,
            Err(e) => {
                // The buffer exists but has no framebuffer; free it rather than
                // leaking a screen's worth of kernel memory on a startup error.
                let _ = device.destroy_dumb_buffer(buffer);
                return Err(e);
            }
        };
        Ok(ScanoutBuffer {
            device: device.clone(),
            buffer,
            fb,
        })
    }

    /// The framebuffer handle to hand an atomic commit.
    pub(crate) fn framebuffer(&self) -> framebuffer::Handle {
        self.fb
    }

    /// The driver-chosen stride, in bytes per row. Recorded once at startup for
    /// the log line, because a padded stride is worth knowing about when a
    /// screenshot looks sheared.
    pub(crate) fn pitch(&self) -> u32 {
        self.buffer.pitch()
    }

    /// Write one frame's worth of already-converted `XRGB8888` bytes into this
    /// buffer, honouring the driver's stride.
    ///
    /// Returns the number of rows written; fewer than `height` means the buffer
    /// could not hold the image, which the caller reports rather than showing a
    /// half-drawn screen.
    ///
    /// Runs on T-commit, on the frame path. The mapping is established and torn
    /// down per frame: `map_dumb_buffer` is two syscalls, which is small against
    /// a multi-megabyte copy, and holding the mapping across frames would mean a
    /// self-referential struct (the mapping borrows the device) for no measured
    /// gain. If profiling ever says otherwise, that is a change with evidence
    /// behind it rather than a guess.
    pub(crate) fn write(&mut self, pixels: &[u8], width: u32, height: u32) -> io::Result<u32> {
        let pitch = self.buffer.pitch();
        let mut mapping = self.device.map_dumb_buffer(&mut self.buffer)?;
        Ok(present::blit_to_pitch(
            pixels,
            width,
            height,
            mapping.as_mut(),
            pitch,
        ))
    }
}

impl Drop for ScanoutBuffer {
    /// Free the framebuffer object first, then the memory behind it — the
    /// opposite order leaves the kernel holding a framebuffer that names a
    /// destroyed buffer. Errors are ignored because this runs on the way out and
    /// there is nothing useful left to do about them.
    fn drop(&mut self) {
        let _ = self.device.destroy_framebuffer(self.fb);
        let _ = self.device.destroy_dumb_buffer(self.buffer);
    }
}
