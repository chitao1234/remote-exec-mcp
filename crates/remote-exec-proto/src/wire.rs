macro_rules! wire_value_mappings {
    ($type:ty { $($variant:ident => $wire:expr),+ $(,)? }) => {
        impl $type {
            pub fn wire_value(&self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            pub fn from_wire_value(value: &str) -> Option<Self> {
                $(if value == $wire { return Some(Self::$variant); })+
                None
            }
        }
    };
}

pub(crate) use wire_value_mappings;
