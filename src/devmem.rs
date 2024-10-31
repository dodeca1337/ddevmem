use bytemuck::{AnyBitPattern, NoUninit};
use std::{fmt::Debug, hint::black_box, io::Error as IOError};

#[cfg(feature = "device")]
use memmap2::{MmapMut, MmapOptions};

#[cfg(feature = "device")]
use std::fs::OpenOptions;

/// Represents an error that can occur while map [DevMem](crate::DevMem).
#[derive(Debug)]
pub enum Error {
    /// Error occurred while opening the file.
    CentOpenFile(IOError),
    /// Error occurred while memory-mapping the file.
    CentMmapFile(IOError),
}

/// Represents a memory-mapped device memory.
pub struct DevMem {
    #[cfg(all(feature = "emulator", not(feature = "device")))]
    mmap: Vec<u8>,
    #[cfg(all(feature = "device", not(feature = "emulator")))]
    mmap: MmapMut,
    address: usize,
}

impl DevMem {
    /// Creates a new `DevMem` instance.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it allows memory-mapping arbitrary addresses,
    /// which can lead to undefined behavior if someone else use mmaped address
    ///
    /// # Arguments
    ///
    /// * `address` - The starting address of the memory-mapped region.
    /// * `size` - The size of the memory-mapped region. If `None`, the page size is used.
    ///
    /// # Errors
    ///
    /// Returns an `Error` if the file cannot be opened or memory-mapped.
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
                .map_err(Error::CentOpenFile)?;

            let mmap = MmapOptions::new()
                .len(size)
                .offset(address as u64)
                .map_mut(&file)
                .map_err(Error::CentMmapFile)?;

            Ok(Self { mmap, address })
        }

        #[cfg(all(feature = "emulator", not(feature = "device")))]
        {
            let mmap = vec![0; size];
            Ok(Self { mmap, address })
        }
    }

    /// Returns the starting address of the memory-mapped region.
    #[inline(always)]
    pub fn address(&self) -> usize {
        self.address
    }

    /// Returns the length of the memory-mapped region.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    /// Returns a reference to the memory-mapped data.
    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        black_box(&self.mmap)
    }

    /// Returns a mutable reference to the memory-mapped data.
    #[inline(always)]
    pub fn data_mut(&mut self) -> &mut [u8] {
        black_box(&mut self.mmap)
    }

    /// Returns a reference to a value of type `T` at the specified offset.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset from the start of the memory-mapped region.
    ///
    /// # Returns
    ///
    /// Returns `None` if the offset is out of bounds.
    #[inline(always)]
    pub fn get<T: AnyBitPattern>(&self, offset: usize) -> Option<&T> {
        let data = self.data().get(offset..offset + std::mem::size_of::<T>())?;

        Some(bytemuck::from_bytes(data))
    }

    /// Returns a mutable reference to a value of type `T` at the specified offset.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset from the start of the memory-mapped region.
    ///
    /// # Returns
    ///
    /// Returns`None` if the offset is out of bounds.
    #[inline(always)]
    pub fn get_mut<T: NoUninit + AnyBitPattern>(&mut self, offset: usize) -> Option<&mut T> {
        let data = self
            .data_mut()
            .get_mut(offset..offset + std::mem::size_of::<T>())?;

        Some(bytemuck::from_bytes_mut(data))
    }

    /// Returns a reference to a slice of values of type `T` at the specified offset.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset from the start of the memory-mapped region.
    /// * `count` - The number of elements in the slice.
    ///
    /// # Returns
    ///
    /// Returns `None` if the offset is out of bounds.
    #[inline(always)]
    pub fn get_slice<T: AnyBitPattern>(&self, offset: usize, count: usize) -> Option<&[T]> {
        let data = self
            .data()
            .get(offset..offset + std::mem::size_of::<T>() * count)?;

        Some(bytemuck::cast_slice(data))
    }

    /// Returns a mutable reference to a slice of values of type `T` at the specified offset.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset from the start of the memory-mapped region.
    /// * `count` - The number of elements in the slice.
    ///
    /// # Returns
    ///
    /// Returns `None` if the offset is out of bounds.
    #[inline(always)]
    pub fn get_slice_mut<T: NoUninit + AnyBitPattern>(
        &mut self,
        offset: usize,
        count: usize,
    ) -> Option<&mut [T]> {
        let data = self
            .data_mut()
            .get_mut(offset..offset + std::mem::size_of::<T>() * count)?;

        Some(bytemuck::cast_slice_mut(data))
    }
}

impl Debug for DevMem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "DevMem({:#X}..{:#X})",
            self.address,
            self.address + self.len()
        ))
    }
}
