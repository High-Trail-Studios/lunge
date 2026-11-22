//! The AWS regions Lunge offers when setting a profile's default region.
//!
//! A fixed list: the CLI has no command that lists regions without
//! credentials, and asking AWS (`aws account list-regions`) would need role
//! credentials just to fill a dropdown. New regions need a Lunge release to
//! appear; a region already set on a profile is shown whether or not it's
//! listed here.

/// A region code and its display name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub code: &'static str,
    pub name: &'static str,
}

const fn region(code: &'static str, name: &'static str) -> Region {
    Region { code, name }
}

/// Commercial regions first (US, then the rest by code), then GovCloud and
/// China, which only work with accounts in those partitions.
pub const REGIONS: &[Region] = &[
    region("us-east-1", "US East (N. Virginia)"),
    region("us-east-2", "US East (Ohio)"),
    region("us-west-1", "US West (N. California)"),
    region("us-west-2", "US West (Oregon)"),
    region("af-south-1", "Africa (Cape Town)"),
    region("ap-east-1", "Asia Pacific (Hong Kong)"),
    region("ap-east-2", "Asia Pacific (Taipei)"),
    region("ap-northeast-1", "Asia Pacific (Tokyo)"),
    region("ap-northeast-2", "Asia Pacific (Seoul)"),
    region("ap-northeast-3", "Asia Pacific (Osaka)"),
    region("ap-south-1", "Asia Pacific (Mumbai)"),
    region("ap-south-2", "Asia Pacific (Hyderabad)"),
    region("ap-southeast-1", "Asia Pacific (Singapore)"),
    region("ap-southeast-2", "Asia Pacific (Sydney)"),
    region("ap-southeast-3", "Asia Pacific (Jakarta)"),
    region("ap-southeast-4", "Asia Pacific (Melbourne)"),
    region("ap-southeast-5", "Asia Pacific (Malaysia)"),
    region("ap-southeast-6", "Asia Pacific (New Zealand)"),
    region("ap-southeast-7", "Asia Pacific (Thailand)"),
    region("ca-central-1", "Canada (Central)"),
    region("ca-west-1", "Canada West (Calgary)"),
    region("eu-central-1", "Europe (Frankfurt)"),
    region("eu-central-2", "Europe (Zurich)"),
    region("eu-north-1", "Europe (Stockholm)"),
    region("eu-south-1", "Europe (Milan)"),
    region("eu-south-2", "Europe (Spain)"),
    region("eu-west-1", "Europe (Ireland)"),
    region("eu-west-2", "Europe (London)"),
    region("eu-west-3", "Europe (Paris)"),
    region("il-central-1", "Israel (Tel Aviv)"),
    region("me-central-1", "Middle East (UAE)"),
    region("me-south-1", "Middle East (Bahrain)"),
    region("mx-central-1", "Mexico (Central)"),
    region("sa-east-1", "South America (São Paulo)"),
    region("us-gov-east-1", "AWS GovCloud (US-East)"),
    region("us-gov-west-1", "AWS GovCloud (US-West)"),
    region("cn-north-1", "China (Beijing)"),
    region("cn-northwest-1", "China (Ningxia)"),
];

/// The display name for a known region code.
pub fn name(code: &str) -> Option<&'static str> {
    REGIONS.iter().find(|r| r.code == code).map(|r| r.name)
}

/// Whether `code` has the shape of a region code (`xx-name-1`), listed or
/// not. Keeps anything odd off the `aws configure set` command line.
pub fn looks_like_region(code: &str) -> bool {
    let parts: Vec<&str> = code.split('-').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
        && parts
            .last()
            .is_some_and(|p| p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_region_is_well_formed_and_unique() {
        for (i, r) in REGIONS.iter().enumerate() {
            assert!(looks_like_region(r.code), "{}", r.code);
            assert!(!r.name.is_empty(), "{}", r.code);
            assert!(
                !REGIONS[..i].iter().any(|o| o.code == r.code),
                "duplicate {}",
                r.code
            );
        }
    }

    #[test]
    fn region_shape() {
        for ok in [
            "us-east-1",
            "us-gov-west-1",
            "ap-southeast-7",
            "eusc-de-east-1",
        ] {
            assert!(looks_like_region(ok), "{ok}");
        }
        for bad in [
            "",
            "us-east",
            "US-EAST-1",
            "us east 1",
            "us-east-1;",
            "-us-east-1",
            "us--1",
            "us-east-x",
        ] {
            assert!(!looks_like_region(bad), "{bad}");
        }
        assert_eq!(name("eu-west-1"), Some("Europe (Ireland)"));
        assert_eq!(name("xx-nowhere-9"), None);
    }
}
