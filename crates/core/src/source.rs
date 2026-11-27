//! Read-only access to the files of a world.
//!
//! Everything in this crate that needs the content of a file asks a
//! [`WorldSource`] for it. The source decides what a path is and how a file is
//! read; this crate never names a file system.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Debug;

/// Read-only access to a world's files.
///
/// There is deliberately no way to write: the original world is never
/// modified.
pub trait WorldSource: Sync {
    /// Names a file or folder.
    type Path: Clone + Ord + Debug + Send + Sync;
    /// Why a file could not be read. It names the file involved.
    type Error: Send;

    /// The content of a file.
    fn read(&self, path: &Self::Path) -> Result<Vec<u8>, Self::Error>;

    /// The content of a file, or `None` if it does not exist.
    fn read_optional(&self, path: &Self::Path) -> Result<Option<Vec<u8>>, Self::Error>;

    /// The size of a file in bytes, without reading it.
    fn size(&self, path: &Self::Path) -> Result<u64, Self::Error>;

    /// Whether something exists at `path`. Anything that cannot be inspected
    /// counts as absent.
    fn exists(&self, path: &Self::Path) -> bool;

    /// The entries of a folder in no particular order. A folder that does
    /// not exist is empty.
    fn list(&self, dir: &Self::Path) -> Result<Vec<Entry<Self::Path>>, Self::Error>;

    /// `relative` (with `/` separators) inside `base`.
    fn join(&self, base: &Self::Path, relative: &str) -> Self::Path;

    /// The last component of `path`, if it is valid UTF-8.
    fn name<'a>(&self, path: &'a Self::Path) -> Option<&'a str>;

    /// A configuration file (YAML), or `None` if it does not exist.
    fn read_config(&self, path: &Self::Path) -> Result<Option<Config>, Self::Error>;
}

/// An entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry<P> {
    pub path: P,
    /// A regular file, possibly reached through symbolic links.
    pub is_file: bool,
}

/// The parts of a configuration document the checks look at. An empty
/// document is [`Config::Null`].
#[derive(Debug, Clone, PartialEq)]
pub enum Config {
    Null,
    Integer(i64),
    String(String),
    /// A mapping; entries whose key is not a string are left out.
    Map(BTreeMap<String, Config>),
    /// Any other value (lists, floats, booleans, ...).
    Other,
}
