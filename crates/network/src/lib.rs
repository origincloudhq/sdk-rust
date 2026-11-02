//! The Origin Cloud network API: VPCs, subnets, routing, gateways, floating IPs, network interfaces, security groups, ACLs and DNS policies.
//!
//! The clients are generated from the API definitions, `origincloud.network`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
