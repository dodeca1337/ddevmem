mod devmem;
mod reg;

#[doc(hidden)]
pub use concat_idents::concat_idents as __concat_idents;

#[doc(inline)]
pub use devmem::DevMem;

#[doc(inline)]
pub use reg::{ReadOnlyReg, ReadOnlySliceReg, Reg, SliceReg};

#[macro_export]
/// A macro to define a register map structure with associated methods for accessing hardware registers.
/// # Usage
///
/// ```rust
/// register_map! {
///    pub unsafe map MyRegisterMap {
///         0x00 => rw reg0: u32,
///         0x04 => ro reg1: u32,
///         0x08 => wo reg2: u32
///     }
/// }
///
/// let devmem = unsafe { DevMem::new(0xD0DE_0000, None).unwrap() };
/// let mut reg_map = unsafe { MyRegisterMap::new(std::sync::Arc::new(devmem)).unwrap() };
/// let (reg0_offset, reg0_address) = (reg_map.reg0_address(), reg_map.reg0_offset());
/// let reg1_value = *reg_map.reg1();
/// *reg_map.mut_reg2() = reg1_value;
/// ```
macro_rules! register_map {
    ($vis: vis unsafe map $name: ident {$($reg_offset: expr => $reg_kind: ident $reg_name: ident : $reg_ty: ty),+}) => {
        $vis struct $name {
            devmem: std::sync::Arc<$crate::DevMem>,
            $($reg_name : std::ptr::NonNull<$reg_ty>),+
        }

        impl $name {
            /// Creates a new register map instance.
            ///
            /// # Safety
            ///
            /// DevMem does not track regions captured by registers.
            ///
            /// # Arguments
            ///
            /// * `devmem` - An `Arc` to the `DevMem` instance.
            ///
            /// # Returns
            ///
            /// Returns `None` if offset one of registers is out of bounds.
            #[inline(always)]
            pub unsafe fn new(devmem: std::sync::Arc<$crate::DevMem>) -> Option<Self> {
                $(let $reg_name = std::ptr::NonNull::new(devmem.get($reg_offset)? as *const $reg_ty as *mut $reg_ty).unwrap());+;
                Some(Self { devmem, $($reg_name),+ })
            }

            $(
                $crate::__register_methods!($vis reg $reg_offset => $reg_kind $reg_name: $reg_ty);
            )+
        }
    };
}

#[macro_export(local_inner_macros)]
#[doc(hidden)]
macro_rules! __register_methods {
    ($vis: vis reg $offset: literal => rw $name: ident : $ty: ty) => {
        $crate::__register_methods!($vis reg base $offset => $name: $ty);
        $crate::__register_methods!($vis reg read $offset => $name: $ty);
        $crate::__register_methods!($vis reg write $offset => $name: $ty);
    };
    ($vis: vis reg $offset: literal => wo $name: ident : $ty: ty) => {
        $crate::__register_methods!($vis reg base $offset => $name: $ty);
        $crate::__register_methods!($vis reg write $offset => $name: $ty);
    };
    ($vis: vis reg $offset: literal => ro $name: ident : $ty: ty) => {
        $crate::__register_methods!($vis reg base $offset => $name: $ty);
        $crate::__register_methods!($vis reg read $offset => $name: $ty);
    };
    ($vis: vis reg base $offset: literal => $name: ident : $ty: ty) => {
        $crate::__concat_idents!(fn_name = $name, _offset, {
            /// Returns the offset of the register within the DevMem.
            #[inline(always)]
            $vis fn fn_name(&self) -> usize {
                $offset
            }
        });

        $crate::__concat_idents!(fn_name = $name, _address, {
            /// Returns the address of the register.
            #[inline(always)]
            $vis fn fn_name(&self) -> usize {
                self.devmem.address() + $offset
            }
        });
    };
    ($vis: vis reg read $offset: literal => $name: ident : $ty: ty) => {
        /// Returns the reference to the register value.
        #[inline(always)]
        $vis fn $name(&self) -> &$ty {
            unsafe { std::hint::black_box(self.$name.as_ref()) }
        }
    };
    ($vis: vis reg write $offset: literal => $name: ident : $ty: ty) => {
        $crate::__concat_idents!(fn_name = mut_, $name, {
            /// Returns the mutable reference to the register value.
            #[inline(always)]
            $vis fn fn_name(&mut self) -> &mut $ty {
                unsafe { std::hint::black_box(self.$name.as_mut()) }
            }
        });
    }
}
