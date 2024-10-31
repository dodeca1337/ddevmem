use crate::DevMem;
use bytemuck::{AnyBitPattern, NoUninit};
use std::{
    hint::black_box,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::Arc,
};

/// A type alias for a read-only register.
pub type ReadOnlyReg<T> = Reg<T, false>;

/// A structure representing a register with an optional write capability.
pub struct Reg<T, const WRITE: bool = true> {
    value: NonNull<T>,
    devmem: Arc<DevMem>,
    offset: usize,
}

impl<T: AnyBitPattern, const WRITE: bool> Reg<T, WRITE> {
    /// Creates a new register instance.
    ///
    /// # Safety
    ///
    /// [DevMem](crate::DevMem) does not track regions captured by registers.
    ///
    /// # Arguments
    ///
    /// * `devmem` - An `Arc` to the `DevMem` instance.
    /// * `offset` - The offset within the [DevMem](crate::DevMem).
    ///
    /// # Returns
    ///
    /// Returns `None` if the offset is out of bounds.
    #[inline]
    pub unsafe fn new(devmem: Arc<DevMem>, offset: usize) -> Option<Self> {
        let value_ptr = devmem.get(offset)? as *const T as *mut T;
        let value = NonNull::new(value_ptr)?;
        Some(Self {
            value,
            devmem,
            offset,
        })
    }

    /// Returns the offset of the register within the [DevMem](crate::DevMem).
    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the address of the register.
    #[inline(always)]
    pub fn address(&self) -> usize {
        self.devmem.address() + self.offset
    }
}

impl<T: AnyBitPattern, const WRITE: bool> Deref for Reg<T, WRITE> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { black_box(self.value.as_ref()) }
    }
}

impl<T: NoUninit + AnyBitPattern> DerefMut for Reg<T, true> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { black_box(self.value.as_mut()) }
    }
}

unsafe impl<T: Sync, const WRITE: bool> Sync for Reg<T, WRITE> {}
unsafe impl<T: Send, const WRITE: bool> Send for Reg<T, WRITE> {}

/// A type alias for a read-only slice register.
pub type ReadOnlySliceReg<T> = SliceReg<T, false>;

/// A structure representing a slice register with an optional write capability.
pub struct SliceReg<T, const WRITE: bool = true> {
    data: NonNull<[T]>,
    devmem: Arc<DevMem>,
    offset: usize,
}

impl<T: AnyBitPattern, const WRITE: bool> SliceReg<T, WRITE> {
    /// Creates a new slice register instance.
    ///
    /// # Safety
    ///
    /// [DevMem](crate::DevMem) does not track regions captured by registers.
    ///
    /// # Arguments
    ///
    /// * `devmem` - An `Arc` to the `DevMem` instance.
    /// * `offset` - The offset within the [DevMem](crate::DevMem).
    /// * `count` - The number of elements in the slice.
    ///
    /// # Returns
    ///
    /// Returns `None` if the offset is out of bounds.
    #[inline]
    pub unsafe fn new(devmem: Arc<DevMem>, offset: usize, count: usize) -> Option<Self> {
        let data_ptr = devmem.get_slice(offset, count)? as *const [T] as *mut [T];
        let data = NonNull::new(data_ptr)?;
        Some(Self {
            data,
            devmem,
            offset,
        })
    }

    /// Returns the offset of the slice register within the [DevMem](crate::DevMem).
    #[inline(always)]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the address of the slice register.
    #[inline(always)]
    pub fn address(&self) -> usize {
        self.devmem.address() + self.offset
    }
}

impl<T: AnyBitPattern, const WRITE: bool> Deref for SliceReg<T, WRITE> {
    type Target = [T];

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        unsafe { black_box(self.data.as_ref()) }
    }
}

impl<T: NoUninit + AnyBitPattern> DerefMut for SliceReg<T, true> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { black_box(self.data.as_mut()) }
    }
}

unsafe impl<T: Sync, const WRITE: bool> Sync for SliceReg<T, WRITE> {}
unsafe impl<T: Send, const WRITE: bool> Send for SliceReg<T, WRITE> {}
