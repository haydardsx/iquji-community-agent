use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(banner: &str) -> Option<Self> {
        banner
            .split(|character: char| {
                !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
            })
            .find_map(|token| {
                let mut parts = token.split(['-', '+']).next()?.split('.');
                let major = parts.next()?.parse().ok()?;
                let minor = parts.next()?.parse().ok()?;
                let patch = parts.next().map_or(Some(0), |value| value.parse().ok())?;
                if parts.next().is_some() {
                    None
                } else {
                    Some(Self(major, minor, patch))
                }
            })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0, self.1, self.2)
    }
}
