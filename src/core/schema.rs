//! The schema operations over this host's [`SchemaEntry`] (Phase 2207,
//! WIRE_FORMAT §3.6.26): the cross-host fingerprint, the structural delta with
//! its replay and canonical wire form, and the depended-on-column compatibility
//! verdict — each reading the field metadata exactly as the reference does.
//!
//! The operations themselves are the Core spine's (`fuaran_core::field`, the
//! twin of `Fuaran.Core.Schema` and `SchemaDeltaCodec`, certified against Core's
//! own copy of `laws/field-laws.json`). This module is the bridge: it carries an
//! entry across to the spine's `Field` and back, so the host keeps ONE
//! implementation of each operation rather than a second twin that could drift
//! from the first. The two column-type enums share one wire tag set, which is
//! what the bridge maps through.

use super::dataframe::{ColumnType, SchemaEntry};
use fuaran_core::column::ColumnType as CoreType;
use fuaran_core::field::{self, Field};

pub use fuaran_core::field::{FieldChange, SchemaCompat, SchemaDelta, SchemaError};

/// The spine's column type for a host column type, by wire tag.
fn core_type(ty: ColumnType) -> CoreType {
    CoreType::of_tag(ty.as_str())
        .unwrap_or_else(|| panic!("the Core spine has no column type '{}'", ty.as_str()))
}

/// The host's column type for a spine column type, by wire tag.
fn host_type(ty: CoreType) -> ColumnType {
    ColumnType::from_wire(ty.tag())
        .unwrap_or_else(|| panic!("this host has no column type '{}'", ty.tag()))
}

/// An entry as the spine's field: every member carried.
pub fn to_field(e: &SchemaEntry) -> Field {
    e.ext.iter().fold(
        Field::create(&e.name, core_type(e.column_type))
            .with_unit(e.unit.clone())
            .with_label(e.label.as_deref())
            .with_description(e.description.as_deref()),
        |f, (k, v)| f.with_ext(k, v),
    )
}

/// The spine's field as an entry: every member carried.
pub fn of_field(f: &Field) -> SchemaEntry {
    let mut e = SchemaEntry::new(f.name(), host_type(f.column_type()));
    e.unit = f.unit().cloned();
    e.label = f.label().map(str::to_string);
    e.description = f.description().map(str::to_string);
    e.ext = f
        .ext()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    e
}

fn fields(schema: &[SchemaEntry]) -> Vec<Field> {
    schema.iter().map(to_field).collect()
}

/// The stable cross-host fingerprint of a schema (`Schema.fingerprint`): eight
/// lower-case hex digits of FNV-1a over the canonical pre-image of each entry's
/// `name:type`, followed by its metadata element where it states any — so a
/// schema without metadata keeps the fingerprint it had before Phase 2207.
pub fn fingerprint(schema: &[SchemaEntry]) -> String {
    field::fingerprint(&fields(schema))
}

/// The structured delta from `old` to `target` (`Schema.diff`; total): added,
/// removed, retyped, the metadata `amended` on columns common to both, and the
/// order.
pub fn diff(old: &[SchemaEntry], target: &[SchemaEntry]) -> SchemaDelta {
    field::diff(&fields(old), &fields(target))
}

/// Replay `delta` over `old` (`Schema.patch`): `patch(old, diff(old, target))`
/// is `target`. Refused where the delta does not describe `old`.
pub fn patch(old: &[SchemaEntry], delta: &SchemaDelta) -> Result<Vec<SchemaEntry>, SchemaError> {
    field::patch(&fields(old), delta).map(|s| s.iter().map(of_field).collect())
}

/// The compatibility verdict of `delta` for a consumer depending on
/// `depends_on` (`Schema.classify`): a removed or narrowed depended-on column,
/// or one whose stated unit was replaced or withdrawn, is breaking; a moved
/// extension member is unknown; breaking outranks unknown.
pub fn classify(depends_on: &[&str], delta: &SchemaDelta) -> SchemaCompat {
    field::classify(depends_on, delta)
}

/// The canonical wire bytes of a delta (`SchemaDeltaCodec.encode`): `added`,
/// `removed`, `retyped`, `reordered` and `order` always, `amended` only when a
/// member moved.
pub fn encode_delta(delta: &SchemaDelta) -> String {
    field::encode_delta(delta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_host_column_type_crosses_the_bridge_and_back() {
        for tag in ColumnType::WIRE_NAMES {
            let ty = ColumnType::from_wire(tag).unwrap();
            assert_eq!(host_type(core_type(ty)), ty, "{tag}");
        }
        assert_eq!(ColumnType::WIRE_NAMES.len(), CoreType::ALL.len());
    }

    #[test]
    fn an_entry_crosses_the_bridge_with_every_member() {
        let mut e = SchemaEntry::new("m", ColumnType::Float);
        e.unit = Some(super::super::unit::parse("kg").unwrap());
        e.label = Some(String::new());
        e.description = Some("d".to_string());
        e.ext.insert("v.k".to_string(), "1".to_string());
        assert_eq!(of_field(&to_field(&e)), e);
    }
}
