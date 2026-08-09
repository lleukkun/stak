mod error;
#[cfg(feature = "libc")]
mod libc;
mod memory;
#[cfg(feature = "std")]
mod os;
mod utility;
mod void;

pub use self::error::FileError;
use core::error::Error;
#[cfg(feature = "libc")]
pub use libc::LibcFileSystem;
pub use memory::MemoryFileSystem;
#[cfg(feature = "std")]
pub use os::OsFileSystem;
use stak_vm::{Heap, Memory, Value};
pub use void::VoidFileSystem;

/// A file descriptor.
pub type FileDescriptor = usize;

/// A file system.
pub trait FileSystem {
    /// A path.
    type Path: ?Sized;

    /// A path buffer.
    type PathBuf: AsRef<Self::Path>;

    /// An error.
    type Error: Error;

    /// Opens a file and returns its descriptor.
    fn open(&mut self, path: &Self::Path, output: bool) -> Result<FileDescriptor, Self::Error>;

    /// Closes a file.
    fn close(&mut self, descriptor: FileDescriptor) -> Result<(), Self::Error>;

    /// Reads a file.
    fn read(&mut self, descriptor: FileDescriptor) -> Result<Option<u8>, Self::Error>;

    /// Writes a file.
    fn write(&mut self, descriptor: FileDescriptor, byte: u8) -> Result<(), Self::Error>;

    /// Reads up to the length of `destination` bytes.
    ///
    /// A successful read may be short, and EOF is reported as `Ok(0)`. For
    /// a valid readable descriptor, an empty destination is a successful
    /// no-op. Implementations may validate the descriptor and direction
    /// before applying that no-op. The default implementation preserves
    /// compatibility with scalar-only filesystems.
    fn read_into(
        &mut self,
        descriptor: FileDescriptor,
        destination: &mut [u8],
    ) -> Result<usize, Self::Error> {
        let mut count = 0;

        while count < destination.len() {
            let Some(byte) = self.read(descriptor)? else {
                break;
            };

            destination[count] = byte;
            count += 1;
        }

        Ok(count)
    }

    /// Writes all bytes in `source` or returns an error.
    ///
    /// An empty source is a successful no-op. The default implementation
    /// preserves compatibility with scalar-only filesystems by forwarding each
    /// byte to `write`.
    fn write_from(&mut self, descriptor: FileDescriptor, source: &[u8]) -> Result<(), Self::Error> {
        for &byte in source {
            self.write(descriptor, byte)?;
        }

        Ok(())
    }

    /// Flushes a file.
    fn flush(&mut self, descriptor: FileDescriptor) -> Result<(), Self::Error>;

    /// Deletes a file.
    fn delete(&mut self, path: &Self::Path) -> Result<(), Self::Error>;

    /// Checks if a file exists.
    fn exists(&self, path: &Self::Path) -> Result<bool, Self::Error>;

    /// Decodes a path.
    fn decode_path<H: Heap>(memory: &Memory<H>, list: Value) -> Result<Self::PathBuf, Self::Error>;
}
