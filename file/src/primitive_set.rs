mod error;
mod primitive;

pub use self::primitive::Primitive;
use crate::{FileError, FileSystem};
pub use error::PrimitiveError;
use stak_vm::{Cons, Error, Heap, ListVectorCursor, Memory, Number, PrimitiveSet, Value};
use winter_maybe_async::maybe_async;

const BYTEVECTOR_TAG: u16 = 8;
const VECTOR_FACTOR: usize = 64;
const BULK_BUFFER_SIZE: usize = 1024;

fn nonnegative_integer(value: Value) -> Result<usize, Error> {
    let number = Number::try_from(value)?;
    let integer = number.to_i64();

    if integer < 0 || number.to_f64() != integer as f64 {
        return Err(Error::NumberExpected);
    }

    usize::try_from(integer as u64).map_err(|_| Error::NumberExpected)
}

fn bytevector_range<H: Heap>(
    memory: &Memory<H>,
    value: Value,
    start: Value,
    end: Value,
) -> Result<(Cons, usize, usize, usize), Error> {
    let bytevector = value.to_cons().ok_or(Error::ConsExpected)?;
    let root = memory.cdr(bytevector)?;
    if root.tag() != BYTEVECTOR_TAG {
        return Err(Error::ConsExpected);
    }

    let length = nonnegative_integer(memory.car(bytevector)?)?;
    let start = nonnegative_integer(start)?;
    let end = nonnegative_integer(end)?;
    if start > end || end > length {
        return Err(Error::InvalidMemoryAccess);
    }

    let root = root.to_cons().ok_or(Error::ConsExpected)?;
    Ok((root, length, start, end))
}

/// A primitive set for a file system.
pub struct FilePrimitiveSet<T: FileSystem> {
    file_system: T,
}

impl<T: FileSystem> FilePrimitiveSet<T> {
    /// Creates a primitive set.
    pub const fn new(file_system: T) -> Self {
        Self { file_system }
    }
}

impl<T: FileSystem, H: Heap> PrimitiveSet<H> for FilePrimitiveSet<T> {
    type Error = PrimitiveError;

    #[maybe_async]
    fn operate(&mut self, memory: &mut Memory<H>, primitive: usize) -> Result<(), Self::Error> {
        match primitive {
            Primitive::OPEN_FILE => {
                let [list, output] = memory.pop_many()?;
                let path = T::decode_path(memory, list).map_err(|_| FileError::PathDecode)?;

                memory.push(
                    Number::from_i64(
                        self.file_system
                            .open(path.as_ref(), output != memory.boolean(false)?.into())
                            .map_err(|_| FileError::Open)? as _,
                    )
                    .into(),
                )?;
            }
            Primitive::CLOSE_FILE => {
                let [descriptor] = memory.pop_numbers()?;

                self.file_system
                    .close(descriptor.to_i64() as _)
                    .map_err(|_| FileError::Close)?;

                memory.push(memory.boolean(false)?.into())?;
            }
            Primitive::READ_FILE => {
                let [descriptor] = memory.pop_numbers()?;

                memory.push(
                    if let Some(byte) = self
                        .file_system
                        .read(descriptor.to_i64() as _)
                        .map_err(|_| FileError::Read)?
                    {
                        Number::from_i64(byte as _).into()
                    } else {
                        memory.boolean(false)?.into()
                    },
                )?;
            }
            Primitive::WRITE_FILE => {
                let [descriptor, byte] = memory.pop_numbers()?;

                self.file_system
                    .write(descriptor.to_i64() as _, byte.to_i64() as _)
                    .map_err(|_| FileError::Write)?;

                memory.push(memory.boolean(false)?.into())?;
            }
            Primitive::DELETE_FILE => {
                let [list] = memory.pop_many()?;
                let path = T::decode_path(memory, list).map_err(|_| FileError::PathDecode)?;

                self.file_system
                    .delete(path.as_ref())
                    .map_err(|_| FileError::Delete)?;

                memory.push(memory.boolean(false)?.into())?;
            }
            Primitive::EXISTS_FILE => {
                let [list] = memory.pop_many()?;
                let path = T::decode_path(memory, list).map_err(|_| FileError::PathDecode)?;

                memory.push(
                    memory
                        .boolean(
                            self.file_system
                                .exists(path.as_ref())
                                .map_err(|_| FileError::Exists)?,
                        )?
                        .into(),
                )?;
            }
            Primitive::FLUSH_FILE => {
                let [descriptor] = memory.pop_numbers()?;

                self.file_system
                    .flush(descriptor.to_i64() as _)
                    .map_err(|_| FileError::Flush)?;

                memory.push(memory.boolean(false)?.into())?;
            }
            Primitive::READ_FILE_BULK => {
                let [descriptor, bytevector, start, end] = memory.pop_many()?;
                let descriptor = Number::try_from(descriptor)?.to_i64() as _;
                let (root, length, start, end) = bytevector_range(memory, bytevector, start, end)?;
                let mut offset = start;
                let mut count = 0;
                let mut cursor = (start < end)
                    .then(|| ListVectorCursor::<VECTOR_FACTOR>::new(memory, root, length, start))
                    .transpose()?;

                while offset < end {
                    let size = (end - offset).min(BULK_BUFFER_SIZE);
                    let mut buffer = [0; BULK_BUFFER_SIZE];
                    let read = self
                        .file_system
                        .read_into(descriptor, &mut buffer[..size])
                        .map_err(|_| FileError::Read)?;

                    for &byte in &buffer[..read] {
                        let cell = cursor.as_mut().unwrap().next(memory)?;
                        memory.set_car(cell, Number::from_i64(byte as _).into())?;
                    }

                    offset += read;
                    count += read;
                    if read < size {
                        break;
                    }
                }

                memory.push(Number::from_i64(count as _).into())?;
            }
            Primitive::WRITE_FILE_BULK => {
                let [descriptor, bytevector, start, end] = memory.pop_many()?;
                let descriptor = Number::try_from(descriptor)?.to_i64() as _;
                let (root, length, start, end) = bytevector_range(memory, bytevector, start, end)?;
                let mut offset = start;
                let mut cursor = (start < end)
                    .then(|| ListVectorCursor::<VECTOR_FACTOR>::new(memory, root, length, start))
                    .transpose()?;

                while offset < end {
                    let size = (end - offset).min(BULK_BUFFER_SIZE);
                    let mut buffer = [0; BULK_BUFFER_SIZE];

                    for byte in &mut buffer[..size] {
                        let cell = cursor.as_mut().unwrap().next(memory)?;
                        let value = Number::try_from(memory.car(cell)?)?.to_i64();
                        *byte = u8::try_from(value).map_err(|_| Error::NumberExpected)?;
                    }

                    self.file_system
                        .write_from(descriptor, &buffer[..size])
                        .map_err(|_| FileError::Write)?;
                    offset += size;
                }

                memory.push(memory.boolean(false)?.into())?;
            }
            _ => return Err(Error::IllegalPrimitive.into()),
        }

        Ok(())
    }
}
