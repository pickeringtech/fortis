//! fortis-names: library modules, packages, the import split and name resolution (Language Reference 4).

pub mod compat;
pub mod features;
pub mod library;
pub mod lock;
pub mod sha256;
pub mod project;
pub mod resolve;
pub mod resolver;

pub use library::{repo_root, Libraries, Library, LibraryModule, CORE_MODULES};
pub use resolver::Resolver;
