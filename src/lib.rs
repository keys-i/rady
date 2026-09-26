pub mod agent;
pub mod cli;
pub mod delivery;
pub mod github;
pub mod runs;
pub mod ui;

// Keep existing public paths while grouping implementation by domain
pub use agent::{context, routing, session};
pub use delivery::{benchmark, quality};
pub use github::apps;
pub use github::{mentions, reviews, setup};
pub use reviews::{model, repair};

pub type Result<T> = anyhow::Result<T>;
