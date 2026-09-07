//! Catalog-backed `include::` resolution.
//!
//! Antora resolves include targets through its content catalog (resource
//! IDs like `partial$note.adoc`), not the filesystem. The parser's
//! `IncludeFileHandler` trait receives every include target, so this
//! handler serves catalog resources — falling back to a path relative to
//! the including file for targets that are not resource IDs.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use asciidoc_parser::{
    attributes::Attrlist,
    parser::{IncludeFileHandler, IncludeResolution, Parser},
};
use bokfell_model::{ContentCatalog, Coords, Family, ResourceRef};

/// Resolves `include::` targets through the content catalog.
///
/// The handler is created per parsed file, seeded with that file's
/// coordinates. Nested includes are tracked by remembering, for every
/// target served, which coordinates it resolved to — the parser hands the
/// including file's target string back as `source` for its nested
/// includes.
#[derive(Debug)]
pub struct CatalogIncludeHandler {
    catalog: Arc<ContentCatalog>,
    root: Coords,
    root_dir: Option<PathBuf>,
    served: RefCell<HashMap<String, Coords>>,
}

impl CatalogIncludeHandler {
    /// Creates the handler for a file at `root` coordinates, whose source
    /// lives in `root_dir` (for relative fallback resolution).
    pub fn new(catalog: Arc<ContentCatalog>, root: Coords, root_dir: Option<PathBuf>) -> Self {
        CatalogIncludeHandler {
            catalog,
            root,
            root_dir,
            served: RefCell::new(HashMap::new()),
        }
    }

    /// The coordinates the include context resolves against: the
    /// including file's, when we've served it before; the root file's
    /// otherwise.
    fn coords_for(&self, source: Option<&str>) -> Coords {
        source
            .and_then(|s| self.served.borrow().get(s).cloned())
            .unwrap_or_else(|| self.root.clone())
    }

    fn read(&self, path: &std::path::Path) -> IncludeResolution {
        match std::fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => IncludeResolution::Found(text.into()),
                Err(_) => IncludeResolution::NotDecodable,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => IncludeResolution::NotFound,
            Err(_) => IncludeResolution::NotReadable,
        }
    }
}

/// Resolves an include target (as recorded in a document's source map)
/// back to the file it was served from, mirroring
/// [`CatalogIncludeHandler`]'s resolution order: resource IDs through the
/// catalog, then a path relative to the including file's directory.
///
/// Targets of *nested* includes resolve against `from` (the page's own
/// coordinates) rather than the intermediate file's, so a deeply nested
/// origin can come back `None`; callers treat that as "not editable".
pub fn resolve_include_source(
    catalog: &ContentCatalog,
    from: &Coords,
    root_dir: Option<&Path>,
    target: &str,
) -> Option<PathBuf> {
    if let Some(reference) = ResourceRef::parse(target) {
        let default_family = if reference.family.is_some() {
            Family::Partial
        } else if from.family == Family::Partial || from.family == Family::Example {
            from.family
        } else {
            Family::Partial
        };
        if let Some(file) = catalog.resolve(&reference, from, default_family) {
            return Some(file.src_path.clone());
        }
    }

    root_dir
        .map(|dir| dir.join(target))
        .filter(|path| path.is_file())
}

impl IncludeFileHandler for CatalogIncludeHandler {
    fn resolve_target<'src>(
        &self,
        source: Option<&str>,
        target: &str,
        _attrlist: &Attrlist<'src>,
        _parser: &Parser,
    ) -> IncludeResolution {
        let from = self.coords_for(source);

        // Resource-ID resolution first. Includes default to the partial
        // family; an explicit family coordinate (`example$`, `page$`) wins.
        if let Some(reference) = ResourceRef::parse(target) {
            let default_family = if reference.family.is_some() {
                // Explicit family; the default is unused.
                Family::Partial
            } else if from.family == Family::Partial || from.family == Family::Example {
                // A partial including a sibling by bare path stays in its
                // own family.
                from.family
            } else {
                Family::Partial
            };

            if let Some(file) = self.catalog.resolve(&reference, &from, default_family) {
                let resolution = self.read(&file.src_path);
                if matches!(resolution, IncludeResolution::Found(_)) {
                    self.served
                        .borrow_mut()
                        .insert(target.to_string(), file.coords.clone());
                }
                return resolution;
            }
        }

        // Fallback: a plain path relative to the root file's directory
        // (covers non-catalog layouts and nav files including fragments).
        if let Some(dir) = &self.root_dir {
            let candidate = dir.join(target);
            if candidate.is_file() {
                return self.read(&candidate);
            }
        }

        IncludeResolution::NotFound
    }
}
