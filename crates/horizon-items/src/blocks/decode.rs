//! Typed projection of gameplay facts. Scalar conversions deliberately retain Value's
//! permissive defaults; maps retain its last-key-wins behavior. Repeated geometry stays
//! borrowed until it is interned, rather than allocating a JSON tree for every state.
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, value::RawValue};
use std::{collections::HashMap, fmt, marker::PhantomData, sync::Arc};
mod stream;
pub(super) use stream::{Row, read as stream};

// RawValue skips numeric range and depth checks. A non-allocating validation pass
// keeps the original Value loader's JSON errors, even in overwritten/unused fields.
#[derive(Default)]
pub(super) struct Validate;
impl<'de> Container<'de> for Validate {}
impl<'de> Deserialize<'de> for Validate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        container(d)
    }
}

trait Container<'de>: Default {
    fn map<A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
            map.next_value::<Validate>()?;
        }
        Ok(Self::default())
    }
    fn seq<A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        while seq.next_element::<Validate>()?.is_some() {}
        Ok(Self::default())
    }
}
struct ContainerVisitor<T>(PhantomData<T>);
impl<'de, T: Container<'de>> Visitor<'de> for ContainerVisitor<T> {
    type Value = T;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON facts")
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::map(map)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
        T::seq(seq)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<T, E> {
        Ok(T::default())
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<T, E> {
        Ok(T::default())
    }
}
fn container<'de, D: Deserializer<'de>, T: Container<'de>>(d: D) -> Result<T, D::Error> {
    d.deserialize_any(ContainerVisitor(PhantomData))
}
macro_rules! read_field {
    ($map:ident, typed) => {
        $map.next_value()?
    };
    ($map:ident, value($convert:expr)) => {
        ($convert)($map.next_value::<Value>()?)
    };
}
macro_rules! object {
    ($name:ident<$life:lifetime> { $($field:ident: $ty:ty = $default:expr => $kind:ident $(($convert:expr))?),* $(,)? }) => {
        pub(super) struct $name<$life> {
            $(pub $field: $ty,)*
            marker: PhantomData<&$life ()>,
        }
        impl Default for $name<'_> {
            fn default() -> Self { Self { $($field: $default,)* marker: PhantomData } }
        }
        impl<$life> Container<$life> for $name<$life> {
            fn map<A: MapAccess<$life>>(mut map: A) -> Result<Self, A::Error> {
                #[derive(Deserialize)]
                #[allow(non_camel_case_types)]
                #[serde(field_identifier)]
                enum Field { $($field,)* #[serde(other)] Unknown }
                let mut result = Self::default();
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        $(Field::$field => result.$field = read_field!(map, $kind $(($convert))?),)*
                        Field::Unknown => { map.next_value::<Validate>()?; }
                    }
                }
                Ok(result)
            }
        }
        impl<$life> Deserialize<$life> for $name<$life> {
            fn deserialize<D: Deserializer<$life>>(d: D) -> Result<Self, D::Error> { container(d) }
        }
    };
}
fn number(v: Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
fn flag(v: Value) -> bool {
    v.as_bool().unwrap_or(false)
}
fn unsigned(v: Value) -> Option<u64> {
    v.as_u64()
}
fn text(v: Value) -> Option<String> {
    if let Value::String(s) = v {
        Some(s)
    } else {
        None
    }
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct Entries<'a>(pub Option<Vec<Entry<'a>>>);
#[cfg(test)]
impl<'de> Container<'de> for Entries<'de> {
    fn seq<A: SeqAccess<'de>>(mut seq: A) -> Result<Self, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = seq.next_element()? {
            entries.push(entry);
        }
        Ok(Self(Some(entries)))
    }
}
#[cfg(test)]
impl<'de> Deserialize<'de> for Entries<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        container(d)
    }
}
#[cfg(test)]
object!(Gameplay<'a> { states: Entries<'a> = Entries::default() => typed });
object!(SoundId<'a> { id: Option<String> = None => value(text) });
object!(Sound<'a> {
    volume: f64 = 1.0 => value(|v: Value| v.as_f64().unwrap_or(1.0)),
    pitch: f64 = 1.0 => value(|v: Value| v.as_f64().unwrap_or(1.0)),
    r#break: SoundId<'a> = SoundId::default() => typed,
    step: SoundId<'a> = SoundId::default() => typed,
    place: SoundId<'a> = SoundId::default() => typed,
    hit: SoundId<'a> = SoundId::default() => typed,
    fall: SoundId<'a> = SoundId::default() => typed,
});
object!(Entry<'a> {
    id: Option<u64> = None => value(unsigned),
    emission: Value = Value::Null => typed,
    dampening: Value = Value::Null => typed,
    skylight_down: bool = false => value(flag),
    shape_occlusion: bool = false => value(flag),
    can_occlude: bool = false => value(flag),
    solid_render: bool = false => value(flag),
    hardness: Value = Value::Null => typed,
    requires_tool: bool = false => value(flag),
    replaceable: bool = false => value(flag),
    random_ticks: bool = false => value(flag),
    block_entity: bool = false => value(flag),
    liquid: bool = false => value(flag),
    ignited_by_lava: bool = false => value(flag),
    push: Option<String> = None => value(text),
    render_shape: Option<String> = None => value(text),
    full_collision: bool = false => value(flag),
    friction: f64 = 0.0 => value(number),
    explosion_resistance: f64 = 0.0 => value(number),
    item: Option<String> = None => value(text),
    sound: Sound<'a> = Sound::default() => typed,
    offset: bool = false => value(flag),
    outline_offset: Option<bool> = None => value(|v: Value| v.as_bool()),
    map_color: Option<u64> = None => value(unsigned),
    outline: Option<&'a RawValue> = None => typed,
    outline_edges: Option<&'a RawValue> = None => typed,
});

type Geometry = Arc<[[f64; 6]]>;

#[derive(Default)]
pub(super) struct Shapes {
    // Memoize raw spellings during the stream so common shapes decode once.
    raw: HashMap<Box<str>, Geometry>,
    by_geometry: HashMap<Vec<[u64; 6]>, Geometry>,
}
impl Shapes {
    pub fn share(&mut self, boxes: Vec<[f64; 6]>) -> Geometry {
        let bits = boxes.iter().map(|row| row.map(f64::to_bits)).collect();
        self.by_geometry
            .entry(bits)
            .or_insert_with(|| boxes.into())
            .clone()
    }

    pub fn clear(&mut self) {
        self.raw.clear();
        // Drop the overwritten rows' geometry, keeping the shared defaults.
        self.by_geometry
            .retain(|_, shape| Arc::strong_count(shape) > 1);
    }

    pub fn intern(
        &mut self,
        raw: Option<&RawValue>,
        fallback: impl FnOnce() -> Geometry,
    ) -> crate::Result<Geometry> {
        let Some(raw) = raw else {
            return Ok(fallback());
        };
        if let Some(shape) = self.raw.get(raw.get()) {
            return Ok(shape.clone());
        }
        let value: Value = serde_json::from_str(raw.get())?;
        let Some(array) = value.as_array() else {
            return Ok(fallback());
        };
        let boxes = array
            .iter()
            .filter_map(|b| {
                let b = b.as_array()?;
                Some(std::array::from_fn(|i| {
                    b.get(i).and_then(Value::as_f64).unwrap_or(0.0)
                }))
            })
            .collect();
        let shape = self.share(boxes);
        self.raw.insert(raw.get().into(), shape.clone());
        Ok(shape)
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
