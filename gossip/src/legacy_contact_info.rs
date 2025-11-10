use {crate::crds_data::reject_deserialize, serde::Serialize};

/// Structure representing a node on the network
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[repr(C)]
pub(crate) struct LegacyContactInfo {}
reject_deserialize!(LegacyContactInfo, "LegacyContactInfo is deprecated");
