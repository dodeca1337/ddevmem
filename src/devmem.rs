use bytemuck::{AnyBitPattern, NoUninit};
use std::{cell::UnsafeCell, fmt, io::Error as IOError};

#[cfg(all(feature = "device", not(feature = "emulator")))]
use memmap2::{MmapMut, MmapOptions};

#[cfg(all(feature = "device", not(feature = "emulator")))]
use std::fs::OpenOptions;

/// Error returned when creating a [`DevMem`] instance.
///
/// Wraps the underlying I/O error from opening or memory-mapping `/dev/mem`.
/// Implements [`std::fmt::Display`], [`std::error::Error`], and
/// [`From<Error>`](std::convert::From) for [`std::io::Error`].
#[derive(Debug)]
pub enum Error {
    /// The `/dev/mem` file could not be opened.
    CantOpenFile(IOError),
    /// The memory-mapping (`mmap`) call failed.
    CantMmapFile(IOError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::CantOpenFile(err) => write!(f, "failed to open /dev/mem: {err}"),
            Error::CantMmapFile(err) => write!(f, "failed to mmap /dev/mem: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::CantOpenFile(err) | Error::CantMmapFile(err) => Some(err),
        }
    }
}

impl From<Error> for IOError {
    fn from(err: Error) -> IOError {
        match err {
            Error::CantOpenFile(e) | Error::CantMmapFile(e) => e,
        }
    }
}

/// A memory-mapped view of a physical address range obtained from `/dev/mem`.
///
/// All reads and writes go through [`std::ptr::read_volatile`] /
/// [`std::ptr::write_volatile`], making this type suitable for MMIO register
/// access where the compiler must not reorder, merge, or elide accesses.
///
/// # Backends
///
/// * **`device`** (default) — opens `/dev/mem` with `memmap2`.
/// * **`emulator`** — uses a heap-allocated `Vec<u8>` for testing.
///
/// Enable exactly one of the two.
///
/// # Thread safety
///
/// `DevMem` is `Send + Sync` but provides no internal synchronization.
/// Wrap it in an [`Arc`](std::sync::Arc) and protect all register accesses
/// with a lock (e.g. `tokio::sync::Mutex`) when sharing across threads.
pub struct DevMem {
    /// `UnsafeCell` is required for interior mutability: volatile writes go
    /// through `&self` (necessary when `DevMem` is shared via `Arc`), and the
    /// Rust memory model requires `UnsafeCell` to permit mutation through a
    /// shared reference without invoking undefined behaviour.
    #[cfg(feature = "emulator")]
    mmap: UnsafeCell<Vec<u8>>,
    #[cfg(all(feature = "device", not(feature = "emulator")))]
    mmap: UnsafeCell<MmapMut>,
    address: usize,
}

// SAFETY: Volatile MMIO accesses are inherently thread-unsafe at the hardware
// level. We declare Send + Sync here and delegate synchronization responsibility
// to the caller (Arc<Mutex<RegisterMap>> is the recommended pattern). This
// mirrors how AtomicXxx types work: they use UnsafeCell and assert Sync.
unsafe impl Send for DevMem {}
unsafe impl Sync for DevMem {}

impl DevMem {
    /// Opens and memory-maps a physical address range.
    ///
    /// When the `device` feature is active the region is backed by
    /// `/dev/mem`; with `emulator` it is a zero-initialized heap buffer.
    ///
    /// # Arguments
    ///
    /// * `address` — physical base address (must be page-aligned for
    ///   `/dev/mem`).
    /// * `size` — length in bytes.  `None` defaults to the system page
    ///   size.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that:
    /// - The address range is valid and not in use by the kernel.
    /// - No other mapping aliases the same region with conflicting
    ///   mutability.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CantOpenFile`] if `/dev/mem` cannot be opened, or
    /// [`Error::CantMmapFile`] if the `mmap` call fails.
    pub unsafe fn new(address: usize, size: Option<usize>) -> Result<Self, Error> {
        let page_size = page_size::get();
        let size = size.unwrap_or(page_size);

        #[cfg(all(feature = "device", not(feature = "emulator")))]
        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(false)
                .open("/dev/mem")
                .map_err(Error::CantOpenFile)?;

            let mmap = MmapOptions::new()
                .len(size)
                .offset(address as u64)
                .map_mut(&file)
                .map_err(Error::CantMmapFile)?;

            Ok(Self { mmap: UnsafeCell::new(mmap), address })
        }

        #[cfg(feature = "emulator")]
        {
            Ok(Self { mmap: UnsafeCell::new(vec![0; size]), address })
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
        // SAFETY: no aliasing &mut to the inner buffer exists at this call site.
        unsafe { (&*self.mmap.get()).len() }
    }

    /// Returns `true` when the mapped region has zero length.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        // SAFETY: same as len().
        unsafe { (&*self.mmap.get()).is_empty() }
    }

    /// Raw pointer to the first byte of the mapped region.
    ///
    /// The returned pointer remains valid for the lifetime of `self`.
    /// Use [`std::ptr::read_volatile`] / [`std::ptr::write_volatile`] to
    /// access MMIO registers through this pointer. The caller must ensure no
    /// conflicting accesses alias the same memory concurrently.
    #[inline(always)]
    pub fn as_ptr(&self) -> *mut u8 {
        // SAFETY: UnsafeCell grants permission to derive a *mut from &self.
        // Callers are responsible for synchronization.
        unsafe { (*self.mmap.get()).as_ptr() as *mut u8 }
    }

    /// Performs a volatile read of type `T` at `offset` bytes from the base.
    ///
    /// `T` must implement [`AnyBitPattern`] so that any bit pattern is a valid
    /// value.
    ///
    /// Returns `None` if `offset + size_of::<T>()` exceeds the mapped length.
    #[inline(always)]
    pub fn read<T: AnyBitPattern>(&self, offset: usize) -> Option<T> {
        if offset + std::mem::size_of::<T>() > self.len() {
            return None;
        }
        Some(unsafe { std::ptr::read_volatile(self.as_ptr().add(offset) as *const T) })
    }

    /// Performs a volatile write of `value` at `offset` bytes from the base.
    ///
    /// `T` must implement [`NoUninit`] to guarantee no padding bytes are
    /// written.
    ///
    /// Returns `None` if `offset + size_of::<T>()` exceeds the mapped length.
    #[inline(always)]
    pub fn write<T: NoUninit>(&self, offset: usize, value: T) -> Option<()> {
        if offset + std::mem::size_of::<T>() > self.len() {
            return None;
        }
        unsafe { std::ptr::write_volatile(self.as_ptr().add(offset) as *mut T, value) };
        Some(())
    }

    /// Volatile read-modify-write of type `T` at `offset`.
    ///
    /// Reads the current value, passes it to `f`, and writes the result back.
    /// The entire operation is **not** atomic.
    ///
    /// Returns `None` if `offset + size_of::<T>()` exceeds the mapped length.
    #[inline(always)]
    pub fn modify<T: AnyBitPattern + NoUninit>(
        &self,
        offset: usize,
        f: impl FnOnce(T) -> T,
    ) -> Option<()> {
        if offset + std::mem::size_of::<T>() > self.len() {
            return None;
        }
        unsafe {
            let ptr = self.as_ptr().add(offset);
            let val = std::ptr::read_volatile(ptr as *const T);
            std::ptr::write_volatile(ptr as *mut T, f(val));
        }
        Some(())
    }

    /// Volatile read of `buf.len()` consecutive elements of type `T` starting
    /// at `offset`.
    ///
    /// Each element is read with a separate [`std::ptr::read_volatile`].
    ///
    /// Returns `None` if `offset + size_of::<T>() * buf.len()` exceeds the
    /// mapped length.
    #[inline(always)]
    pub fn read_slice<T: AnyBitPattern>(&self, offset: usize, buf: &mut [T]) -> Option<()> {
        if offset + std::mem::size_of_val(buf) > self.len() {
            return None;
        }
        for (i, slot) in buf.iter_mut().enumerate() {
            unsafe {
                *slot = std::ptr::read_volatile(self.as_ptr().add(offset).cast::<T>().add(i));
            }
        }
        Some(())
    }

    /// Volatile write of `buf.len()` consecutive elements of type `T` starting
    /// at `offset`.
    ///
    /// Each element is written with a separate [`std::ptr::write_volatile`].
    /// `T: Copy` is required so that each element can be passed by value to
    /// `write_volatile` without affecting the original slice.
    ///
    /// Returns `None` if `offset + size_of::<T>() * buf.len()` exceeds the
    /// mapped length.
    #[inline(always)]
    pub fn write_slice<T: NoUninit + Copy>(&self, offset: usize, buf: &[T]) -> Option<()> {
        if offset + std::mem::size_of_val(buf) > self.len() {
            return None;
        }
        for (i, val) in buf.iter().enumerate() {
            unsafe {
                std::ptr::write_volatile(self.as_ptr().add(offset).cast::<T>().add(i), *val);
            }
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
            self.address + self.len()
        )
    }
}
