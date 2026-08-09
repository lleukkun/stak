use super::utility::decode_path;
use crate::{FileDescriptor, FileSystem};
use core::str;
use stak_vm::{Heap, Memory, Value};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions, remove_file},
    io::{self, BufReader, BufWriter, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

const BUFFER_SIZE: usize = 64 * 1024;
const PATH_SIZE: usize = 256;

#[derive(Debug)]
enum OpenFile {
    Input(BufReader<File>),
    Output(BufWriter<File>),
}

/// A file system on an operating system.
#[derive(Debug, Default)]
pub struct OsFileSystem {
    descriptor: FileDescriptor,
    files: HashMap<FileDescriptor, OpenFile>,
}

impl OsFileSystem {
    /// Creates a file system.
    pub fn new() -> Self {
        Self::default()
    }

    fn file_mut(&mut self, descriptor: FileDescriptor) -> Result<&mut OpenFile, io::Error> {
        self.files
            .get_mut(&descriptor)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "corrupted file descriptor"))
    }
}

impl FileSystem for OsFileSystem {
    type Path = Path;
    type PathBuf = PathBuf;
    type Error = io::Error;

    fn open(&mut self, path: &Path, output: bool) -> Result<FileDescriptor, Self::Error> {
        let file = OpenOptions::new()
            .read(!output)
            .create(output)
            .write(output)
            .truncate(output)
            .open(path)?;
        let descriptor = self.descriptor;
        let file = if output {
            OpenFile::Output(BufWriter::with_capacity(BUFFER_SIZE, file))
        } else {
            OpenFile::Input(BufReader::with_capacity(BUFFER_SIZE, file))
        };

        self.files.insert(descriptor, file);
        self.descriptor = self.descriptor.wrapping_add(1);

        Ok(descriptor)
    }

    fn close(&mut self, descriptor: FileDescriptor) -> Result<(), Self::Error> {
        let Some(mut file) = self.files.remove(&descriptor) else {
            return Ok(());
        };

        let result = match &mut file {
            OpenFile::Input(_) => Ok(()),
            OpenFile::Output(writer) => writer.flush(),
        };

        if let Err(error) = result {
            self.files.insert(descriptor, file);
            return Err(error);
        }

        Ok(())
    }

    fn read(&mut self, descriptor: FileDescriptor) -> Result<Option<u8>, Self::Error> {
        let file = self.file_mut(descriptor)?;
        let OpenFile::Input(reader) = file else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "cannot read from an output file",
            ));
        };
        let mut buffer = [0u8; 1];
        let count = reader.read(&mut buffer)?;

        Ok((count > 0).then_some(buffer[0]))
    }

    fn write(&mut self, descriptor: FileDescriptor, byte: u8) -> Result<(), Self::Error> {
        let file = self.file_mut(descriptor)?;
        let OpenFile::Output(writer) = file else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "cannot write to an input file",
            ));
        };
        writer.write_all(&[byte])?;

        Ok(())
    }

    fn read_into(
        &mut self,
        descriptor: FileDescriptor,
        destination: &mut [u8],
    ) -> Result<usize, Self::Error> {
        let file = self.file_mut(descriptor)?;
        let OpenFile::Input(reader) = file else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "cannot read from an output file",
            ));
        };

        reader.read(destination)
    }

    fn write_from(&mut self, descriptor: FileDescriptor, source: &[u8]) -> Result<(), Self::Error> {
        let file = self.file_mut(descriptor)?;
        let OpenFile::Output(writer) = file else {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "cannot write to an input file",
            ));
        };

        writer.write_all(source)?;

        Ok(())
    }

    fn flush(&mut self, descriptor: FileDescriptor) -> Result<(), Self::Error> {
        let file = self.file_mut(descriptor)?;
        if let OpenFile::Output(writer) = file {
            writer.flush()?;
        }

        Ok(())
    }

    fn delete(&mut self, path: &Path) -> Result<(), Self::Error> {
        remove_file(path)
    }

    fn exists(&self, path: &Path) -> Result<bool, Self::Error> {
        Ok(path.exists())
    }

    fn decode_path<H: Heap>(memory: &Memory<H>, list: Value) -> Result<Self::PathBuf, Self::Error> {
        Ok(PathBuf::from(
            str::from_utf8(
                &decode_path::<PATH_SIZE, _>(memory, list)
                    .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "path too long"))?,
            )
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use std::fs;

    #[test]
    fn close() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");
        fs::write(&path, []).unwrap();

        let mut file_system = OsFileSystem::new();

        let descriptor = file_system.open(&path, false).unwrap();
        file_system.close(descriptor).unwrap();
    }

    #[test]
    fn read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");

        let mut file_system = OsFileSystem::new();

        fs::write(&path, [42]).unwrap();

        let descriptor = file_system.open(&path, false).unwrap();

        assert_eq!(file_system.read(descriptor).unwrap(), Some(42));
        assert_eq!(file_system.read(descriptor).unwrap(), None);
    }

    #[test]
    fn buffered_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");
        let expected: Vec<_> = (0..(2 * BUFFER_SIZE + 17))
            .map(|index| index as u8)
            .collect();
        fs::write(&path, &expected).unwrap();

        let mut file_system = OsFileSystem::new();
        let descriptor = file_system.open(&path, false).unwrap();
        assert_eq!(file_system.read_into(descriptor, &mut []).unwrap(), 0);
        let mut actual = Vec::new();
        let mut buffer = [0; 8192];

        loop {
            let count = file_system.read_into(descriptor, &mut buffer).unwrap();
            if count == 0 {
                break;
            }
            actual.extend_from_slice(&buffer[..count]);
        }

        assert_eq!(actual, expected);
        file_system.close(descriptor).unwrap();
    }

    #[test]
    fn write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");

        let mut file_system = OsFileSystem::new();

        let descriptor = file_system.open(&path, true).unwrap();

        file_system.write(descriptor, 42).unwrap();
        file_system.close(descriptor).unwrap();

        let descriptor = file_system.open(&path, false).unwrap();
        assert_eq!(file_system.read(descriptor).unwrap(), Some(42));
        assert_eq!(file_system.read(descriptor).unwrap(), None);
        file_system.close(descriptor).unwrap();
    }

    #[test]
    fn buffered_write_is_persisted_by_close() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");
        let expected: Vec<_> = (0..(2 * BUFFER_SIZE + 17))
            .map(|index| index as u8)
            .collect();

        let mut file_system = OsFileSystem::new();
        let descriptor = file_system.open(&path, true).unwrap();

        file_system.write_from(descriptor, &[]).unwrap();
        file_system.write_from(descriptor, &expected).unwrap();

        file_system.close(descriptor).unwrap();

        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn flush() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");

        let mut file_system = OsFileSystem::new();

        let descriptor = file_system.open(&path, true).unwrap();

        file_system.write(descriptor, 42).unwrap();
        assert!(fs::read(&path).unwrap().is_empty());

        file_system.flush(descriptor).unwrap();
        assert_eq!(fs::read(&path).unwrap(), [42]);

        file_system.close(descriptor).unwrap();
    }

    #[test]
    fn wrong_direction_operations_fail() {
        let directory = tempfile::tempdir().unwrap();
        let input_path = directory.path().join("input");
        let output_path = directory.path().join("output");
        fs::write(&input_path, [42]).unwrap();

        let mut file_system = OsFileSystem::new();
        let input = file_system.open(&input_path, false).unwrap();
        let output = file_system.open(&output_path, true).unwrap();

        assert!(file_system.write(input, 42).is_err());
        assert!(file_system.write_from(input, &[42]).is_err());
        assert!(file_system.read(output).is_err());
        assert!(file_system.read_into(output, &mut [0]).is_err());

        file_system.close(input).unwrap();
        file_system.close(output).unwrap();
    }

    #[test]
    fn invalid_descriptor_operations_fail() {
        let mut file_system = OsFileSystem::new();
        let descriptor = FileDescriptor::MAX;

        assert!(file_system.read(descriptor).is_err());
        assert!(file_system.read_into(descriptor, &mut [0]).is_err());
        assert!(file_system.write(descriptor, 42).is_err());
        assert!(file_system.write_from(descriptor, &[42]).is_err());
        assert!(file_system.flush(descriptor).is_err());
        assert!(file_system.close(descriptor).is_ok());
    }

    #[test]
    fn delete() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");
        fs::write(&path, []).unwrap();

        let mut file_system = OsFileSystem::new();

        file_system.delete(&path).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn exists() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("foo");
        fs::write(&path, []).unwrap();

        let file_system = OsFileSystem::new();

        assert!(file_system.exists(&path).unwrap());
    }
}
