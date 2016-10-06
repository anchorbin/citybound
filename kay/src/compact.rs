use std::mem;
use std::mem::transmute;
use std::ptr;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use tagged_relative_pointer::TaggedRelativePointer;
use allocators::{Allocator, DefaultHeap};

pub trait Compact : Sized {
    fn is_still_compact(&self) -> bool;
    fn dynamic_size_bytes(&self) -> usize;
    fn total_size_bytes(&self) -> usize {
        self.dynamic_size_bytes() + mem::size_of::<Self>()
    }
    unsafe fn compact_from(&mut self, source: &Self, new_dynamic_part: *mut u8);
    unsafe fn behind(&mut self) -> *mut u8 {
        let behind_self = (self as *mut Self).offset(1);
        transmute(behind_self)
    }
    unsafe fn compact_behind_from(&mut self, source: &Self) {
        let behind_self = Self::behind(self);
        self.compact_from(source, behind_self)
    }
}

pub struct CompactVec <T, A: Allocator = DefaultHeap> {
    ptr: TaggedRelativePointer<T>,
    len: usize,
    cap: usize,
    _alloc: PhantomData<A>
}

const FREE : bool = true;
const EMBEDDED : bool = false;

impl<T, A: Allocator> CompactVec<T, A> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn new() -> CompactVec<T, A> {
        CompactVec {
            ptr: TaggedRelativePointer::null(EMBEDDED),
            len: 0,
            cap: 0,
            _alloc: PhantomData
        }
    }

    pub fn with_capacity(cap: usize) -> CompactVec<T, A> {
        let mut vec = CompactVec {
            ptr: TaggedRelativePointer::default(),
            len: 0,
            cap: cap,
            _alloc: PhantomData
        };

        vec.ptr.set(A::allocate::<T>(cap), FREE);
        vec
    }

    pub fn from_backing(ptr: *mut T, len: usize, cap: usize) -> CompactVec<T, A> {
        let mut vec = CompactVec {
            ptr: TaggedRelativePointer::default(),
            len: len,
            cap: cap,
            _alloc: PhantomData
        };

        vec.ptr.set(ptr, EMBEDDED);
        vec
    }

    fn maybe_drop(&mut self) {
        if self.ptr.is_tagged() == FREE {
            unsafe {
                ptr::drop_in_place(&mut self[..]);
                A::deallocate(self.ptr.mut_ptr(), self.cap);
            }
        }
    }

    fn double_buf(&mut self) {
        let new_cap = if self.cap == 0 {1} else {self.cap * 2};
        let mut vec = Vec::<T>::with_capacity(new_cap);
        let new_ptr = vec.as_mut_ptr();

        unsafe {
            ptr::copy_nonoverlapping(self.ptr.ptr(), new_ptr, self.len);
        }
        self.maybe_drop();
        self.ptr.set(new_ptr, FREE);
        unsafe {
            let p = self.ptr.ptr();
            self.cap = new_cap;
        }
    }

    pub fn push(&mut self, value: T) {
        if self.len == self.cap {
            self.double_buf();
        }

        unsafe {
            let end = self.as_mut_ptr().offset(self.len as isize);
            ptr::write(end, value);
            self.len += 1;
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            unsafe {
                self.len -= 1;
                Some(ptr::read(self.get_unchecked(self.len())))
            }
        }
    }

    pub fn insert(&mut self, index: usize, value: T) {
        if self.len == self.cap {
            self.double_buf();
        }

        unsafe {
            // infallible
            {
                let p = self.as_mut_ptr().offset(index as isize);
                ptr::copy(p, p.offset(1), self.len - index);
                ptr::write(p, value);
            }
            self.len += 1;
        }
    }
}

impl<T, A: Allocator> Drop for CompactVec<T, A> {
    fn drop(&mut self) {
        self.maybe_drop();
    }
}

impl<T, A: Allocator> Deref for CompactVec<T, A> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        unsafe {
            ::std::slice::from_raw_parts(self.ptr.ptr(), self.len)
        }
    }
}

impl<T, A: Allocator> DerefMut for CompactVec<T, A> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe {
            ::std::slice::from_raw_parts_mut(self.ptr.mut_ptr(), self.len)
        }
    }
}

impl<'a, T, A: Allocator> IntoIterator for &'a CompactVec<T, A> {
    type Item = &'a T;
    type IntoIter = ::std::slice::Iter<'a, T>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.deref().into_iter()
    }
}

impl<'a, T, A: Allocator> IntoIterator for &'a mut CompactVec<T, A> {
    type Item = &'a mut T;
    type IntoIter = ::std::slice::IterMut<'a, T>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.deref_mut().into_iter()
    }
}

impl<T, A: Allocator> Compact for CompactVec<T, A> {
    fn is_still_compact(&self) -> bool {
        self.ptr.is_tagged() == EMBEDDED
    }

    fn dynamic_size_bytes(&self) -> usize {
        self.cap * mem::size_of::<T>()
    }

    unsafe fn compact_from(&mut self, source: &Self, new_dynamic_part: *mut u8) {
        self.len = source.len;
        self.cap = source.cap;
        self.ptr.set(transmute(new_dynamic_part), EMBEDDED);
        ptr::copy_nonoverlapping(source.ptr.ptr(), self.ptr.mut_ptr(), self.len);
    }
}

macro_rules! plain {
    ($($trivial_type:ty),*) => {
        $(
            impl Compact for $trivial_type {
                fn is_still_compact(&self) -> bool {true}
                fn dynamic_size_bytes(&self) -> usize {0}
                unsafe fn compact_from(&mut self, source: &Self, _new_dynamic_part: *mut u8) {
                    *self = *source;
                }
            }
        )*
    }
}

//plain!(usize, u32, u16, u8, f32);

#[macro_export]
macro_rules! derive_compact {
    (struct $name:ident $fields:tt) => {
        echo_struct!($name, $fields);

        impl Compact for $name {
            fn is_still_compact(&self) -> bool {
                derive_is_still_compact!(self, $fields)
            }

            fn dynamic_size_bytes(&self) -> usize {
                derive_dynamic_size_bytes!(self, $fields)
            }

            unsafe fn compact_from(&mut self, source: &Self, new_dynamic_part: *mut u8) {
                #![allow(unused_assignments)]
                let mut offset: isize = 0;
                derive_compact_from!(self, source, new_dynamic_part, offset, $fields);
            }
        }
    }
}

// TODO: figure out how to resolve overlapping traits
// impl<T: Compact + !Copy> Compact for Option<T> {
//     fn is_still_compact(&self) -> bool {
//         match self {
//             &None => true,
//             &Some(ref inner) => inner.is_still_compact()
//         }
//     }
//     fn dynamic_size_bytes(&self) -> usize {
//         match self {
//             &None => 0,
//             &Some(ref inner) => inner.dynamic_size_bytes()
//         }
//     }
//     unsafe fn compact_from(&mut self, source: &Self, new_dynamic_part: *mut u8) {
//         ptr::copy_nonoverlapping(source as *const Self, self as *mut Self, 1);
//         match self {
//             &mut Some(ref mut inner) => match source {
//                 &Some(ref inner_source) => inner.compact_from(inner_source, new_dynamic_part),
//                 &None => {}
//             },
//             &mut None => {}
//         }
//     }
// }

impl<T: Copy> Compact for T {
    fn is_still_compact(&self) -> bool {true}
    fn dynamic_size_bytes(&self) -> usize {0}
    unsafe fn compact_from(&mut self, source: &Self, _new_dynamic_part: *mut u8) {
        *self = *source;
    }
}

#[macro_export]
macro_rules! echo_struct {
    ($name:ident, {$($field:ident: $field_type:ty),*}) => {
        struct $name {
            $($field: $field_type),*
        }
    }
}

#[macro_export]
macro_rules! derive_is_still_compact {
    ($the_self:ident, {$($field:ident: $field_type:ty),*}) => {
        $($the_self.$field.is_still_compact())&&*
    }
}

#[macro_export]
macro_rules! derive_dynamic_size_bytes {
    ($the_self:ident, {$($field:ident: $field_type:ty),*}) => {
        $($the_self.$field.dynamic_size_bytes() + )* 0
    }
}

#[macro_export]
macro_rules! derive_compact_from {
    ($the_self:ident, $source:ident, $new_dynamic_part:ident, $offset:ident, {$($field:ident: $field_type:ty),*}) => {
        $(
            $the_self.$field.compact_from(&$source.$field, $new_dynamic_part.offset($offset));
            $offset += $source.$field.dynamic_size_bytes() as isize;
        )*
    }
}