//! Component version ordering, following Antora's documented rules.
//!
//! Within one component, versions sort: **versionless** first (a
//! versionless component version is always the latest), then **named**
//! versions (strings that don't parse as semver) in reverse alphabetical
//! order, then **semantic** versions in descending order (an optional
//! leading `v` is ignored). The **latest** version is the first entry in
//! that order that is not marked prerelease; if every version is a
//! prerelease, the first prerelease.

use std::cmp::Ordering;

/// Compares two version coordinates in display order (highest first).
///
/// `None` is the versionless coordinate and sorts before everything.
pub fn version_order(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(a), Some(b)) => match (parse_semver(a), parse_semver(b)) {
            // Named versions come before semantic ones…
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            // …and sort reverse-alphabetically among themselves.
            (None, None) => b.cmp(a),
            // Semantic versions sort descending.
            (Some(sa), Some(sb)) => sb.cmp(&sa),
        },
    }
}

/// A parsed `major.minor.patch` prefix plus any remainder (prerelease/build
/// suffix), enough to order real-world tags without a semver dependency.
///
/// The remainder is compared so `1.0.0` sorts *after* `1.0.0-rc.1`
/// (an empty remainder wins), matching semver precedence closely enough
/// for version listings.
#[derive(Debug, Eq, PartialEq)]
struct SemverKey {
    parts: [u64; 3],
    // `true` for a bare release; a release outranks any suffixed version
    // with the same numbers.
    release: bool,
    suffix: String,
}

impl Ord for SemverKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.parts
            .cmp(&other.parts)
            .then(self.release.cmp(&other.release))
            .then_with(|| self.suffix.cmp(&other.suffix))
    }
}

impl PartialOrd for SemverKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_semver(version: &str) -> Option<SemverKey> {
    let trimmed = version.strip_prefix('v').unwrap_or(version);

    let (numbers, suffix) = match trimmed.find(['-', '+']) {
        Some(at) => (&trimmed[..at], &trimmed[at..]),
        None => (trimmed, ""),
    };

    let mut parts = [0u64; 3];
    let mut seen = 0usize;
    for piece in numbers.split('.') {
        if seen >= 3 {
            return None;
        }
        parts[seen] = piece.parse().ok()?;
        seen += 1;
    }
    if seen == 0 {
        return None;
    }

    Some(SemverKey {
        parts,
        release: suffix.is_empty(),
        suffix: suffix.to_string(),
    })
}

/// Derives a version coordinate from a git ref name, for content sources
/// configured with `version_from_ref`.
///
/// A trailing dotted-number run introduced by a `v` is extracted
/// (`asciidoc-html5-v0.2.1` → `0.2.1`, `v1.4` → `1.4`); any other ref name
/// is used as-is (`main` → `main`), except that `/` (as in
/// `release/2.0`) becomes `-` so the version is usable as a URL segment.
pub fn version_from_refname(refname: &str) -> String {
    if let Some(at) = refname.rfind('v') {
        let candidate = &refname[at + 1..];
        let looks_semver = candidate.chars().next().is_some_and(|c| c.is_ascii_digit())
            && candidate.contains('.')
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
        if looks_semver {
            return candidate.to_string();
        }
    }
    refname.replace('/', "-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut versions: Vec<Option<&str>>) -> Vec<Option<&str>> {
        versions.sort_by(|a, b| version_order(*a, *b));
        versions
    }

    #[test]
    fn versionless_sorts_first() {
        assert_eq!(
            sorted(vec![Some("1.0"), None, Some("2.0")]),
            vec![None, Some("2.0"), Some("1.0")]
        );
    }

    #[test]
    fn named_before_semver_reverse_alphabetical() {
        assert_eq!(
            sorted(vec![Some("1.0"), Some("edge"), Some("dev")]),
            vec![Some("edge"), Some("dev"), Some("1.0")]
        );
    }

    #[test]
    fn semver_descending_with_v_prefix_and_prerelease() {
        assert_eq!(
            sorted(vec![
                Some("1.9.0"),
                Some("v1.10.0"),
                Some("1.10.0-rc.1"),
                Some("2.0"),
            ]),
            vec![
                Some("2.0"),
                Some("v1.10.0"),
                Some("1.10.0-rc.1"),
                Some("1.9.0"),
            ]
        );
    }

    #[test]
    fn refname_version_extraction() {
        assert_eq!(version_from_refname("asciidoc-html5-v0.2.1"), "0.2.1");
        assert_eq!(version_from_refname("v1.4"), "1.4");
        assert_eq!(version_from_refname("main"), "main");
        assert_eq!(version_from_refname("dev"), "dev");
        assert_eq!(version_from_refname("v2.0.0-rc.3"), "2.0.0-rc.3");
        assert_eq!(version_from_refname("release/2.0"), "release-2.0");
    }
}
