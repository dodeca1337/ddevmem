use bytemuck::{AnyBitPattern, NoUninit};
use std::{fmt::Debug, hint::black_box, io::Error as IOError};

#[cfg(feature = "device")]
use memmap2::{MmapMut, MmapOptions};
#[cfg(feature = "device")]
use std::fs::OpenOptions;

#[derive(Debug)]
pub enum Error {
    CentOpenFile(IOError),
    CentMmapFile(IOError),
}

impl Into<IOError> for Error {
    fn into(self) -> IOError {
        match self {
            Error::CentOpenFile(err) => err,
            Error::CentMmapFile(err) => err,
        }
    }
}

pub struct DevMem {
    #[cfg(all(feature = "emulator", not(feature = "device")))]
    mmap: Vec<u8>,
    #[cfg(all(feature = "device", not(feature = "emulator")))]
    mmap: MmapMut,
    address: usize,
}

impl DevMem {
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

            let mmap = unsafe {
                MmapOptions::new()
                    .len(size)
                    .offset(address as u64)
                    .map_mut(&file)
            }
            .map_err(Error::CentMmapFile)?;

            Ok(Self { mmap, address })
        }

        #[cfg(all(feature = "emulator", not(feature = "device")))]
        {
            let mmap = vec![0; size];
            Ok(Self { mmap, address })
        }
    }

    #[inline(always)]
    pub fn address(&self) -> usize {
        self.address
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        black_box(&self.mmap)
    }

    #[inline(always)]
    pub fn data_mut(&mut self) -> &mut [u8] {
        black_box(&mut self.mmap)
    }

    #[inline(always)]
    pub fn get<T: AnyBitPattern>(&self, offset: usize) -> Option<&T> {
        let data = self.data().get(offset..offset + std::mem::size_of::<T>())?;

        Some(bytemuck::from_bytes(data))
    }

    #[inline(always)]
    pub fn get_mut<T: NoUninit + AnyBitPattern>(&mut self, offset: usize) -> Option<&mut T> {
        let data = self
            .data_mut()
            .get_mut(offset..offset + std::mem::size_of::<T>())?;

        Some(bytemuck::from_bytes_mut(data))
    }

    #[inline(always)]
    pub fn get_slice<T: AnyBitPattern>(&self, offset: usize, count: usize) -> Option<&[T]> {
        let data = self
            .data()
            .get(offset..offset + std::mem::size_of::<T>() * count)?;

        Some(bytemuck::cast_slice(data))
    }

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
