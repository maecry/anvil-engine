use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Serde adapter writing a `u128` as a hyphenated UUID string (RON/JSON friendly).
pub mod u128_str {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    /// Serializes as `xxxxxxxx-xxxx-...`.
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&Uuid::from_u128(*v).hyphenated())
    }

    /// Parses the string form.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        Uuid::parse_str(&s)
            .map(|u| u.as_u128())
            .map_err(serde::de::Error::custom)
    }
}

/// Stable asset identity: a UUID stored in the asset's `.meta` sidecar (ADR-0014 §4).
/// `AssetId::NIL` means "none / use the built-in default".
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(#[serde(with = "u128_str")] pub u128);

impl AssetId {
    /// The "no asset" id.
    pub const NIL: Self = Self(0);

    /// A fresh random id.
    pub fn random() -> Self {
        Self(Uuid::new_v4().as_u128())
    }

    /// Deterministic id of a sub-asset: UUID v5 of `name` in the namespace of `parent`.
    pub fn derive(parent: AssetId, name: &str) -> Self {
        Self(Uuid::new_v5(&Uuid::from_u128(parent.0), name.as_bytes()).as_u128())
    }

    /// True for [`AssetId::NIL`].
    pub fn is_nil(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", Uuid::from_u128(self.0).hyphenated())
    }
}

impl std::fmt::Debug for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AssetId({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_is_deterministic_and_distinct() {
        let p = AssetId::random();
        assert_eq!(AssetId::derive(p, "mesh/0"), AssetId::derive(p, "mesh/0"));
        assert_ne!(AssetId::derive(p, "mesh/0"), AssetId::derive(p, "mesh/1"));
        assert_ne!(AssetId::derive(p, "mesh/0"), p);
    }

    #[test]
    fn ron_round_trip() {
        let id = AssetId::random();
        let s = ron::to_string(&id).unwrap();
        assert_eq!(ron::from_str::<AssetId>(&s).unwrap(), id);
    }
}
