//! `major.minor[.patch]` 版本解析与 API 兼容性门禁。

use std::fmt;
use std::str::FromStr;

/// 解析得到的版本号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl ApiVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// 兼容性规则：major 必须一致，插件 minor 不得高于宿主 minor。
    pub fn is_compatible_with_host(self, host: Self) -> bool {
        self.major == host.major && self.minor <= host.minor
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 版本号格式非法。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("版本号非法: {0}")]
pub struct VersionError(pub String);

impl FromStr for ApiVersion {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || VersionError(s.to_string());
        let mut parts = s.split('.');
        let major: u64 = parts.next().ok_or_else(err)?.parse().map_err(|_| err())?;
        let minor: u64 = match parts.next() {
            None => 0,
            Some(p) => p.parse().map_err(|_| err())?,
        };
        let patch: u64 = match parts.next() {
            None => 0,
            Some(p) => p.parse().map_err(|_| err())?,
        };
        if parts.next().is_some() {
            return Err(err());
        }
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_and_short_forms() {
        assert_eq!("1.0.0".parse::<ApiVersion>().unwrap(), ApiVersion::new(1, 0, 0));
        assert_eq!("1.2".parse::<ApiVersion>().unwrap(), ApiVersion::new(1, 2, 0));
        assert_eq!("3".parse::<ApiVersion>().unwrap(), ApiVersion::new(3, 0, 0));
    }

    #[test]
    fn reject_garbage() {
        for bad in ["", "abc", "1.x", "1.2.3.4", "-1.0", "1..2"] {
            assert!(bad.parse::<ApiVersion>().is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn compatibility_rules() {
        let host = ApiVersion::new(1, 0, 0);
        assert!(ApiVersion::new(1, 0, 9).is_compatible_with_host(host));
        assert!(ApiVersion::new(1, 0, 0).is_compatible_with_host(host));
        assert!(!ApiVersion::new(1, 1, 0).is_compatible_with_host(host));
        assert!(!ApiVersion::new(2, 0, 0).is_compatible_with_host(host));
        assert!(!ApiVersion::new(0, 9, 0).is_compatible_with_host(host));
    }
}
