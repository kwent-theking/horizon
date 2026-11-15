//! Bind resource selectors to a block's property schema and ordered states.
use super::{BlockStateDefinition, Condition, Term};
use crate::json::{Result, error};
use horizon_core::text::parse_java_integer;
use std::collections::{BTreeMap, BTreeSet};

pub type Properties = BTreeMap<String, String>;
pub type PropertySchema = BTreeMap<String, PropertyValues>;
#[derive(Clone, Debug)]
pub enum PropertyValues {
    Named(BTreeSet<String>),
    Integer { minimum: i32, maximum: i32 },
}
impl PropertyValues {
    fn parse(&self, text: &str) -> Option<String> {
        match self {
            Self::Named(values) => values.contains(text).then(|| text.to_owned()),
            Self::Integer { minimum, maximum } => {
                let value = parse_java_integer(text)?;
                (value >= *minimum && value <= *maximum).then(|| value.to_string())
            }
        }
    }
}
/// Indices into the original definition. An empty multipart selection is distinct
/// from an absent root: the native root exists but contributes no model parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectedModels {
    Variant(usize),
    Multipart(Vec<usize>),
}
#[derive(Clone, Debug)]
enum BoundCondition {
    Or(Vec<Self>),
    And(Vec<Self>),
    Properties(BTreeMap<String, Vec<Term>>),
}
impl BoundCondition {
    fn bind(value: &Condition, schema: &PropertySchema) -> Result<Self> {
        Ok(match value {
            Condition::Or(values) => Self::Or(
                values
                    .iter()
                    .map(|v| Self::bind(v, schema))
                    .collect::<Result<_>>()?,
            ),
            Condition::And(values) => Self::And(
                values
                    .iter()
                    .map(|v| Self::bind(v, schema))
                    .collect::<Result<_>>()?,
            ),
            Condition::Properties(values) => Self::Properties(
                values
                    .iter()
                    .map(|(key, terms)| {
                        let domain = schema
                            .get(key)
                            .ok_or_else(|| error("unknown multipart property"))?;
                        let terms = terms
                            .iter()
                            .map(|t| {
                                Ok(Term {
                                    value: domain
                                        .parse(&t.value)
                                        .ok_or_else(|| error("unknown multipart property value"))?,
                                    negated: t.negated,
                                })
                            })
                            .collect::<Result<_>>()?;
                        Ok((key.clone(), terms))
                    })
                    .collect::<Result<_>>()?,
            ),
        })
    }
    fn matches(&self, state: &Properties) -> bool {
        match self {
            Self::Or(values) => values.iter().any(|v| v.matches(state)),
            Self::And(values) => values.iter().all(|v| v.matches(state)),
            Self::Properties(values) => values.iter().all(|(key, terms)| {
                state
                    .get(key)
                    .is_some_and(|value| terms.iter().any(|t| (value == &t.value) ^ t.negated))
            }),
        }
    }
}
impl Condition {
    fn has_unknown_value(&self, schema: &PropertySchema) -> bool {
        match self {
            Self::Or(values) | Self::And(values) => {
                values.iter().any(|v| v.has_unknown_value(schema))
            }
            Self::Properties(values) => values.iter().any(|(key, terms)| {
                schema
                    .get(key)
                    .is_some_and(|domain| terms.iter().any(|t| domain.parse(&t.value).is_none()))
            }),
        }
    }
}
fn variant_predicate(text: &str, schema: &PropertySchema) -> Result<Properties> {
    let mut result = Properties::new();
    for clause in text.split(',') {
        let (key, value) = clause
            .split_once('=')
            .map_or((clause, None), |(k, v)| (k, Some(v)));
        if let (Some(domain), Some(value)) = (schema.get(key), value) {
            result.insert(
                key.to_owned(),
                domain
                    .parse(value)
                    .ok_or_else(|| error("unknown variant property value"))?,
            );
        } else if !key.is_empty() {
            return Err(error("unknown or incomplete variant property"));
        }
    }
    Ok(result)
}
impl BlockStateDefinition {
    /// Imported resources can name values absent from the imported registry.
    /// Drop those multipart selectors before binding and compiling choices.
    /// Unknown property names remain errors in `bind`.
    pub fn skip_unknown_multipart_values(&mut self, schema: &PropertySchema) {
        if let Some(selectors) = &mut self.multipart {
            selectors.retain(|s| {
                !s.condition
                    .as_ref()
                    .is_some_and(|v| v.has_unknown_value(schema))
            });
        }
    }
    /// `states` must be the block's canonical possible states in native order.
    /// Invalid variant selectors are skipped. On overlapping selectors, vanilla
    /// replaces the first overlapping state before aborting that selector's walk.
    /// Multipart predicates bind eagerly, even when variants cover every state.
    pub fn bind(
        &self,
        schema: &PropertySchema,
        states: &[Properties],
    ) -> Result<Vec<Option<SelectedModels>>> {
        let mut result = vec![None; states.len()];
        if let Some(variants) = &self.variants {
            for (index, (key, _)) in variants.iter().enumerate() {
                let Ok(predicate) = variant_predicate(key, schema) else {
                    continue;
                };
                for (target, state) in result.iter_mut().zip(states) {
                    if predicate
                        .iter()
                        .all(|(key, value)| state.get(key) == Some(value))
                        && target.replace(SelectedModels::Variant(index)).is_some()
                    {
                        break;
                    }
                }
            }
        }
        if let Some(selectors) = &self.multipart {
            let predicates = selectors
                .iter()
                .map(|s| {
                    s.condition
                        .as_ref()
                        .map(|v| BoundCondition::bind(v, schema))
                        .transpose()
                })
                .collect::<Result<Vec<_>>>()?;
            for (target, state) in result.iter_mut().zip(states) {
                if target.is_none() {
                    let selected = predicates
                        .iter()
                        .enumerate()
                        .filter_map(|(index, p)| {
                            p.as_ref().is_none_or(|p| p.matches(state)).then_some(index)
                        })
                        .collect();
                    *target = Some(SelectedModels::Multipart(selected));
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockstate::choice::CompiledChoices;
    use horizon_core::random::LegacyRandom;

    fn schema() -> PropertySchema {
        BTreeMap::from([
            (
                "north".to_owned(),
                PropertyValues::Named(["none".to_owned(), "side".to_owned()].into()),
            ),
            (
                "age".to_owned(),
                PropertyValues::Integer {
                    minimum: 0,
                    maximum: 3,
                },
            ),
        ])
    }

    fn state(north: &str, age: &str) -> Properties {
        BTreeMap::from([
            ("north".to_owned(), north.to_owned()),
            ("age".to_owned(), age.to_owned()),
        ])
    }

    #[test]
    fn imported_multipart_skips_unknown_named_values_and_keeps_choices_aligned() {
        let mut definition = BlockStateDefinition::from_json(
            r#"{"multipart":[
                {"apply":{"model":"test:base"}},
                {"when":{"north":"low"},"apply":{"model":"test:unused"}},
                {"when":{"north":"side"},"apply":[
                    {"model":"test:side","y":90,"uvlock":true,"weight":3}
                ]}
            ]}"#,
        )
        .unwrap();
        let original = definition.multipart.clone().unwrap();
        definition.skip_unknown_multipart_values(&schema());
        assert_eq!(
            definition.multipart,
            Some(vec![original[0].clone(), original[2].clone()])
        );
        let bound = definition
            .bind(&schema(), &[state("none", "0"), state("side", "0")])
            .unwrap();
        assert_eq!(
            bound,
            vec![
                Some(SelectedModels::Multipart(vec![0])),
                Some(SelectedModels::Multipart(vec![0, 1])),
            ]
        );
        let choices = CompiledChoices::new(&definition).unwrap();
        let mut models = Vec::new();
        choices
            .collect(
                bound[1].as_ref().unwrap(),
                &mut LegacyRandom::new(0),
                &mut models,
            )
            .unwrap();
        assert_eq!(
            models.iter().map(|v| v.model.as_str()).collect::<Vec<_>>(),
            vec!["test:base", "test:side"]
        );
        assert_eq!(models[1].rotations, [0, 1, 0]);
        assert!(models[1].uv_lock);
    }

    #[test]
    fn imported_multipart_checks_every_integer_alternative() {
        let mut definition = BlockStateDefinition::from_json(
            r#"{"multipart":[
                {"when":{"age":"4|5"},"apply":{"model":"test:outside"}},
                {"when":{"age":"1|9"},"apply":{"model":"test:mixed"}},
                {"when":{"age":"-1"},"apply":{"model":"test:negative"}},
                {"when":{"age":"01|+2"},"apply":{"model":"test:valid"}}
            ]}"#,
        )
        .unwrap();
        definition.skip_unknown_multipart_values(&schema());
        assert_eq!(definition.multipart.as_ref().unwrap().len(), 1);
        assert_eq!(
            definition
                .bind(
                    &schema(),
                    &[
                        state("none", "0"),
                        state("none", "1"),
                        state("none", "2"),
                        state("none", "3"),
                    ],
                )
                .unwrap(),
            vec![
                Some(SelectedModels::Multipart(vec![])),
                Some(SelectedModels::Multipart(vec![0])),
                Some(SelectedModels::Multipart(vec![0])),
                Some(SelectedModels::Multipart(vec![])),
            ]
        );
    }

    #[test]
    fn imported_multipart_checks_nested_conditions_and_negated_terms() {
        let mut definition = BlockStateDefinition::from_json(
            r#"{"multipart":[
                {"when":{"OR":[{"north":"up"},{"age":"1"}]},
                 "apply":{"model":"test:unknown_or"}},
                {"when":{"AND":[{"north":"side"},{"age":"8"}]},
                 "apply":{"model":"test:unknown_and"}},
                {"when":{"north":"!low"},"apply":{"model":"test:unknown_negated"}},
                {"when":{"OR":[{"north":"!none"},{"AND":[{"age":"1"},{"north":"none"}]}]},
                 "apply":{"model":"test:valid"}}
            ]}"#,
        )
        .unwrap();
        definition.skip_unknown_multipart_values(&schema());
        assert_eq!(definition.multipart.as_ref().unwrap().len(), 1);
        assert_eq!(
            definition
                .bind(
                    &schema(),
                    &[state("none", "0"), state("none", "1"), state("side", "0")],
                )
                .unwrap(),
            vec![
                Some(SelectedModels::Multipart(vec![])),
                Some(SelectedModels::Multipart(vec![0])),
                Some(SelectedModels::Multipart(vec![0])),
            ]
        );
    }

    #[test]
    fn imported_multipart_keeps_an_empty_root_when_all_cases_are_skipped() {
        let mut definition = BlockStateDefinition::from_json(
            r#"{"multipart":[{"when":{"north":"low"},"apply":{"model":"test:unused"}}]}"#,
        )
        .unwrap();
        definition.skip_unknown_multipart_values(&schema());
        assert_eq!(definition.multipart, Some(vec![]));
        assert_eq!(
            definition.bind(&schema(), &[state("none", "0")]).unwrap(),
            vec![Some(SelectedModels::Multipart(vec![]))]
        );
        let choices = CompiledChoices::new(&definition).unwrap();
        assert!(
            choices
                .particle_variant(&SelectedModels::Multipart(vec![]))
                .is_none()
        );
    }

    #[test]
    fn imported_multipart_leaves_valid_definitions_and_variants_unchanged() {
        let mut definition = BlockStateDefinition::from_json(
            r#"{"variants":{"north=unknown":{"model":"test:variant"}},"multipart":[
                {"apply":{"model":"test:base"}},
                {"when":{"north":"none|!side","age":"0|3"},
                 "apply":{"model":"test:valid"}}
            ]}"#,
        )
        .unwrap();
        let original = definition.clone();
        definition.skip_unknown_multipart_values(&schema());
        assert_eq!(definition, original);
        assert_eq!(
            definition.bind(&schema(), &[state("none", "0")]).unwrap(),
            vec![Some(SelectedModels::Multipart(vec![0, 1]))]
        );
    }

    #[test]
    fn strict_binding_and_unknown_multipart_properties_still_fail() {
        let definition = BlockStateDefinition::from_json(
            r#"{"multipart":[{"when":{"north":"low"},"apply":{"model":"test:unused"}}]}"#,
        )
        .unwrap();
        assert!(definition.bind(&schema(), &[state("none", "0")]).is_err());

        let mut definition = BlockStateDefinition::from_json(
            r#"{"multipart":[{"when":{"missing":"side"},"apply":{"model":"test:unused"}}]}"#,
        )
        .unwrap();
        definition.skip_unknown_multipart_values(&schema());
        assert!(definition.bind(&schema(), &[state("none", "0")]).is_err());
    }
}
