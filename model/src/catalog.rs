//! The content catalog: every file of every component version, addressable
//! by coordinates, with computed output paths and site URLs.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    descriptor::{ComponentDescriptor, DescriptorError},
    resource::{Family, ResourceRef},
};

/// Errors from scanning content sources into the catalog.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// The source root has no `antora.yml`.
    #[error("content source {0} has no antora.yml")]
    MissingDescriptor(PathBuf),

    /// The descriptor failed to load.
    #[error(transparent)]
    Descriptor(#[from] DescriptorError),

    /// A directory walk failed.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path being read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// Two files claimed the same resource coordinates.
    #[error("duplicate resource {0}")]
    DuplicateResource(String),
}

/// The full coordinates of one cataloged file.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Coords {
    /// Component name.
    pub component: String,
    /// Component version (`None` = versionless).
    pub version: Option<String>,
    /// Module name (`ROOT` for the root module).
    pub module: String,
    /// Resource family.
    pub family: Family,
    /// Family-relative source path, `/`-separated, with its source
    /// extension (e.g. `tables/borders.adoc`).
    pub path: String,
}

impl std::fmt::Display for Coords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}@{}:{}:{}${}",
            self.version.as_deref().unwrap_or("~"),
            self.component,
            self.module,
            self.family,
            self.path
        )
    }
}

/// One cataloged file.
#[derive(Clone, Debug)]
pub struct VirtualFile {
    /// The file's coordinates.
    pub coords: Coords,
    /// Absolute filesystem path of the source.
    pub src_path: PathBuf,
    /// Site-root-relative output path/URL (no leading slash), for
    /// publishable families; `None` for partials, examples, and nav files.
    pub url: Option<String>,
}

/// One component version registered in the catalog.
#[derive(Clone, Debug)]
pub struct Component {
    /// The parsed descriptor.
    pub desc: ComponentDescriptor,
    /// The content source root (the directory holding `antora.yml`).
    pub root: PathBuf,
}

impl Component {
    /// Site-wide attribute seeds contributed by this component's
    /// descriptor: `(name, Some(value))` to set, `(name, None)` to unset.
    pub fn attribute_seeds(&self) -> Vec<(String, Option<String>)> {
        self.desc.asciidoc.attribute_seeds()
    }

    /// Whether this component version is marked prerelease (`prerelease:`
    /// set to anything but `false`/`null` in `antora.yml`).
    pub fn is_prerelease(&self) -> bool {
        !matches!(
            &self.desc.prerelease,
            None | Some(serde_norway::Value::Null) | Some(serde_norway::Value::Bool(false))
        )
    }
}

/// The content catalog.
#[derive(Debug, Default)]
pub struct ContentCatalog {
    components: Vec<Component>,
    files: Vec<VirtualFile>,
    index: HashMap<Coords, usize>,
}

impl ContentCatalog {
    /// Creates an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scans one content source root (a directory containing `antora.yml`
    /// and `modules/`) into the catalog, returning the scanned component's
    /// `(name, version)` key.
    pub fn scan_source(&mut self, root: &Path) -> Result<(String, Option<String>), CatalogError> {
        self.scan_source_versioned(root, None)
    }

    /// Like [`scan_source`](Self::scan_source), with the component version
    /// overridden (used when the version derives from the git ref a source
    /// was aggregated from rather than from `antora.yml`).
    pub fn scan_source_versioned(
        &mut self,
        root: &Path,
        version_override: Option<&str>,
    ) -> Result<(String, Option<String>), CatalogError> {
        let descriptor_path = root.join("antora.yml");
        if !descriptor_path.is_file() {
            return Err(CatalogError::MissingDescriptor(root.to_path_buf()));
        }
        let mut desc = ComponentDescriptor::load(&descriptor_path)?;
        if let Some(version) = version_override {
            desc.version = Some(version.to_string());
        }

        let component = Component {
            desc,
            root: root.to_path_buf(),
        };

        self.scan_modules(&component)?;
        self.register_nav_files(&component)?;

        // The scanned component's key, so callers can associate
        // per-source data (e.g. spec coverage) with exactly the component
        // version this scan contributed.
        let key = (component.desc.name.clone(), component.desc.version.clone());
        self.components.push(component);
        Ok(key)
    }

    fn scan_modules(&mut self, component: &Component) -> Result<(), CatalogError> {
        let modules_dir = component.root.join("modules");
        for module_entry in read_dir_sorted(&modules_dir)? {
            if !module_entry.is_dir() {
                continue;
            }
            let module = file_name_string(&module_entry);
            if module.starts_with('.') || module.starts_with('_') {
                continue;
            }

            for family in [
                Family::Page,
                Family::Partial,
                Family::Image,
                Family::Attachment,
                Family::Example,
            ] {
                let family_dir = module_entry.join(family.dir_name().expect("family has dir"));
                if !family_dir.is_dir() {
                    continue;
                }
                self.scan_family(component, &module, family, &family_dir)?;
            }
        }
        Ok(())
    }

    fn scan_family(
        &mut self,
        component: &Component,
        module: &str,
        family: Family,
        family_dir: &Path,
    ) -> Result<(), CatalogError> {
        for path in walk_files(family_dir)? {
            let rel = path
                .strip_prefix(family_dir)
                .expect("walked file is under its family dir");
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");

            // Dotfiles are hidden from the catalog. Extension-less files
            // are hidden except in partials/examples (Antora's rule).
            let base = rel.rsplit('/').next().unwrap_or(&rel);
            if base.starts_with('.') {
                continue;
            }
            if !base.contains('.') && !matches!(family, Family::Partial | Family::Example) {
                continue;
            }

            let coords = Coords {
                component: component.desc.name.clone(),
                version: component.desc.version.clone(),
                module: module.to_string(),
                family,
                path: rel,
            };
            self.insert(coords, path)?;
        }
        Ok(())
    }

    fn register_nav_files(&mut self, component: &Component) -> Result<(), CatalogError> {
        for nav_rel in &component.desc.nav {
            let src = component.root.join(nav_rel);

            // Derive the nav file's module from its standard location
            // (`modules/<module>/nav.adoc`); fall back to ROOT.
            let module = Path::new(nav_rel)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>();
            let module = match module.as_slice() {
                [m0, m1, ..] if m0 == "modules" => m1.clone(),
                _ => "ROOT".to_string(),
            };

            let coords = Coords {
                component: component.desc.name.clone(),
                version: component.desc.version.clone(),
                module,
                family: Family::Nav,
                path: nav_rel.clone(),
            };
            self.insert(coords, src)?;
        }
        Ok(())
    }

    fn insert(&mut self, coords: Coords, src_path: PathBuf) -> Result<(), CatalogError> {
        if self.index.contains_key(&coords) {
            return Err(CatalogError::DuplicateResource(coords.to_string()));
        }

        let url = Self::url_for(&coords);
        let file = VirtualFile {
            coords: coords.clone(),
            src_path,
            url,
        };
        self.index.insert(coords, self.files.len());
        self.files.push(file);
        Ok(())
    }

    /// Computes the site-root-relative URL for publishable coordinates,
    /// per Antora's rules: `component/version/module/…`, dropping the
    /// component segment for `ROOT`, the version segment when versionless,
    /// and the module segment for `ROOT`; images and attachments gain a
    /// `_images/` / `_attachments/` family segment.
    fn url_for(coords: &Coords) -> Option<String> {
        if !coords.family.is_publishable() {
            return None;
        }

        let mut segments: Vec<String> = Vec::new();
        if coords.component != "ROOT" {
            segments.push(coords.component.clone());
        }
        if let Some(v) = &coords.version {
            segments.push(v.clone());
        }
        if coords.module != "ROOT" {
            segments.push(coords.module.clone());
        }

        match coords.family {
            Family::Page => {
                let html = coords
                    .path
                    .strip_suffix(".adoc")
                    .map(|stem| format!("{stem}.html"))
                    .unwrap_or_else(|| coords.path.clone());
                segments.push(html);
            }
            Family::Image => {
                segments.push("_images".to_string());
                segments.push(coords.path.clone());
            }
            Family::Attachment => {
                segments.push("_attachments".to_string());
                segments.push(coords.path.clone());
            }
            _ => unreachable!("non-publishable families returned above"),
        }

        Some(segments.join("/"))
    }

    /// The registered component versions, in scan order.
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The versions of one component, highest (per the display order) first.
    pub fn versions_of(&self, name: &str) -> Vec<&Component> {
        let mut versions: Vec<&Component> = self
            .components
            .iter()
            .filter(|c| c.desc.name == name)
            .collect();
        versions.sort_by(|a, b| {
            crate::versions::version_order(a.desc.version.as_deref(), b.desc.version.as_deref())
        });
        versions
    }

    /// The latest version of a component: the first non-prerelease in
    /// display order, else the first prerelease.
    pub fn latest_of(&self, name: &str) -> Option<&Component> {
        let versions = self.versions_of(name);
        versions
            .iter()
            .find(|c| !c.is_prerelease())
            .or_else(|| versions.first())
            .copied()
    }

    /// Every version of the page (or other resource) at `coords`: each of
    /// the component's versions, highest first, paired with that version's
    /// matching file when it exists.
    pub fn versions_of_resource(&self, coords: &Coords) -> Vec<(&Component, Option<&VirtualFile>)> {
        self.versions_of(&coords.component)
            .into_iter()
            .map(|component| {
                let candidate = Coords {
                    version: component.desc.version.clone(),
                    ..coords.clone()
                };
                (component, self.get(&candidate))
            })
            .collect()
    }

    /// All cataloged files.
    pub fn files(&self) -> &[VirtualFile] {
        &self.files
    }

    /// All files of one family.
    pub fn files_of(&self, family: Family) -> impl Iterator<Item = &VirtualFile> {
        self.files.iter().filter(move |f| f.coords.family == family)
    }

    /// Looks a file up by exact coordinates.
    pub fn get(&self, coords: &Coords) -> Option<&VirtualFile> {
        self.index.get(coords).map(|&i| &self.files[i])
    }

    /// Resolves a resource reference against the catalog, defaulting the
    /// omitted coordinates from `from` (the referencing file) and the
    /// family from `default_family`.
    ///
    /// A path starting with `./` resolves relative to the referencing
    /// file's directory within its family.
    pub fn resolve(
        &self,
        reference: &ResourceRef,
        from: &Coords,
        default_family: Family,
    ) -> Option<&VirtualFile> {
        let family = reference.family.unwrap_or(default_family);

        let component = reference
            .component
            .clone()
            .unwrap_or_else(|| from.component.clone());

        // With an explicit component but no explicit version, Antora
        // resolves to that component's *latest* version — including when
        // the reference names the referencing page's own component.
        // Without a component coordinate the reference stays within the
        // referencing version.
        let version = if let Some(v) = &reference.version {
            Some(v.clone())
        } else if reference.component.is_some() {
            self.latest_of(&component)?.desc.version.clone()
        } else {
            from.version.clone()
        };

        let module = reference
            .module
            .clone()
            .unwrap_or_else(|| from.module.clone());

        let path = if let Some(rest) = reference.path.strip_prefix("./") {
            match from.path.rsplit_once('/') {
                Some((dir, _)) => format!("{dir}/{rest}"),
                None => rest.to_string(),
            }
        } else {
            reference.path.clone()
        };

        self.get(&Coords {
            component,
            version,
            module,
            family,
            path,
        })
    }
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, CatalogError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| CatalogError::Io {
            path: dir.display().to_string(),
            source,
        })?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>, CatalogError> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in read_dir_sorted(&d)? {
            if entry.is_dir() {
                let name = file_name_string(&entry);
                if !name.starts_with('.') {
                    stack.push(entry);
                }
            } else {
                out.push(entry);
            }
        }
    }
    out.sort();
    Ok(out)
}

fn file_name_string(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coords(component: &str, module: &str, family: Family, path: &str) -> Coords {
        Coords {
            component: component.to_string(),
            version: None,
            module: module.to_string(),
            family,
            path: path.to_string(),
        }
    }

    #[test]
    fn url_rules() {
        assert_eq!(
            ContentCatalog::url_for(&coords("html5", "ROOT", Family::Page, "index.adoc")),
            Some("html5/index.html".to_string())
        );
        assert_eq!(
            ContentCatalog::url_for(&coords("html5", "api", Family::Page, "options.adoc")),
            Some("html5/api/options.html".to_string())
        );
        assert_eq!(
            ContentCatalog::url_for(&coords("ROOT", "ROOT", Family::Page, "index.adoc")),
            Some("index.html".to_string())
        );
        assert_eq!(
            ContentCatalog::url_for(&coords("html5", "cli", Family::Image, "pipe.png")),
            Some("html5/cli/_images/pipe.png".to_string())
        );
        assert_eq!(
            ContentCatalog::url_for(&coords("html5", "ROOT", Family::Partial, "hdr.adoc")),
            None
        );

        let mut versioned = coords("html5", "ROOT", Family::Page, "index.adoc");
        versioned.version = Some("1.4".to_string());
        assert_eq!(
            ContentCatalog::url_for(&versioned),
            Some("html5/1.4/index.html".to_string())
        );
    }

    #[test]
    fn scans_and_resolves_a_source_tree() {
        let dir = std::env::temp_dir().join(format!("bokfell-model-test-{}", std::process::id()));
        let module = dir.join("modules/ROOT");
        std::fs::create_dir_all(module.join("pages/sub")).unwrap();
        std::fs::create_dir_all(module.join("partials")).unwrap();
        std::fs::create_dir_all(dir.join("modules/api/pages")).unwrap();
        std::fs::write(
            dir.join("antora.yml"),
            "name: html5\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
        )
        .unwrap();
        std::fs::write(module.join("nav.adoc"), "* xref:index.adoc[]\n").unwrap();
        std::fs::write(module.join("pages/index.adoc"), "= Index\n").unwrap();
        std::fs::write(module.join("pages/sub/deep.adoc"), "= Deep\n").unwrap();
        std::fs::write(module.join("partials/hdr.adoc"), "shared\n").unwrap();
        std::fs::write(dir.join("modules/api/pages/options.adoc"), "= Options\n").unwrap();

        let mut catalog = ContentCatalog::new();
        catalog.scan_source(&dir).unwrap();

        assert_eq!(catalog.components().len(), 1);
        assert_eq!(catalog.files_of(Family::Page).count(), 3);
        assert_eq!(catalog.files_of(Family::Nav).count(), 1);

        let from = coords("html5", "ROOT", Family::Page, "index.adoc");

        // Same-module page reference.
        let hit = catalog
            .resolve(
                &ResourceRef::parse("sub/deep.adoc").unwrap(),
                &from,
                Family::Page,
            )
            .unwrap();
        assert_eq!(hit.url.as_deref(), Some("html5/sub/deep.html"));

        // Cross-module reference.
        let hit = catalog
            .resolve(
                &ResourceRef::parse("api:options.adoc").unwrap(),
                &from,
                Family::Page,
            )
            .unwrap();
        assert_eq!(hit.url.as_deref(), Some("html5/api/options.html"));

        // Partial by family coordinate.
        let hit = catalog
            .resolve(
                &ResourceRef::parse("partial$hdr.adoc").unwrap(),
                &from,
                Family::Page,
            )
            .unwrap();
        assert_eq!(hit.coords.family, Family::Partial);
        assert_eq!(hit.url, None);

        // `./`-relative reference from a subdirectory page.
        let from_deep = coords("html5", "ROOT", Family::Page, "sub/deep.adoc");
        let hit = catalog
            .resolve(
                &ResourceRef::parse("./deep.adoc").unwrap(),
                &from_deep,
                Family::Page,
            )
            .unwrap();
        assert_eq!(hit.coords.path, "sub/deep.adoc");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn component_qualified_references_resolve_to_latest_version() {
        let base = std::env::temp_dir().join(format!("bokfell-latest-test-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();

        // Scan order deliberately puts the OLDER version first, so scan
        // order and version order disagree.
        for version in ["1.0.0", "2.0.0"] {
            let root = base.join(version);
            std::fs::create_dir_all(root.join("modules/ROOT/pages")).unwrap();
            std::fs::write(root.join("antora.yml"), "name: demo\nversion: ~\n").unwrap();
            std::fs::write(root.join("modules/ROOT/pages/index.adoc"), "= Index\n").unwrap();
        }

        let mut catalog = ContentCatalog::new();
        catalog
            .scan_source_versioned(&base.join("1.0.0"), Some("1.0.0"))
            .unwrap();
        catalog
            .scan_source_versioned(&base.join("2.0.0"), Some("2.0.0"))
            .unwrap();

        let from_old = Coords {
            component: "demo".to_string(),
            version: Some("1.0.0".to_string()),
            module: "ROOT".to_string(),
            family: Family::Page,
            path: "index.adoc".to_string(),
        };

        // A component-qualified reference without a version — even naming
        // the referencing page's own component — routes to the latest
        // version, not the scan-first one and not the source version.
        let hit = catalog
            .resolve(
                &ResourceRef::parse("demo::index.adoc").unwrap(),
                &from_old,
                Family::Page,
            )
            .unwrap();
        assert_eq!(hit.coords.version.as_deref(), Some("2.0.0"));
        assert_eq!(hit.url.as_deref(), Some("demo/2.0.0/index.html"));

        // Without a component coordinate, the reference stays within the
        // referencing version.
        let hit = catalog
            .resolve(
                &ResourceRef::parse("index.adoc").unwrap(),
                &from_old,
                Family::Page,
            )
            .unwrap();
        assert_eq!(hit.coords.version.as_deref(), Some("1.0.0"));

        std::fs::remove_dir_all(&base).ok();
    }
}
