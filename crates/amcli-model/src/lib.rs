//! The ArchiMate model layer: what an Archi file means, on top of the
//! byte-preserving document that `amcli-xml` provides.

use std::path::PathBuf;

pub mod canon;
pub mod container;
pub mod edit;
pub mod generated;
pub mod ids;
pub mod model;

pub use edit::{Cascade, EditError};
pub use generated::{ElementType, FolderType, Layer, RelType, matrix, viewpoints};
pub use model::{Concept, ConceptId, ConceptKind, Entity, Folder, FolderId, Model, View, ViewId};

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("`{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{path}`: {source}")]
    Xml {
        path: PathBuf,
        #[source]
        source: amcli_xml::XmlError,
    },
    #[error("`{path}`: not a zipped Archi model: {message}")]
    Archive { path: PathBuf, message: String },
    #[error(
        "`{path}`: root element is <{root}>, expected <archimate:model> — \
         this does not look like an Archi model file"
    )]
    NotAModel { path: PathBuf, root: String },
    #[error("{0}")]
    Edit(#[from] amcli_xml::MixedContent),
}
