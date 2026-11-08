use std::fmt;
use std::str::FromStr;

/// Opaque 128-bit identity for document objects, serialized as 32 lowercase
/// hex digits. Renaming, recoloring, reordering or moving an object never
/// changes its identity, and deleted ids are never reused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(u128);

impl Id {
    /// The nil id (used only as a sentinel, never for a real object).
    pub const NIL: Id = Id(0);

    /// A fresh random id.
    pub fn new() -> Id {
        Id(uuid::Uuid::new_v4().as_u128())
    }

    pub const fn from_u128(v: u128) -> Id {
        Id(v)
    }

    pub const fn as_u128(self) -> u128 {
        self.0
    }

    pub fn is_nil(self) -> bool {
        self.0 == 0
    }

    /// A short stable hash for UI widget ids and engine lookups.
    pub fn short(self) -> u64 {
        (self.0 as u64) ^ ((self.0 >> 64) as u64)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:08x})", (self.0 >> 96) as u32)
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl FromStr for Id {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Id, Self::Err> {
        u128::from_str_radix(s.trim(), 16).map(Id)
    }
}

impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Id, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
