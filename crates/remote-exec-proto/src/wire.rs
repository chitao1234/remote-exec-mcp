pub(crate) fn from_wire_value<T>(value: &str, mappings: &[(T, &'static str)]) -> Option<T>
where
    T: Clone,
{
    mappings
        .iter()
        .find(|(_, wire)| *wire == value)
        .map(|(variant, _)| variant.clone())
}
