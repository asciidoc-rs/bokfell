//! Content model for the Bokfell documentation site generator.
//!
//! This crate holds the pieces of the pipeline that know what a site *is*
//! without knowing how it is rendered: the playbook and `antora.yml`
//! descriptors, the component/version/module/family coordinate system,
//! resource IDs, the content catalog (files, output paths, URLs), and the
//! navigation model. See `PLAN.md` §5–§6 at the workspace root.
//!
//! Repository-side formats stay Antora-compatible: the standard directory
//! set (`antora.yml` + `modules/<module>/<family>/`), resource IDs, and the
//! URL construction rules follow Antora's documented behavior so content
//! written for Antora builds unchanged.

mod catalog;
mod descriptor;
mod nav;
mod playbook;
mod resource;
mod versions;

pub use catalog::{CatalogError, Component, ContentCatalog, Coords, VirtualFile};
pub use descriptor::{AsciiDocConfig, ComponentDescriptor, DescriptorError};
pub use nav::{NavItem, NavTree};
pub use playbook::{Playbook, PlaybookError, RuntimeConfig, SourceConfig};
pub use resource::{relative_url, Family, ResourceRef};
pub use versions::{version_from_refname, version_order};
