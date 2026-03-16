//! Typed register handles for single-value and array MMIO registers.
//!
//! [`Reg`] and [`SliceReg`] wrap a [`DevMem`] with an offset (and element
//! count for slices) and provide volatile access with compile-time
//! read/write control via the `WRITE` const-generic flag.
//!
//! Use the `ReadOnly*` type aliases when a register must not be written.

use crate::DevMem;
use bytemuck::{AnyBitPattern, NoUninit};
use std::sync::Arc;

/// Type alias for a read-only register (write methods are absent).
pub type ReadOnlyReg<T> = Reg<T, false>;

/// A typed handle to a single MMIO register inside a [`DevMem`] region.
///
/// All accesses use [`std::ptr::read_volatile`] / [`std::ptr::write_volatile`].
///
/// The const-generic `WRITE` parameter controls whether the write-side API
/// (`write`, `modify`) is available. Use the [`ReadOnlyReg<T>`] alias for
/// registers that must not be written.
///
/// # Thread safety
///
/// `Reg` is [`Send`] + [`Sync`] when `T` is, so it can be shared across
/// threads behind an appropriate lock.
pub struct Reg<T, const WRITE: bool = true> {
    devmem: Arc<DevMem>,
    offset: usize,
    _marker: std::marker::PhantomData<T>,
}

impl<T: AnyBitPattern + Copy, const WRITE: bool> Reg<T, WRITE> {
    /// Creates a new register handle.
    ///
    /// Returns `None` if `offset + size_of::<T>()` exceeds the mapped
    /// length of `devmem`.
    ///
    /// # Safety
    ///
    /// [`DevMem`] does not track which offsets are claimed. The caller
    /// must ensure that no other handle aliases this register with
    /// conflicting mutability.
    #[inline]
    pub unsafe fn new(devmem: Arc<DevMem>, offset: usize) -> Option<Self> {
        if offset + std::mem::size_of::<T>() > devmem.len() {
            return None;
        }
        Some(Self {
            devmem,
            offset,
            _marker: std::marker::PhantomData,
        })
    }

    /// Byte offset of this register within the [`DevMem`] region.
    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Physical address of this register (`devmem.address() + offset`).
    #[inline(always)]
    pub fn address(&self) -> usize {
        self.devmem.address() + self.offset
    }

    /// Volatile read of the register value.
    #[inline(always)]
    pub fn read(&self) -> T {
        unsafe { std::ptr::read_volatile(self.devmem.as_ptr().add(self.offset) as *const T) }
    }
}

impl<T: NoUninit + AnyBitPattern + Copy> Reg<T, true> {
    /// Volatile write of `value` to the register.
    #[inline(always)]
    pub fn write(&mut self, value: T) {
        unsafe { std::ptr::write_volatile(self.devmem.as_ptr().add(self.offset) as *mut T, value) }
    }

    /// Volatile read-modify-write.
    ///
    /// Reads the current value, passes it through `f`, and writes the
    /// result back. The operation is **not** atomic.
    #[inline(always)]
    pub fn modify(&mut self, f: impl FnOnce(T) -> T) {
        unsafe {
            let ptr = self.devmem.as_ptr().add(self.offset);
            let val = std::ptr::read_volatile(ptr as *const T);
            std::ptr::write_volatile(ptr as *mut T, f(val));
        }
    }
}

unsafe impl<T: Sync, const WRITE: bool> Sync for Reg<T, WRITE> {}
unsafe impl<T: Send, const WRITE: bool> Send for Reg<T, WRITE> {}

/// Type alias for a read-only slice register.
pub type ReadOnlySliceReg<T> = SliceReg<T, false>;

/// A typed handle to an array of consecutive MMIO registers.
///
/// Behaves like [`Reg`] but addresses `count` elements of type `T`
/// starting at `offset`. Each element is accessed individually via
/// `read_at` / `write_at` / `modify_at`.
///
/// The const-generic `WRITE` parameter controls the write-side API.
/// Use [`ReadOnlySliceReg<T>`] for read-only arrays.
pub struct SliceReg<T, const WRITE: bool = true> {
    devmem: Arc<DevMem>,
    offset: usize,
    count: usize,
    _marker: std::marker::PhantomData<T>,
}

impl<T: AnyBitPattern + Copy, const WRITE: bool> SliceReg<T, WRITE> {
    /// Creates a new slice-register handle.
    ///
    /// Returns `None` if `offset + size_of::<T>() * count` exceeds the
    /// mapped length.
    ///
    /// # Safety
    ///
    /// Same as [`Reg::new`] — the caller must avoid aliasing.
    #[inline]
    pub unsafe fn new(devmem: Arc<DevMem>, offset: usize, count: usize) -> Option<Self> {
        if offset + std::mem::size_of::<T>() * count > devmem.len() {
            return None;
        }
        Some(Self {
            devmem,
            offset,
            count,
            _marker: std::marker::PhantomData,
        })
    }

    /// Byte offset of this slice within the [`DevMem`] region.
    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Physical address of the first element.
    #[inline(always)]
    pub fn address(&self) -> usize {
        self.devmem.address() + self.offset
    }

    /// Number of elements in the slice.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` when the slice has zero elements.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Volatile read of element `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= self.len()`.
    #[inline(always)]
    pub fn read_at(&self, index: usize) -> T {
        assert!(index < self.count, "SliceReg index out of bounds");
        unsafe {
            std::ptr::read_volatile(self.devmem.as_ptr().add(self.offset).cast::<T>().add(index))
        }
    }
}

impl<T: NoUninit + AnyBitPattern + Copy> SliceReg<T, true> {
    /// Volatile write of `value` to element `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= self.len()`.
    #[inline(always)]
    pub fn write_at(&mut self, index: usize, value: T) {
        assert!(index < self.count, "SliceReg index out of bounds");
        unsafe {
            std::ptr::write_volatile(
                self.devmem.as_ptr().add(self.offset).cast::<T>().add(index),
                value,
            )
        }
    }

    /// Volatile read-modify-write of element `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= self.len()`.
    #[inline(always)]
    pub fn modify_at(&mut self, index: usize, f: impl FnOnce(T) -> T) {
        assert!(index < self.count, "SliceReg index out of bounds");
        unsafe {
            let ptr = self.devmem.as_ptr().add(self.offset).cast::<T>().add(index);
            let val = std::ptr::read_volatile(ptr);
            std::ptr::write_volatile(ptr, f(val));
        }
    }
}

unsafe impl<T: Sync, const WRITE: bool> Sync for SliceReg<T, WRITE> {}
unsafe impl<T: Send, const WRITE: bool> Send for SliceReg<T, WRITE> {}
