//! Visit one gameplay row at a time, including overwritten `states` fields.
use super::Validate;
use serde::{
    Deserializer,
    de::{DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{fmt, io::Read};

pub(crate) enum Row<'a> {
    Reset,
    Array,
    Entry(&'a RawValue),
}

struct Root<'a, F>(&'a mut F);
struct Rows<'a, F>(&'a mut F);
struct RootSeed<'a, F>(&'a mut F);
struct RowsSeed<'a, F>(&'a mut F);

impl<'de, F: FnMut(Row<'_>)> DeserializeSeed<'de> for RootSeed<'_, F> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(Root(self.0))
    }
}
impl<'de, F: FnMut(Row<'_>)> DeserializeSeed<'de> for RowsSeed<'_, F> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        (self.0)(Row::Reset);
        d.deserialize_any(Rows(self.0))
    }
}

// The existing fact decoder accepts a primitive in place of an object/array
// and supplies defaults. Consume those values exactly as its Container visitor.
macro_rules! primitives {
    () => {
        fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
            Ok(())
        }
    };
}

impl<'de, F: FnMut(Row<'_>)> Visitor<'de> for Root<'_, F> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("gameplay facts")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "states" {
                map.next_value_seed(RowsSeed(&mut *self.0))?;
            } else {
                map.next_value::<Validate>()?;
            }
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element::<Validate>()?.is_some() {}
        Ok(())
    }
    primitives!();
}
impl<'de, F: FnMut(Row<'_>)> Visitor<'de> for Rows<'_, F> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("gameplay states")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        (self.0)(Row::Array);
        while let Some(raw) = seq.next_element::<Box<RawValue>>()? {
            (self.0)(Row::Entry(&raw));
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
            map.next_value::<Validate>()?;
        }
        Ok(())
    }
    primitives!();
}

pub(crate) fn read(reader: impl Read, mut row: impl FnMut(Row<'_>)) -> serde_json::Result<()> {
    let mut d = serde_json::Deserializer::from_reader(reader);
    RootSeed(&mut row).deserialize(&mut d)?;
    d.end()
}
