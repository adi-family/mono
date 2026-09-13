//! Addressing one marketplace item: the whole bundle, or one element inside it.
//!
//! `<marketplace>/<slug>` still names the whole bundle — installing it with nothing further
//! installs everything it offers, exactly as a v1 app always has. A single element adds a third
//! coordinate, `<kind>/<name>` (`<kind>` alone for the `project` singleton), taken **verbatim
//! from the repository layout** (`crate::layout::read_layout`) rather than from the manifest's
//! advisory preview:
//!
//! ```text
//! adi/crm-suite                    # the whole bundle
//! adi/crm-suite/agents/sales-bot   # just the agent
//! adi/crm-suite/dashboards/crm     # just the dashboard
//! adi/crm-suite/project            # the scaffold — a singleton, no name needed
//! ```
//!
//! `kind` is part of the address, not just of the layout, because the same published name can
//! appear under two kinds (`agents/reviewer` and `tools/reviewer` are not the same element) and
//! the address has to disambiguate that without reading the repository first
//! (`docs/marketplace-bundles.md`).

use std::fmt;

use crate::error::{Error, Result};
use crate::kind::Kind;

/// One marketplace item, addressed down to at most one element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// The source's local name — the first half, unaffected by whether an element follows.
    pub marketplace: String,
    /// The bundle's published slug — the second half.
    pub slug: String,
    /// The element within it, or `None` for the whole bundle.
    pub element: Option<ElementAddress>,
}

/// The third coordinate: which element, of which kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementAddress {
    /// Which of the eight kind directories it lives under.
    pub kind: Kind,
    /// The published name — absent only for [`Kind::Project`], the one singleton kind.
    pub name: Option<String>,
}

impl Address {
    /// Parse `<marketplace>/<slug>`, `<marketplace>/<slug>/<kind>/<name>`, or
    /// `<marketplace>/<slug>/project`.
    ///
    /// # Errors
    /// [`Error::BadSpec`] when the first two segments — which marketplace, which bundle — are
    /// missing. [`Error::BadAddress`] for anything after them that is not one of the two element
    /// shapes above: an unknown kind segment, a name on the `project` singleton, a name that is
    /// not one safe path segment, or more segments than either shape has room for.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut segments = spec.split('/');
        let marketplace = segments.next().filter(|s| !s.is_empty());
        let slug = segments.next().filter(|s| !s.is_empty());
        let (Some(marketplace), Some(slug)) = (marketplace, slug) else {
            return Err(Error::BadSpec(spec.to_string()));
        };

        let rest: Vec<&str> = segments.collect();
        let element = match rest.as_slice() {
            [] => None,
            [kind] => Some(singleton(spec, kind)?),
            [kind, name] => Some(named(spec, kind, name)?),
            _ => return Err(Error::BadAddress(spec.to_string())),
        };

        Ok(Address {
            marketplace: marketplace.to_string(),
            slug: slug.to_string(),
            element,
        })
    }
}

/// `<marketplace>/<slug>/<kind>` with nothing after it — only ever the `project` singleton.
fn singleton(spec: &str, kind: &str) -> Result<ElementAddress> {
    if Kind::from_dir(kind) == Some(Kind::Project) {
        Ok(ElementAddress {
            kind: Kind::Project,
            name: None,
        })
    } else {
        Err(Error::BadAddress(spec.to_string()))
    }
}

/// `<marketplace>/<slug>/<kind>/<name>` — every kind but the singleton, which takes no name.
fn named(spec: &str, kind: &str, name: &str) -> Result<ElementAddress> {
    let kind = Kind::from_dir(kind).ok_or_else(|| Error::BadAddress(spec.to_string()))?;
    if kind.is_singleton() || !adi_config::valid_name(name) {
        return Err(Error::BadAddress(spec.to_string()));
    }
    Ok(ElementAddress {
        kind,
        name: Some(name.to_string()),
    })
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.marketplace, self.slug)?;
        if let Some(element) = &self.element {
            write!(f, "/{}", element.kind)?;
            if let Some(name) = &element.name {
                write!(f, "/{name}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_spec_names_the_whole_bundle() {
        let addr = Address::parse("adi/crm-suite").expect("parses");
        assert_eq!(addr.marketplace, "adi");
        assert_eq!(addr.slug, "crm-suite");
        assert_eq!(addr.element, None);
        assert_eq!(addr.to_string(), "adi/crm-suite", "round-trips");
    }

    #[test]
    fn a_kind_and_name_addresses_one_element() {
        let addr = Address::parse("adi/crm-suite/agents/sales-bot").expect("parses");
        assert_eq!(
            addr.element,
            Some(ElementAddress {
                kind: Kind::Agent,
                name: Some("sales-bot".to_string()),
            })
        );
        assert_eq!(addr.to_string(), "adi/crm-suite/agents/sales-bot");

        // The same name under a different kind is a different element — the whole reason kind is
        // part of the address rather than just of the layout.
        let other = Address::parse("adi/crm-suite/tools/sales-bot").expect("parses");
        assert_ne!(addr, other);
    }

    #[test]
    fn the_project_scaffold_is_a_singleton_addressed_with_no_name() {
        let addr = Address::parse("adi/crm-suite/project").expect("parses");
        assert_eq!(
            addr.element,
            Some(ElementAddress {
                kind: Kind::Project,
                name: None,
            })
        );
        assert_eq!(addr.to_string(), "adi/crm-suite/project");

        // Giving it a name anyway is refused rather than ignored — there is exactly one, and an
        // address that named one that does not exist would be a silent typo.
        assert!(matches!(
            Address::parse("adi/crm-suite/project/scaffold"),
            Err(Error::BadAddress(_))
        ));
    }

    #[test]
    fn every_wrong_shape_is_refused_by_name() {
        for spec in ["", "adi", "adi/"] {
            assert!(matches!(Address::parse(spec), Err(Error::BadSpec(_))), "{spec:?}");
        }
        for spec in [
            "adi/crm-suite/nope/sales-bot",   // not one of the eight directories
            "adi/crm-suite/agents",           // a non-singleton kind with no name
            "adi/crm-suite/agents/..",        // a name that is not one safe path segment
            "adi/crm-suite/agents/sales-bot/extra", // too many segments
        ] {
            assert!(matches!(Address::parse(spec), Err(Error::BadAddress(_))), "{spec:?}");
        }
    }
}
