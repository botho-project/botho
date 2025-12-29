// Copyright (c) 2018-2022 The Botho Foundation

//! Fixed-length buffer, useful for operating on a portion of an underlying
//! buffer.

use crate::aead::{Buffer, Error};

/// The rust aead crate is organized around a Buffer trait which abstracts
/// commonalities of alloc::vec::Vec and heapless::Vec which are useful for
/// aead abstractions.
///
/// The needed functionalities are:
/// - Getting the bytes that have been written as a &mut [u8] (or &[u8])
/// - Extending the buffer (which is allowed to fail)
/// - Truncating the buffer
///
/// A drawback of heapless is that it is strictly an "owning" data-structure,
/// it doesn't have light-weight "views" or "reference" types.
///
/// This provides a zero-overhead abstraction over &mut [u8] which does this,
/// so that applications can easily use the aead trait to encrypt into e.g.
/// [u8; 128] without using vec, making allocations, or using heapless, which
/// might commit them to storing extra counters in their structures.
///
/// This represents a view of a fixed capacity buffer, where len() indicates
/// how many bytes, from the beginning of the buffer, have been "used".
///
/// It is expected that this type will be used to wrap e.g. [u8;128] briefly
/// in order to interact with interfaces like Aead, and then discarded.
pub struct FixedBuffer<'a> {
    buf: &'a mut [u8],
    length: usize,
}

impl AsRef<[u8]> for FixedBuffer<'_> {
    fn as_ref(&self) -> &[u8] {
        &self.buf[..self.length]
    }
}

impl AsMut<[u8]> for FixedBuffer<'_> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.length]
    }
}

impl Buffer for FixedBuffer<'_> {
    fn len(&self) -> usize {
        self.length
    }
    fn is_empty(&self) -> bool {
        self.length == 0
    }

    fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), Error> {
        if other.len() > self.buf.len() - self.length {
            return Err(Error);
        }
        self.buf[self.length..self.length + other.len()].copy_from_slice(other);
        self.length += other.len();
        Ok(())
    }

    fn truncate(&mut self, len: usize) {
        self.length = core::cmp::min(self.length, len);
    }
}

impl<'a> FixedBuffer<'a> {
    /// Create a new FixedBuffer "view" over a mutable slice of bytes,
    /// with length set to zero, so that we will be overwriting those bytes.
    pub fn overwriting(target: &'a mut [u8]) -> Self {
        Self {
            buf: target,
            length: 0,
        }
    }

    /// Test if there is no more space to extend the buffer,
    /// i.e. we have completely exhausted the capacity.
    pub fn is_exhausted(&self) -> bool {
        self.buf.len() == self.length
    }
}

impl<'a> From<&'a mut [u8]> for FixedBuffer<'a> {
    /// Initialize a fixed buffer from a mutable slice, which is initially
    /// "exhausted", so all of the initial values of those bytes are in the
    /// buffer. This buffer can then be modified or truncated etc.
    fn from(buf: &'a mut [u8]) -> Self {
        let length = buf.len();
        Self { buf, length }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overwriting_creates_empty_buffer() {
        let mut data = [0u8; 16];
        let buffer = FixedBuffer::overwriting(&mut data);

        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
        assert!(!buffer.is_exhausted());
    }

    #[test]
    fn test_from_slice_creates_full_buffer() {
        let mut data = [1u8, 2, 3, 4, 5];
        let buffer = FixedBuffer::from(&mut data[..]);

        assert_eq!(buffer.len(), 5);
        assert!(!buffer.is_empty());
        assert!(buffer.is_exhausted());
    }

    #[test]
    fn test_extend_from_slice() {
        let mut data = [0u8; 16];
        let mut buffer = FixedBuffer::overwriting(&mut data);

        buffer.extend_from_slice(&[1, 2, 3, 4]).unwrap();

        assert_eq!(buffer.len(), 4);
        assert_eq!(buffer.as_ref(), &[1, 2, 3, 4]);
        assert!(!buffer.is_exhausted());
    }

    #[test]
    fn test_extend_from_slice_multiple() {
        let mut data = [0u8; 16];
        let mut buffer = FixedBuffer::overwriting(&mut data);

        buffer.extend_from_slice(&[1, 2]).unwrap();
        buffer.extend_from_slice(&[3, 4]).unwrap();
        buffer.extend_from_slice(&[5]).unwrap();

        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer.as_ref(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_extend_from_slice_to_capacity() {
        let mut data = [0u8; 4];
        let mut buffer = FixedBuffer::overwriting(&mut data);

        buffer.extend_from_slice(&[1, 2, 3, 4]).unwrap();

        assert_eq!(buffer.len(), 4);
        assert!(buffer.is_exhausted());
    }

    #[test]
    fn test_extend_from_slice_overflow_fails() {
        let mut data = [0u8; 4];
        let mut buffer = FixedBuffer::overwriting(&mut data);

        let result = buffer.extend_from_slice(&[1, 2, 3, 4, 5]);

        assert!(result.is_err());
        // Buffer should be unchanged
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_extend_partial_then_overflow() {
        let mut data = [0u8; 4];
        let mut buffer = FixedBuffer::overwriting(&mut data);

        buffer.extend_from_slice(&[1, 2]).unwrap();
        let result = buffer.extend_from_slice(&[3, 4, 5]);

        assert!(result.is_err());
        // Buffer should retain previous content
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.as_ref(), &[1, 2]);
    }

    #[test]
    fn test_truncate_shorter() {
        let mut data = [1u8, 2, 3, 4, 5];
        let mut buffer = FixedBuffer::from(&mut data[..]);

        buffer.truncate(3);

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.as_ref(), &[1, 2, 3]);
        assert!(!buffer.is_exhausted());
    }

    #[test]
    fn test_truncate_to_zero() {
        let mut data = [1u8, 2, 3, 4, 5];
        let mut buffer = FixedBuffer::from(&mut data[..]);

        buffer.truncate(0);

        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_truncate_longer_is_noop() {
        let mut data = [1u8, 2, 3];
        let mut buffer = FixedBuffer::from(&mut data[..]);

        buffer.truncate(10);

        // Should not change length
        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn test_as_ref() {
        let mut data = [1u8, 2, 3, 4, 5];
        let buffer = FixedBuffer::from(&mut data[..]);

        let slice: &[u8] = buffer.as_ref();
        assert_eq!(slice, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_as_mut() {
        let mut data = [1u8, 2, 3, 4, 5];
        let mut buffer = FixedBuffer::from(&mut data[..]);

        {
            let slice: &mut [u8] = buffer.as_mut();
            slice[0] = 99;
            slice[4] = 100;
        }

        assert_eq!(buffer.as_ref(), &[99, 2, 3, 4, 100]);
    }

    #[test]
    fn test_as_mut_only_affects_used_portion() {
        let mut data = [0u8; 8];
        let mut buffer = FixedBuffer::overwriting(&mut data);
        buffer.extend_from_slice(&[1, 2, 3]).unwrap();

        let slice: &mut [u8] = buffer.as_mut();
        assert_eq!(slice.len(), 3);

        slice[0] = 10;
        assert_eq!(buffer.as_ref(), &[10, 2, 3]);
    }

    #[test]
    fn test_is_empty_true() {
        let mut data = [0u8; 8];
        let buffer = FixedBuffer::overwriting(&mut data);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_is_empty_false() {
        let mut data = [0u8; 8];
        let mut buffer = FixedBuffer::overwriting(&mut data);
        buffer.extend_from_slice(&[1]).unwrap();
        assert!(!buffer.is_empty());
    }

    #[test]
    fn test_is_exhausted_zero_capacity() {
        let mut data = [0u8; 0];
        let buffer = FixedBuffer::overwriting(&mut data);

        // Zero capacity buffer is immediately exhausted
        assert!(buffer.is_exhausted());
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_extend_empty_slice() {
        let mut data = [0u8; 8];
        let mut buffer = FixedBuffer::overwriting(&mut data);

        buffer.extend_from_slice(&[]).unwrap();

        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_buffer_trait_len() {
        let mut data = [1u8, 2, 3];
        let buffer = FixedBuffer::from(&mut data[..]);

        // Using Buffer trait method
        assert_eq!(Buffer::len(&buffer), 3);
    }

    #[test]
    fn test_buffer_trait_is_empty() {
        let mut data = [0u8; 8];
        let buffer = FixedBuffer::overwriting(&mut data);

        // Using Buffer trait method
        assert!(Buffer::is_empty(&buffer));
    }
}
