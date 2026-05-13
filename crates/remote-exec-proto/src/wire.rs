pub(crate) fn from_wire_value<T>(value: &str, mappings: &[(T, &'static str)]) -> Option<T>
where
    T: Clone,
{
    mappings
        .iter()
        .find(|(_, wire)| *wire == value)
        .map(|(variant, _)| variant.clone())
}

pub(crate) fn from_wire_value_with_aliases<T>(
    value: &str,
    mappings: &[(T, &'static str)],
    aliases: &[(&'static str, T)],
) -> Option<T>
where
    T: Clone,
{
    from_wire_value(value, mappings).or_else(|| {
        aliases
            .iter()
            .find(|(alias, _)| *alias == value)
            .map(|(_, variant)| variant.clone())
    })
}
