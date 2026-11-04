//! The Origin Cloud billing API: billing accounts and the projects they pay for, payment methods, cost and usage, spend limits and the price list.
//!
//! The clients are generated from the API definitions, `origincloud.billing`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
