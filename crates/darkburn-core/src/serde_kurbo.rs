//! Serde helpers for kurbo types: an affine as its six numbers, a path as SVG path data.

pub mod affine {
    use kurbo::Affine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(a: &Affine, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(a.as_coeffs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Affine, D::Error> {
        let c = <[f64; 6]>::deserialize(d)?;
        if c.iter().all(|v| v.is_finite()) { Ok(Affine::new(c)) } else { Err(serde::de::Error::custom("transform is not finite")) }
    }
}

pub mod path {
    use kurbo::BezPath;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(p: &BezPath, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&p.to_svg())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BezPath, D::Error> {
        let text = String::deserialize(d)?;
        if text.trim().is_empty() {
            return Ok(BezPath::new());
        }
        BezPath::from_svg(&text).map_err(|e| serde::de::Error::custom(format!("path data: {e}")))
    }
}

pub mod opt_path {
    use kurbo::BezPath;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(p: &Option<BezPath>, s: S) -> Result<S::Ok, S::Error> {
        match p {
            Some(p) => s.serialize_some(&p.to_svg()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<BezPath>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(t) => BezPath::from_svg(&t).map(Some).map_err(|e| serde::de::Error::custom(format!("path data: {e}"))),
            None => Ok(None),
        }
    }
}
