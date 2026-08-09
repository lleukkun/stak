/// A primitive of a file system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Primitive {
    /// Open a file.
    OpenFile,
    /// Close a file.
    CloseFile,
    /// Read a file.
    ReadFile,
    /// Write a file.
    WriteFile,
    /// Delete a file.
    DeleteFile,
    /// Check if a file exists.
    ExistsFile,
    /// Flush a file.
    FlushFile,
    /// Reads a bytevector from a file.
    ReadFileBulk,
    /// Writes a bytevector to a file.
    WriteFileBulk,
}

impl Primitive {
    pub(super) const OPEN_FILE: usize = Self::OpenFile as _;
    pub(super) const CLOSE_FILE: usize = Self::CloseFile as _;
    pub(super) const READ_FILE: usize = Self::ReadFile as _;
    pub(super) const WRITE_FILE: usize = Self::WriteFile as _;
    pub(super) const DELETE_FILE: usize = Self::DeleteFile as _;
    pub(super) const EXISTS_FILE: usize = Self::ExistsFile as _;
    pub(super) const FLUSH_FILE: usize = Self::FlushFile as _;
    pub(super) const READ_FILE_BULK: usize = Self::ReadFileBulk as _;
    pub(super) const WRITE_FILE_BULK: usize = Self::WriteFileBulk as _;
}
