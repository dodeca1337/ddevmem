use bytemuck::{AnyBitPattern, NoUninit};
use std::{fmt, io, mem, ptr};

#[cfg(all(feature = "device", not(feature = "emulator")))]
use memmap2::{MmapOptions, MmapRaw};

/// Error returned when creating a [`DevMem`] instance.
#[derive(Debug)]
pub enum Error {
    /// `/dev/mem` could not be opened.
    Open(io::Error),
    /// The `mmap` call failed (e.g. the address is not page-aligned).
    Mmap(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Open(err) => write!(f, "failed to open /dev/mem: {err}"),
            Error::Mmap(err) => write!(f, "failed to mmap /dev/mem: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Open(err) | Error::Mmap(err) => Some(err),
        }
    }
}

impl From<Error> for io::Error {
    fn from(err: Error) -> io::Error {
        match err {
            Error::Open(e) | Error::Mmap(e) => e,
        }
    }
}

/// Page-aligned, zero-initialized heap buffer standing in for a real
/// `/dev/mem` mapping. Page alignment mirrors the kernel mapping, so any
/// access that would be aligned on hardware is also aligned here.
#[cfg(feature = "emulator")]
struct EmulatorBuf {
    ptr: ptr::NonNull<u8>,
    layout: std::alloc::Layout,
}

#[cfg(feature = "emulator")]
impl EmulatorBuf {
    fn zeroed(size: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(size.max(1), page_size::get())
            .expect("emulator buffer layout");
        // SAFETY: `layout` has non-zero size.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        let Some(ptr) = ptr::NonNull::new(ptr) else {
            std::alloc::handle_alloc_error(layout);
        };
        Self { ptr, layout }
    }
}

#[cfg(feature = "emulator")]
impl Drop for EmulatorBuf {
    fn drop(&mut self) {
        // SAFETY: allocated in `zeroed` with the same layout.
        unsafe { std::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}

// SAFETY: `EmulatorBuf` owns its allocation and never hands out references
// to it; all access goes through raw pointers, with synchronization
// delegated to the caller exactly like the real MMIO backend.
#[cfg(feature = "emulator")]
unsafe impl Send for EmulatorBuf {}
#[cfg(feature = "emulator")]
unsafe impl Sync for EmulatorBuf {}

/// A memory-mapped view of a physical address range obtained from `/dev/mem`.
///
/// All reads and writes go through [`std::ptr::read_volatile`] /
/// [`std::ptr::write_volatile`], making this type suitable for MMIO register
/// access where the compiler must not reorder, merge, or elide accesses.
///
/// # Backends
///
/// * **`device`** (default) — maps `/dev/mem` via `memmap2`.
/// * **`emulator`** — a page-aligned, zero-initialized heap buffer for
///   testing without hardware.
///
/// When both features are enabled the `emulator` backend takes precedence,
/// which lets a crate's dev-dependencies opt into emulation for tests and
/// examples without disabling default features.
///
/// # Thread safety
///
/// `DevMem` is `Send + Sync` but provides no internal synchronization —
/// hardware registers cannot be protected by the Rust type system anyway.
/// Wrap it in an [`Arc`](std::sync::Arc) and guard register access with a
/// lock (e.g. `tokio::sync::Mutex`) when sharing across threads.
pub struct DevMem {
    #[cfg(all(feature = "device", not(feature = "emulator")))]
    mmap: MmapRaw,
    #[cfg(feature = "emulator")]
    buf: EmulatorBuf,
    address: usize,
    len: usize,
}

impl DevMem {
    /// Opens and memory-maps a physical address range.
    ///
    /// With the `device` feature the region is backed by `/dev/mem`; with
    /// `emulator` it is a zero-initialized heap buffer. Both are page-aligned.
    ///
    /// # Arguments
    ///
    /// * `address` — physical base address. `/dev/mem` requires it to be
    ///   page-aligned; the emulator accepts anything.
    /// * `size` — length in bytes. `None` defaults to the system page size.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that:
    /// - The address range refers to a device that tolerates the accesses
    ///   this mapping will perform.
    /// - No other mapping aliases the same region with conflicting
    ///   expectations.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Open`] if `/dev/mem` cannot be opened, or
    /// [`Error::Mmap`] if the `mmap` call fails.
    pub unsafe fn new(address: usize, size: Option<usize>) -> Result<Self, Error> {
        let len = size.unwrap_or_else(page_size::get);

        #[cfg(all(feature = "device", not(feature = "emulator")))]
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/mem")
                .map_err(Error::Open)?;

            let mmap = MmapOptions::new()
                .len(len)
                .offset(address as u64)
                .map_raw(&file)
                .map_err(Error::Mmap)?;

            Ok(Self { mmap, address, len })
        }

        #[cfg(feature = "emulator")]
        {
            Ok(Self { buf: EmulatorBuf::zeroed(len), address, len })
        }
    }

    /// Physical base address passed to [`DevMem::new`].
    #[inline(always)]
    pub fn address(&self) -> usize {
        self.address
    }

    /// Length of the mapped region in bytes.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when the mapped region has zero length.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Raw pointer to the first byte of the mapped region.
    ///
    /// The pointer remains valid for the lifetime of `self`. Use
    /// [`std::ptr::read_volatile`] / [`std::ptr::write_volatile`] for MMIO
    /// access through it; the caller is responsible for synchronization.
    #[inline(always)]
    pub fn as_ptr(&self) -> *mut u8 {
        #[cfg(all(feature = "device", not(feature = "emulator")))]
        {
            self.mmap.as_mut_ptr()
        }

        #[cfg(feature = "emulator")]
        {
            self.buf.ptr.as_ptr()
        }
    }

    /// Returns `true` when `count` values of `T` starting at `offset` lie
    /// within the mapping and the resulting pointer is aligned for `T`.
    #[inline(always)]
    fn access_ok<T>(&self, offset: usize, count: usize) -> bool {
        let Some(size) = mem::size_of::<T>().checked_mul(count) else {
            return false;
        };
        let Some(end) = offset.checked_add(size) else {
            return false;
        };
        end <= self.len && (self.as_ptr() as usize + offset).is_multiple_of(mem::align_of::<T>())
    }

    /// Volatile read of type `T` at `offset` bytes from the base.
    ///
    /// `T` must implement [`AnyBitPattern`] so that any bit pattern read from
    /// the device is a valid value.
    ///
    /// Returns `None` if the access would fall outside the mapped region or
    /// `offset` is not aligned for `T`.
    #[inline(always)]
    pub fn read<T: AnyBitPattern>(&self, offset: usize) -> Option<T> {
        if !self.access_ok::<T>(offset, 1) {
            return None;
        }
        // SAFETY: bounds and alignment verified by `access_ok`.
        Some(unsafe { self.read_unchecked(offset) })
    }

    /// Volatile write of `value` at `offset` bytes from the base.
    ///
    /// `T` must implement [`NoUninit`] to guarantee no padding bytes are
    /// written.
    ///
    /// Returns `None` if the access would fall outside the mapped region or
    /// `offset` is not aligned for `T`.
    #[inline(always)]
    pub fn write<T: NoUninit>(&self, offset: usize, value: T) -> Option<()> {
        if !self.access_ok::<T>(offset, 1) {
            return None;
        }
        // SAFETY: bounds and alignment verified by `access_ok`.
        unsafe { self.write_unchecked(offset, value) };
        Some(())
    }

    /// Volatile read-modify-write of type `T` at `offset`.
    ///
    /// Reads the current value, passes it to `f`, and writes the result back.
    /// The whole operation is **not** atomic.
    ///
    /// Returns `None` if the access would fall outside the mapped region or
    /// `offset` is not aligned for `T`.
    #[inline(always)]
    pub fn modify<T: AnyBitPattern + NoUninit>(
        &self,
        offset: usize,
        f: impl FnOnce(T) -> T,
    ) -> Option<()> {
        if !self.access_ok::<T>(offset, 1) {
            return None;
        }
        // SAFETY: bounds and alignment verified by `access_ok`.
        unsafe {
            let value = self.read_unchecked(offset);
            self.write_unchecked(offset, f(value));
        }
        Some(())
    }

    /// Volatile read of type `T` at `offset`, without bounds or alignment
    /// checks.
    ///
    /// # Safety
    ///
    /// `offset + size_of::<T>()` must not exceed [`len`](Self::len), and
    /// `offset` must be aligned for `T` (the mapping itself is page-aligned).
    #[inline(always)]
    pub unsafe fn read_unchecked<T: AnyBitPattern>(&self, offset: usize) -> T {
        ptr::read_volatile(self.as_ptr().add(offset).cast::<T>())
    }

    /// Volatile write of `value` at `offset`, without bounds or alignment
    /// checks.
    ///
    /// # Safety
    ///
    /// `offset + size_of::<T>()` must not exceed [`len`](Self::len), and
    /// `offset` must be aligned for `T` (the mapping itself is page-aligned).
    #[inline(always)]
    pub unsafe fn write_unchecked<T: NoUninit>(&self, offset: usize, value: T) {
        ptr::write_volatile(self.as_ptr().add(offset).cast::<T>(), value);
    }

    /// Volatile read of `buf.len()` consecutive values of `T` starting at
    /// `offset`, one [`read_volatile`](std::ptr::read_volatile) per element.
    ///
    /// Returns `None` if the access would fall outside the mapped region or
    /// `offset` is not aligned for `T`.
    #[inline(always)]
    pub fn read_slice<T: AnyBitPattern>(&self, offset: usize, buf: &mut [T]) -> Option<()> {
        if !self.access_ok::<T>(offset, buf.len()) {
            return None;
        }
        for (i, slot) in buf.iter_mut().enumerate() {
            // SAFETY: `access_ok` covered all `buf.len()` elements.
            *slot = unsafe { self.read_unchecked(offset + i * mem::size_of::<T>()) };
        }
        Some(())
    }

    /// Volatile write of `buf.len()` consecutive values of `T` starting at
    /// `offset`, one [`write_volatile`](std::ptr::write_volatile) per element.
    ///
    /// Returns `None` if the access would fall outside the mapped region or
    /// `offset` is not aligned for `T`.
    #[inline(always)]
    pub fn write_slice<T: NoUninit + Copy>(&self, offset: usize, buf: &[T]) -> Option<()> {
        if !self.access_ok::<T>(offset, buf.len()) {
            return None;
        }
        for (i, value) in buf.iter().enumerate() {
            // SAFETY: `access_ok` covered all `buf.len()` elements.
            unsafe { self.write_unchecked(offset + i * mem::size_of::<T>(), *value) };
        }
        Some(())
    }
}

impl fmt::Debug for DevMem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DevMem({:#X}..{:#X})",
            self.address,
            self.address + self.len
        )
    }
}
